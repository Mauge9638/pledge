use std::collections::HashMap;
use std::{io, sync::Arc};

use sqlx::error::{DatabaseError, Error};
use sqlx::postgres::types::Oid;
use sqlx::{PgPool, Row, postgres::PgRow};
use tokio::net::{TcpListener, TcpStream};

use types::{SQLCommand, WireProtocolStates};

use messages::{
    AuthenticationOk, CommandComplete, DataRow, Encode, ErrorResponse, Query, ReadyForQuery,
};
use messages::{Decode, RowDescription};

use crate::wire::messages::ErrorResponseSeverity;

mod messages;
pub mod types;

pub async fn listener_start(listener: TcpListener, postgres_pool: Arc<PgPool>) {
    if let Ok(types) = get_pg_type_lens(&postgres_pool).await {
        let pg_type_lens = Arc::new(types);
        loop {
            match listener.accept().await {
                Ok((stream, ip)) => {
                    let pool = postgres_pool.clone();
                    let type_lens = pg_type_lens.clone();
                    tokio::spawn(async move {
                        println!(
                            "Accepted connection from {:?} on ip {:?}",
                            stream.peer_addr(),
                            ip
                        );
                        handle_connection(stream, &pool, &type_lens).await
                    })
                }
                Err(err) => tokio::spawn(async move {
                    eprintln!("Failed to accept connection: {}", err);
                }),
            };
        }
    } else {
        eprintln!("Failed to get pg_type.typlen");
    }
}

