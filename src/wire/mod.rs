use std::collections::{BTreeMap, HashMap};
use std::io::{self, Error};
use std::ops::Range;
use std::sync::Arc;
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
use crate::cache::{lfu::CachedResponse, store::cache_key_wire};
use crate::config::DatabaseConfig;
use crate::wire::messages::ClientMessageContent;
use crate::wire::types::{ProtocolMode, ReplayTrimExtended, ReplayTrimSimple};
use reader::{ByteReader, ByteReaderError, ByteReaderErrorKind};
use writer::ByteWriter;
//use state_handling::{ready_for_query, waiting_for_ssl, waiting_for_startup};
use types::{
    CacheCommand, ClientState, DBState, DescribeKind, Portal, PreparedStatement, ProtocolState,
    ReplayTrim, StateHandlingResult,
};

use messages::{
    BindComplete, BindMessageContent,
    ClientMessageContent::{
        BindMessage, DescribeMessage, ExecuteMessage, ParseMessage, QueryMessage, SyncMessage,
        TerminateMessage,
    },
    DBMessageContent, DescribeMessageContent, DescribeMessageContentTarget, ErrorResponseSeverity,
    ExecuteMessageContent, ParseComplete, ParseMessageContent, QueryMessageContent,
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
                biased; Some(cache_commands) = rx.recv() => {
                    if let Err(e) = handle_db_cached(cache_commands,&mut db_state, &client_write, &mut buffer_data_length).await {
                        eprintln!("cache handling failed: {e}");
                        break 'outerLoop;
                    }
                }
                result = db_read.read(&mut db_state.buffer[buffer_data_length..]) => {
                    if let Err(e) = handle_db_read(&result, &mut db_state, &client_write, &mut buffer_data_length).await {
                        eprintln!("db read failed: {e}");
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
    tx: &Sender<BTreeMap<u16, CacheCommand>>,
) {
    let mut reader = ByteReader::new(&client_state.buffer[..buffer_data_length], 0);
    if let Ok(messages) = reader.crawl_and_find_messages_client() {
        let (cache_commands, replay_trims) =
            find_cache_related_messages(messages, client_state).await;
        if replay_trims.len() > 0 {
            let mut writer = ByteWriter::new(&mut client_state.buffer, 0);
            writer.trim_from_pending_commands(replay_trims);
        }
        if cache_commands.keys().len() > 0 {
            println!("has cache commands");
            println!("has cache commands");
            println!("has cache commands");
            println!("has cache commands");
            println!("has cache commands");
            let _ = tx.send(cache_commands).await;
            return;
        }
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
                            DBMessageContent::RowDescription(content) => {
                                println!("got RowDescription")
                            }
                            DBMessageContent::DataRow(content) => {
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
    cache_commands: BTreeMap<u16, CacheCommand>,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    buffer_data_length: &mut usize,
) -> Result<(), String> {
    println!("LOOKING AT CACHE COMMANDS IN DB_CACHED HANDLER");
    for (_, command) in cache_commands {
        match command {
            CacheCommand::Replay { data, .. } => {
                println!("{:?}", data);
            }
            CacheCommand::Capture { key, .. } => {
                println!("{:?}", key);
            }
        }
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
    content: &ParseMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    if client_state
        .app_state
        .matcher
        .template_exists(&content.query)
    {
        let prepared_statement = PreparedStatement {
            query: content.query.clone(),
            parameter_data_types: content.parameter_data_types.clone(),
        };
        client_state
            .prepared_statements
            .insert(content.prepared_statement_name.clone(), prepared_statement);

        println!("saved in prepared statement");
    }
    Ok(())
}

async fn bind_message(
    content: &BindMessageContent,
    client_state: &mut ClientState,
) -> Result<(), StateHandlingResult> {
    if let Some(prepared_statement) = client_state
        .prepared_statements
        .get(&content.source_prepared_statement_name)
    {
        if client_state
            .app_state
            .matcher
            .template_exists(&prepared_statement.query)
        {
            let portal = Portal {
                source_prepared_statement_name: content.source_prepared_statement_name.clone(),
                parameter_format_codes: content.parameter_format_codes.clone(),
                parameter_values: content.parameter_values.clone(),
                result_column_format_codes: content.result_column_format_codes.clone(),
            };
            client_state
                .portals
                .insert(content.portal_name.clone(), portal);
        }
        println!("saved in portals");
    };
    Ok(())
}

fn describe_message(content: &DescribeMessageContent) -> DescribeKind {
    match content.target {
        DescribeMessageContentTarget::PreparedStatement => DescribeKind::Statement,
        DescribeMessageContentTarget::Portal => DescribeKind::Portal,
    }
}

// async fn execute_message(
//     content: ExecuteMessageContent,
//     client_state: &mut ClientState,
//     order: u16,
//     describe_kind: &DescribeKind,
// ) -> Result<Option<CacheCommand>, StateHandlingResult> {
//     // if a row limit is set, then we can't really cache the result(this is not affected by 'LIMIT')
//     if content.rows_to_return_limit > 0 {
//         let cache_key = match is_cache_configured(&content, client_state) {
//             Some(cache_key) => cache_key,
//             None => return Ok(None),
//         };

//         return Ok(match get_from_cache(client_state, &cache_key) {
//             Some(data) => Some(CacheCommand::Replay {
//                 data,
//                 order,
//                 describe_kind: describe_kind.clone(),
//                 protocol_mode: ProtocolMode::Extended,
//             }),
//             None => Some(CacheCommand::Capture {
//                 key: cache_key,
//                 order,
//                 describe_kind: describe_kind.clone(),
//                 protocol_mode: ProtocolMode::Extended,
//             }),
//         });
//     }
//     Ok(None)
// }

fn get_from_cache(client_state: &ClientState, key: &str) -> Option<Arc<CachedResponse>> {
    return client_state.app_state.cache.get(&key);
}

fn set_in_cache(app_state: &AppState, query: &str, key: &str, data: CachedResponse) {
    println!("cache_key set: {}", key);
    if let Some(template) = app_state.matcher.find_template(query) {
        let expiration = match template.ttl {
            Some(ttl) => Instant::now() + Duration::from_secs(ttl),
            None => Instant::now() + Duration::from_secs(app_state.global_ttl),
        };
        app_state.cache.insert(key.to_string(), data, expiration);
    }
}

fn is_cache_configured(
    content: &ExecuteMessageContent,
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
fn is_cache_configured_simple(query: &String, client_state: &mut ClientState) -> Option<String> {
    if client_state.app_state.matcher.template_exists(&query) {
        return Some(cache_key_wire(&query, &Vec::new()));
    }
    None
}

async fn find_cache_related_messages(
    messages: Vec<ClientMessageContent>,
    client_state: &mut ClientState,
) -> (BTreeMap<u16, CacheCommand>, Vec<ReplayTrim>) {
    let mut cache_commands: BTreeMap<u16, CacheCommand> = BTreeMap::new();
    let mut replay_trims: Vec<ReplayTrim> = Vec::new();
    let mut parse_messages: Vec<(ParseMessageContent, usize, usize)> = Vec::new();
    let mut bind_messages: Vec<(BindMessageContent, usize, usize)> = Vec::new();
    let mut describe_messages: Vec<(DescribeMessageContent, usize, usize)> = Vec::new();
    let mut order = 0;
    for message in messages {
        match message {
            QueryMessage { data, start, end } => 'query: {
                order += 1;
                let cache_key = match is_cache_configured_simple(&data.query, client_state) {
                    Some(cache_key) => cache_key,
                    None => break 'query,
                };
                let cache_command = match get_from_cache(client_state, &cache_key) {
                    Some(data) => CacheCommand::Replay {
                        data,
                        describe_kind: DescribeKind::None,
                        protocol_mode: ProtocolMode::Simple,
                        synthesize_ready_for_query: true,
                    },
                    None => CacheCommand::Capture {
                        key: cache_key,
                        describe_kind: DescribeKind::None,
                        protocol_mode: ProtocolMode::Simple,
                        synthesize_ready_for_query: true,
                    },
                };
                cache_commands.insert(order, cache_command);
                replay_trims.push(ReplayTrim::Simple(ReplayTrimSimple { query: start..end }));
            }
            ParseMessage { data, start, end } => {
                let _ = parse_message(&data, client_state).await;
                parse_messages.push((data, start, end));
            }
            BindMessage { data, start, end } => {
                let _ = bind_message(&data, client_state).await;
                bind_messages.push((data, start, end));
            }
            DescribeMessage { data, start, end } => {
                let _ = describe_message(&data);
                describe_messages.push((data, start, end));
            }
            ExecuteMessage { data, start, end } => 'execute: {
                order += 1;
                if data.rows_to_return_limit > 0 {
                    let cache_key = match is_cache_configured(&data, client_state) {
                        Some(cache_key) => cache_key,
                        None => break 'execute,
                    };
                    let cache_response = get_from_cache(client_state, &cache_key);
                    let describe_kind: DescribeKind;
                    let mut paired_parse_message = None;
                    let mut paired_bind_message = None;
                    let mut paired_describe_message = None;

                    'find_describe: for (index, current_describe_message) in
                        describe_messages.clone().iter().enumerate()
                    {
                        match current_describe_message.0.target {
                            DescribeMessageContentTarget::Portal => {
                                if get_portal_in_session(
                                    &current_describe_message.0.name,
                                    client_state,
                                )
                                .is_some()
                                {
                                    paired_describe_message =
                                        Some(current_describe_message.clone());
                                    describe_messages.remove(index);
                                    break 'find_describe;
                                }
                            }
                            DescribeMessageContentTarget::PreparedStatement => {
                                if get_prepared_statement_in_session(
                                    &current_describe_message.0.name,
                                    client_state,
                                )
                                .is_some()
                                {
                                    paired_describe_message =
                                        Some(current_describe_message.clone());
                                    describe_messages.remove(index);
                                    break 'find_describe;
                                }
                            }
                        }
                    }
                    describe_kind = match &paired_describe_message {
                        Some(data) => describe_message(&data.0),
                        None => DescribeKind::None,
                    };

                    let cache_command = match &cache_response {
                        Some(data) if cache_data_can_satisfy(data, &describe_kind) => {
                            CacheCommand::Replay {
                                data: data.clone(),
                                describe_kind,
                                protocol_mode: ProtocolMode::Extended,
                                synthesize_ready_for_query: false,
                            }
                        }
                        _ => CacheCommand::Capture {
                            key: cache_key,
                            describe_kind,
                            protocol_mode: ProtocolMode::Extended,
                            synthesize_ready_for_query: false,
                        },
                    };

                    if cache_response.is_some() {
                        let portal_name = &data.name;
                        'find_bind: for (index, current_bind_message) in
                            bind_messages.clone().iter().enumerate()
                        {
                            if &current_bind_message.0.portal_name == portal_name {
                                paired_bind_message = Some(current_bind_message.clone());
                                bind_messages.remove(index);
                                'find_parse: for (index, current_parse_message) in
                                    parse_messages.clone().iter().enumerate()
                                {
                                    if current_parse_message.0.prepared_statement_name
                                        == current_bind_message.0.source_prepared_statement_name
                                    {
                                        paired_parse_message = Some(current_parse_message.clone());
                                        parse_messages.remove(index);
                                        break 'find_parse;
                                    }
                                }

                                break 'find_bind;
                            }
                        }
                    }

                    cache_commands.insert(order, cache_command.clone());
                    if let CacheCommand::Replay { .. } = cache_command {
                        replay_trims.push(ReplayTrim::Extended(ReplayTrimExtended {
                            execute: Range { start, end },
                            parse: {
                                if let Some(parse) = paired_parse_message {
                                    Some(parse.1..parse.2)
                                } else {
                                    None
                                }
                            },
                            bind: {
                                if let Some(bind) = paired_bind_message {
                                    Some(bind.1..bind.2)
                                } else {
                                    None
                                }
                            },
                            describe: {
                                if let Some(describe) = paired_describe_message {
                                    Some(describe.1..describe.2)
                                } else {
                                    None
                                }
                            },
                        }));
                    }
                }
            }
            _ => {}
        }
    }
    return (cache_commands, replay_trims);
}

fn get_prepared_statement_in_session<'a>(
    name: &str,
    client_state: &'a mut ClientState,
) -> Option<&'a PreparedStatement> {
    client_state.prepared_statements.get(name)
}

fn get_portal_in_session<'a>(name: &str, client_state: &'a mut ClientState) -> Option<&'a Portal> {
    client_state.portals.get(name)
}

fn cache_data_can_satisfy(response: &CachedResponse, kind: &DescribeKind) -> bool {
    match kind {
        DescribeKind::None => response.has_data(),
        DescribeKind::Portal => response.has_data() && response.has_row_desc(),
        DescribeKind::Statement => {
            response.has_data() && response.has_row_desc() && response.has_param_desc()
        }
    }
}
