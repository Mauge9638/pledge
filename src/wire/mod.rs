use std::collections::HashMap;
use std::io::{self, Error};
use std::time::{Duration, Instant};

use sqlx::postgres::types::Oid;
use sqlx::{PgPool, Row};
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::mpsc::Sender;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::tcp::OwnedWriteHalf,
    net::{TcpListener, TcpStream},
    sync::mpsc,
};

use types::{SQLCommand, WireProtocolStates};

use messages::{
    AuthenticationOk, CommandComplete, DataRow, Encode, ErrorResponse, Query, ReadyForQuery,
};
use messages::{Decode, RowDescription};

use crate::AppState;
use crate::cache::store::cache_key_wire;
use crate::config::DatabaseConfig;
use crate::wire::types::{CacheCommandMetadata, SectionType};
use reader::{ByteReader, ByteReaderError, ByteReaderErrorKind};
//use state_handling::{ready_for_query, waiting_for_ssl, waiting_for_startup};
use types::{
    CacheCommand, ClientState, DBState, Portal, PreparedStatement, ProtocolState,
    StateHandlingResult,
};

use messages::{
    BindComplete, BindMessageContent,
    ClientMessageContent::{
        BindMessage, DescribeMessage, ExecuteMessage, ParseMessage, QueryMessage, SyncMessage,
        TerminateMessage,
    },
    DBMessageContent, DescribeMessageContent, ErrorResponseSeverity, ExecuteMessageContent,
    ParseComplete, ParseMessageContent, QueryMessageContent,
};

mod messages;
mod reader;
mod state_handling;
pub mod types;
mod writer;

pub async fn listener_start(listener: TcpListener, app_state: &AppState) {
    loop {
        match listener.accept().await {
            Ok((mut stream, ip)) => {
                let state = app_state.clone();
                let db_stream = match connect_to_db(&state.database_config).await {
                    Ok(stream) => stream,
                    Err(err) => {
                        eprintln!("Failed to connect to database: {}", err);
                        continue;
                    }
                };
                tokio::spawn(async move {
                    println!(
                        "Accepted connection from {:?} on ip {:?}",
                        stream.peer_addr(),
                        ip
                    );
                    spawn_tasks(stream, db_stream, &state).await
                })
            }
            Err(err) => tokio::spawn(async move {
                eprintln!("Failed to accept connection: {}", err);
            }),
        };
    }
}

async fn connect_to_db(database_config: &DatabaseConfig) -> Result<TcpStream, String> {
    match TcpStream::connect(format!("{}:{}", database_config.host, database_config.port)).await {
        Ok(stream) => Ok(stream),
        Err(err) => return Err(err.to_string()),
    }
}

async fn spawn_tasks(client_stream: TcpStream, db_stream: TcpStream, app_state: &AppState) {
    let (mut client_read, client_write) = client_stream.into_split();
    let (mut db_read, db_write) = db_stream.into_split();
    let (tx, mut rx) = mpsc::channel(32 * 1024);
    let mut client_state = ClientState {
        app_state: app_state.clone(),
        buffer: vec![0u8; 8 * 1024],
        prepared_statements: HashMap::new(),
        portals: HashMap::new(),
    };
    let mut db_state = DBState {
        app_state: app_state.clone(),
        buffer: vec![0u8; 32 * 1024],
    };

    let client_spawn = tokio::spawn(async move {
        println!("Accepted connection from {:?} ", client_read.peer_addr(),);
        let mut buffer_data_length;
        'outerLoop: loop {
            buffer_data_length = 0;
            'innerLoop: loop {
                buffer_data_length += match client_read
                    .read(&mut client_state.buffer[buffer_data_length..])
                    .await
                {
                    Ok(n) => n,
                    Err(err) => {
                        eprintln!("Failed to read from client: {}", err);
                        break 'outerLoop;
                    }
                };
                if buffer_data_length == 0 {
                    break 'innerLoop;
                }
                if buffer_data_length >= client_state.buffer.len() {
                    println!("Resizing client buffer");
                    client_state.buffer.resize(client_state.buffer.len() * 2, 0);
                } else {
                    break 'innerLoop;
                }
            }
            handle_client(&mut client_state, &db_write, buffer_data_length, &tx).await;
        }
    });
    let db_spawn = tokio::spawn(async move {
        let mut buffer_data_length;
        'outerLoop: loop {
            buffer_data_length = 0;

            tokio::select! {
                result = db_read.read(&mut db_state.buffer[buffer_data_length..]) => {
                    if let Err(e) = handle_db_read(&result, &mut db_state, &client_write, &mut buffer_data_length).await {
                        eprintln!("db read failed: {e}");
                        break 'outerLoop;
                    }
                }
                cache_commands = rx.recv() => {
                    if let Err(e) = handle_db_cached(cache_commands,&mut db_state, &client_write, &mut buffer_data_length).await {
                        eprintln!("cache handling failed: {e}");
                        break 'outerLoop;
                    }
                }
            }
        }
    });

    let handles = tokio::join!(client_spawn, db_spawn);
}