async fn handle_connection(
    stream: TcpStream,
    postgres_pool: &PgPool,
    pg_type_lens: &HashMap<u32, i16>,
) {
    let mut state = WireProtocolStates::WaitingForSSL;
    'mainloop: loop {
        let _ = stream.readable().await;
        let mut read_buffer = [0u8; 10024];

        match stream.try_read(&mut read_buffer) {
            Ok(0) => break,
            Ok(n) => {
                println!(
                    "----------------------\nRECEIVED (STATE: {:?})\nbyte length: {}\nraw content {:?}\n----------------------\n",
                    state,
                    n,
                    &read_buffer[..n]
                );

                match state {
                    WireProtocolStates::WaitingForSSL => {
                        let response = b"N";
                        match stream_try_write(&stream, response).await {
                            Some(_) => state = WireProtocolStates::WaitingForStartup,
                            None => {
                                break 'mainloop;
                            }
                        };
                    }
                    WireProtocolStates::WaitingForStartup => {
                        let auth_ok = &AuthenticationOk.encode();
                        stream_try_write(&stream, &auth_ok).await;
                        let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                        match stream_try_write(&stream, &ready_for_query).await {
                            Some(_) => state = WireProtocolStates::ReadyForQuery,
                            None => {
                                break 'mainloop;
                            }
                        }
                    }
                    WireProtocolStates::ReadyForQuery => {
                        let message_type_byte = read_buffer[0];
                        if message_type_byte == b'X' {
                            break 'mainloop;
                        }
                        match (Query {
                            bytes: read_buffer[..n].to_vec(),
                        }
                        .decode())
                        {
                            Ok(query_string) => {
                                match command_tag_from_query_str(&query_string) {
                                    Some(command_tag) => {
                                        println!("Decoded query: {}", query_string);
                                        match execute_query(&query_string, postgres_pool).await {
                                            Ok(results) => {
                                                if let Some(bytes) = create_response_bytes(
                                                    results,
                                                    &command_tag,
                                                    pg_type_lens,
                                                ) {
                                                    stream_try_write(&stream, &bytes).await;
                                                }
                                            }
                                            Err(err) => {
                                                stream_try_write(
                                                    &stream,
                                                    &create_error_message(
                                                        ErrorResponseSeverity::Error,
                                                        err,
                                                    ),
                                                )
                                                .await;
                                            }
                                        }
                                        let ready_for_query =
                                            &ReadyForQuery { status: b'I' }.encode();
                                        stream_try_write(&stream, &ready_for_query).await;
                                    }
                                    None => {
                                        stream_try_write(
                                            &stream,
                                            &ErrorResponse {
                                                severity: ErrorResponseSeverity::Error,
                                                error_message:
                                                    "error while creating command tag from query, is your query malformed or incomplete?"
                                                        .to_string(),
                                                sql_state_code: "XX000".to_string(),
                                            }
                                            .encode(),
                                        )
                                        .await;
                                        let ready_for_query =
                                            &ReadyForQuery { status: b'I' }.encode();
                                        stream_try_write(&stream, &ready_for_query).await;
                                    }
                                };
                            }
                            Err(err) => {
                                stream_try_write(
                                    &stream,
                                    &ErrorResponse {
                                        severity: ErrorResponseSeverity::Error,
                                        error_message: err.to_string(),
                                        sql_state_code: "XX000".to_string(),
                                    }
                                    .encode(),
                                )
                                .await;
                                let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                                stream_try_write(&stream, &ready_for_query).await;
                            }
                        }
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(err) => {
                eprintln!("Error occured: {}", err);
                stream_try_write(
                    &stream,
                    &ErrorResponse {
                        severity: ErrorResponseSeverity::Fatal,
                        error_message: err.to_string(),
                        sql_state_code: "XX000".to_string(),
                    }
                    .encode(),
                )
                .await;
                break 'mainloop;
            }
        }
    }
}

async fn execute_query(query_string: &str, pool: &PgPool) -> Result<Vec<PgRow>, sqlx::Error> {
    let query = sqlx::query(&query_string);
    match query.fetch_all(pool).await {
        Ok(response) => Ok(response),
        Err(err) => {
            eprintln!("Error occurred: {}", err);
            Err(err)
        }
    }
}

fn create_response_bytes(
    query_results: Vec<PgRow>,
    command_tag: &SQLCommand,
    pg_type_lens: &HashMap<u32, i16>,
) -> Option<Vec<u8>> {
    let columns = match query_results.first() {
        Some(row) => row.columns(),
        None => return None,
    };
    let mut response: Vec<u8> = Vec::new();
    response.extend(
        RowDescription {
            columns: columns,
            type_lens: pg_type_lens,
        }
        .encode(),
    );
    for row in &query_results {
        response.extend(DataRow { row: row }.encode());
    }
    response.extend(
        CommandComplete {
            rows: query_results.len() as u16,
            command_tag: command_tag,
        }
        .encode(),
    );

    println!("----------------------\nCOMMAND COMPLETE\n----------------------");
    Some(response)
}

fn command_tag_from_query_str(query: &str) -> Option<SQLCommand> {
    let split = query.split(" ").collect::<Vec<&str>>();
    let first = split[0];
    match first {
        "SELECT" => Some(SQLCommand::Select),
        "INSERT" => Some(SQLCommand::Insert),
        "UPDATE" => Some(SQLCommand::Update),
        "DELETE" => Some(SQLCommand::Delete),
        "MERGE" => Some(SQLCommand::Merge),
        "MOVE" => Some(SQLCommand::Move),
        "FETCH" => Some(SQLCommand::Fetch),
        "COPY" => Some(SQLCommand::Copy),
        "CREATE" => {
            let (second, third) = (split.get(1), split.get(2));
            if second.unwrap_or(&&"") == &"TABLE" && third.unwrap_or(&&"") == &"AS" {
                Some(SQLCommand::CreateTableAs)
            } else {
                None
            }
        }
        _ => None,
    }
}

async fn stream_try_write(stream: &TcpStream, buf: &[u8]) -> Option<usize> {
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

fn create_error_message(severity: ErrorResponseSeverity, err: sqlx::Error) -> Vec<u8> {
    let error_message: String;
    let sql_state_code: String;
    match err.as_database_error() {
        Some(err) => {
            error_message = err.message().to_string();
            match err.code() {
                Some(code) => sql_state_code = code.to_string(),
                None => sql_state_code = "XX000".to_string(), // Pledge undefined error
            }
        }
        None => {
            error_message = "Unknown error".to_string();
            sql_state_code = "XX000".to_string(); // Pledge undefined error
        }
    }

    let error = ErrorResponse {
        severity: severity,
        error_message: error_message,
        sql_state_code: sql_state_code,
    }
    .encode();

    error
}

async fn get_pg_type_lens(postgres_pool: &PgPool) -> Result<HashMap<u32, i16>, Error> {
    let mut type_lens_hashmap: HashMap<u32, i16> = HashMap::new();
    let query = sqlx::query("SELECT oid, typlen FROM pg_type");
    match query.fetch_all(postgres_pool).await {
        Ok(response) => {
            for row in response {
                type_lens_hashmap.insert(row.get::<Oid, _>(0).0, row.get(1));
            }
        }
        Err(err) => {
            eprintln!("Error occurred: {}", err);
            return Err(err);
        }
    };
    Ok(type_lens_hashmap)
}
