use crate::cache::store::cache_key;
use crate::wire::types::StateHandlingResult;
use crate::{
    AppState,
    wire::{
        AuthenticationOk, CommandComplete, DataRow, Decode, Encode, ErrorResponse,
        ErrorResponseSeverity, ProtocolState, Query, ReadyForQuery, RowDescription, SQLCommand,
        WireProtocolStates,
    },
};
use sqlx::{Row, postgres::PgRow};
use std::io;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;

pub(super) async fn waiting_for_ssl(protocol_state: &ProtocolState) -> StateHandlingResult {
    let response = b"N";
    match stream_try_write(&protocol_state.stream, response).await {
        Some(_) => StateHandlingResult::Continue(WireProtocolStates::WaitingForStartup),
        None => StateHandlingResult::Error("failed to write SSL response".to_string()),
    }
}
pub(super) async fn waiting_for_startup(protocol_state: &ProtocolState) -> StateHandlingResult {
    let auth_ok = &AuthenticationOk.encode();
    stream_try_write(&protocol_state.stream, &auth_ok).await;
    let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
    match stream_try_write(&protocol_state.stream, &ready_for_query).await {
        Some(_) => StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery),
        None => StateHandlingResult::Error("failed to write auth ok response".to_string()),
    }
}
pub(super) async fn ready_for_query(
    buffer_length: usize,
    protocol_state: &ProtocolState,
) -> StateHandlingResult {
    let message_type_byte = protocol_state.read_buffer[0];
    if message_type_byte == b'X' {
        return StateHandlingResult::Break("client sent terminate message".to_string());
    }
    match (Query {
        bytes: protocol_state.read_buffer[..buffer_length].to_vec(),
    }
    .decode())
    {
        Ok(query_string) => {
            match command_tag_from_query_str(&query_string) {
                Some(command_tag) => {
                    println!("Decoded query: {}", query_string);
                    match get_from_cache(protocol_state, &query_string) {
                        Some(cached_result) => {
                            stream_try_write(&protocol_state.stream, &cached_result).await;
                        }
                        None => {
                            match execute_query(&query_string, &protocol_state.app_state).await {
                                Ok(results) => {
                                    if let Some(bytes) = create_response_bytes(
                                        results,
                                        &command_tag,
                                        &protocol_state.app_state,
                                    ) {
                                        stream_try_write(&protocol_state.stream, &bytes).await;
                                        set_in_cache(protocol_state, &query_string, bytes);
                                    }
                                }
                                Err(err) => {
                                    stream_try_write(
                                        &protocol_state.stream,
                                        &create_error_message(ErrorResponseSeverity::Error, err),
                                    )
                                    .await;
                                }
                            }
                        }
                    }
                    let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                    stream_try_write(&protocol_state.stream, &ready_for_query).await;
                }
                None => {
                    stream_try_write(
                        &protocol_state.stream,
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
                    let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                    stream_try_write(&protocol_state.stream, &ready_for_query).await;
                }
            };
            StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery)
        }
        Err(err) => {
            stream_try_write(
                &protocol_state.stream,
                &ErrorResponse {
                    severity: ErrorResponseSeverity::Error,
                    error_message: err.to_string(),
                    sql_state_code: "XX000".to_string(),
                }
                .encode(),
            )
            .await;
            let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
            stream_try_write(&protocol_state.stream, &ready_for_query).await;
            StateHandlingResult::Continue(WireProtocolStates::ReadyForQuery)
        }
    }
}

async fn execute_query(
    query_string: &str,
    app_state: &AppState,
) -> Result<Vec<PgRow>, sqlx::Error> {
    let query = sqlx::query(&query_string);
    match query.fetch_all(app_state.pool.as_ref()).await {
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
    app_state: &AppState,
) -> Option<Vec<u8>> {
    let columns = match query_results.first() {
        Some(row) => row.columns(),
        None => return None,
    };
    let mut response: Vec<u8> = Vec::new();
    response.extend(
        RowDescription {
            columns: columns,
            type_lens: &app_state.pg_type_lens,
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

pub(super) async fn stream_try_write(stream: &TcpStream, buf: &[u8]) -> Option<usize> {
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

fn get_from_cache(protocol_state: &ProtocolState, query: &str) -> Option<Vec<u8>> {
    if protocol_state.app_state.matcher.template_exists(query) {
        let key = cache_key(query, &Vec::new());
        return protocol_state.app_state.cache.get(&key);
    }
    None
}

fn set_in_cache(protocol_state: &ProtocolState, query: &str, result: Vec<u8>) {
    if let Some(template) = protocol_state.app_state.matcher.find_template(query) {
        let expiration = match template.ttl {
            Some(ttl) => Instant::now() + Duration::from_secs(ttl),
            None => Instant::now() + Duration::from_secs(protocol_state.app_state.global_ttl),
        };
        let key = cache_key(query, &Vec::new());
        protocol_state
            .app_state
            .cache
            .insert(key, result, expiration);
    }
}