async fn handle_client(
    client_state: &mut ClientState,
    db_write: &OwnedWriteHalf,
    buffer_data_length: usize,
    tx: &Sender<Vec<CacheCommand>>,
) {
    let mut reader = ByteReader::new(&client_state.buffer[..buffer_data_length], 0);
    match reader.crawl_and_find_messages_client() {
        Ok(messages) => {
            let mut cacheable_entries: Vec<CacheCommand> = Vec::new();
            let mut message_number = 0;
            let mut total_length = 0;
            for message in messages {
                match message {
                    QueryMessage(content, length) => {
                        message_number += 1;
                        println!("query: {:?}", content.query);
                        total_length = 0;
                    }
                    ParseMessage(content, length) => {
                        let _ = parse_message(content, client_state).await;
                        total_length += length;
                    }
                    BindMessage(content, length) => {
                        let _ = bind_message(content, client_state).await;
                        total_length += length;
                    }
                    ExecuteMessage(content, length) => {
                        message_number += 1;
                        total_length += length;
                        match execute_message(content, client_state, message_number, total_length)
                            .await
                        {
                            Ok(result) => match result {
                                Some(result) => match result {
                                    CacheCommand::Replay(value, message_number) => {
                                        cacheable_entries
                                            .push(CacheCommand::Replay(value, message_number));
                                    }
                                    CacheCommand::Capture(key, message_number) => {
                                        cacheable_entries
                                            .push(CacheCommand::Capture(key, message_number));
                                    }
                                },
                                None => {
                                    println!("should not be set in cache")
                                }
                            },
                            Err(err) => {
                                println!("error occured");
                                let _ = err;
                            }
                        }
                        total_length = 0;
                    }
                    _ => {}
                }
            }
            if cacheable_entries.len() > 0 {
                let _ = tx.send(cacheable_entries).await;
                return;
            }
        }
        Err(_) => {}
    }
    stream_try_write(&db_write, &client_state.buffer[..buffer_data_length]).await;
}

async fn handle_db_read(
    result: &Result<usize, Error>,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    buffer_data_length: &mut usize,
) -> Result<(), String> {
    match result {
        Ok(n) => {
            *buffer_data_length += n;
            'innerLoop: loop {
                if *buffer_data_length == 0 {
                    break 'innerLoop;
                }
                if *buffer_data_length >= db_state.buffer.len() {
                    println!("Resizing db buffer");
                    db_state.buffer.resize(db_state.buffer.len() * 2, 0);
                } else {
                    break 'innerLoop;
                }
            }
            let mut reader = ByteReader::new(&db_state.buffer[..*buffer_data_length], 0);
            match reader.crawl_and_find_messages_db() {
                Ok(messages) => {
                    for message in messages {
                        match message {
                            DBMessageContent::ParseComplete => {
                                println!("got ParseComplete")
                            }
                            DBMessageContent::BindComplete => {
                                println!("got BindComplete")
                            }
                            DBMessageContent::RowDescription => {
                                println!("got RowDescription")
                            }
                            DBMessageContent::DataRow => {
                                println!("got DataRow")
                            }
                            DBMessageContent::CommandComplete => {
                                println!("got CommandComplete")
                            }
                            DBMessageContent::ReadyForQuery => {
                                println!("got ReadyForQuery")
                            }
                            DBMessageContent::AuthenticationOk => {
                                println!("got AuthenticationOk")
                            }
                            DBMessageContent::UnknownMessage => {
                                println!("got UnknownMessage")
                            }
                        }
                    }
                }
                Err(_) => {}
            }
            stream_try_write(&client_write, &db_state.buffer[..*buffer_data_length]).await;
            Ok(())
        }
        Err(err) => {
            eprintln!("Failed to read from client: {}", err);
            Err("Failed to read from client:".to_string())
        }
    }
}

async fn handle_db_cached(
    cache_commands: Option<Vec<CacheCommand>>,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    buffer_data_length: &mut usize,
) -> Result<(), String> {
    let mut buffer_data_length = 0;
    match cache_commands {
        Some(commands) => {
            for cache_command in commands {
                match cache_command {
                    CacheCommand::Replay(bytes, message_number) => {
                        stream_try_write(&client_write, &bytes).await;
                        println!("GOT CACHED RESPONSE TO DB SPAWN");
                    }
                    CacheCommand::Capture(key, message_number) => {
                        println!("should cache this, {}", key)
                    }
                }
            }
        }
        None => return Err("cache command not found".to_string()),
    }
    Ok(())
}

pub(super) async fn stream_try_write(stream: &OwnedWriteHalf, buf: &[u8]) -> Option<usize> {
    let mut written = 0;
    while written < buf.len() {
        stream.writable().await.ok()?;
        match stream.try_write(&buf[written..]) {
            Ok(n) => written += n,
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => continue,
            Err(err) => {
                eprintln!("Error occurred: {}", err);
                return None;
            }
        }
    }
    Some(written)
}

async fn parse_message(
    content: ParseMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    let prepared_statement = PreparedStatement {
        query: content.query,
        parameter_data_types: content.parameter_data_types,
    };
    client_state
        .prepared_statements
        .insert(content.prepared_statement_name, prepared_statement);

    println!("saved in prepared statement");
    Ok(())
}

async fn bind_message(
    content: BindMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    let portal = Portal {
        source_prepared_statement_name: content.source_prepared_statement_name,
        parameter_format_codes: content.parameter_format_codes,
        parameter_values: content.parameter_values,
        result_column_format_codes: content.result_column_format_codes,
    };
    client_state.portals.insert(content.portal_name, portal);
    println!("saved in portals");
    Ok(())
}

async fn execute_message(
    content: ExecuteMessageContent,
    client_state: &mut ClientState,
    message_number: i32,
    total_length: usize,
) -> Result<Option<CacheCommand>, StateHandlingResult> {
    let cache_key = match is_cache_configured(content, client_state) {
        Some(cache_key) => cache_key,
        None => return Ok(None),
    };

    return Ok(match get_from_cache(client_state, &cache_key) {
        Some(value) => Some(CacheCommand::Replay(
            value,
            CacheCommandMetadata {
                section_type: SectionType::ExtendedQuery,
                length: total_length,
                message_number,
            },
        )),
        None => Some(CacheCommand::Capture(
            cache_key,
            CacheCommandMetadata {
                section_type: SectionType::ExtendedQuery,
                length: total_length,
                message_number,
            },
        )),
    });
}

fn get_from_cache(client_state: &ClientState, key: &str) -> Option<Vec<u8>> {
    return client_state.app_state.cache.get(&key);
}

fn set_in_cache(app_state: &AppState, query: &str, key: &str, result: Vec<u8>) {
    println!("cache_key set: {}", query);
    if let Some(template) = app_state.matcher.find_template(query) {
        let expiration = match template.ttl {
            Some(ttl) => Instant::now() + Duration::from_secs(ttl),
            None => Instant::now() + Duration::from_secs(app_state.global_ttl),
        };
        app_state.cache.insert(key.to_string(), result, expiration);
    }
}

fn is_cache_configured(
    content: ExecuteMessageContent,
    client_state: &mut ClientState,
) -> Option<String> {
    let portal = match client_state.portals.get(&content.name) {
        Some(portal) => portal,
        None => return None,
    };
    let prepared_statement = match client_state
        .prepared_statements
        .get(&portal.source_prepared_statement_name)
    {
        Some(prepared_statement) => prepared_statement,
        None => return None,
    };
    if client_state
        .app_state
        .matcher
        .template_exists(&prepared_statement.query)
    {
        return Some(cache_key_wire(
            &prepared_statement.query,
            &portal.parameter_values,
        ));
    }
    None
}
