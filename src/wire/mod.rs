use std::{io, sync::Arc};

use sqlx::{PgPool, Row, postgres::PgRow};
use tokio::net::{TcpListener, TcpStream};

use types::{SQLCommand, WireProtocolStates};

use messages::{AuthenticationOk, CommandComplete, DataRow, Encode, Query, ReadyForQuery};
use messages::{Decode, RowDescription};

mod messages;
pub mod types;

pub async fn listener_start(listener: TcpListener, postgres_pool: Arc<PgPool>) {
    loop {
        match listener.accept().await {
            Ok((stream, ip)) => {
                let pool = postgres_pool.clone();
                tokio::spawn(async move {
                    println!(
                        "Accepted connection from {:?} on ip {:?}",
                        stream.peer_addr(),
                        ip
                    );
                    handle_connection(stream, &pool).await
                })
            }
            Err(err) => tokio::spawn(async move {
                eprintln!("Failed to accept connection: {}", err);
            }),
        };
    }
}

async fn handle_connection(stream: TcpStream, postgres_pool: &PgPool) {
    let mut state = WireProtocolStates::WaitingForSSL;
    let mut command_tag: SQLCommand;
    loop {
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
                            None => {}
                        };
                    }
                    WireProtocolStates::WaitingForStartup => {
                        let auth_ok = &AuthenticationOk.encode();
                        stream_try_write(&stream, &auth_ok).await;
                        let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                        match stream_try_write(&stream, &ready_for_query).await {
                            Some(_) => state = WireProtocolStates::ReadyForQuery,
                            None => {}
                        }
                    }
                    WireProtocolStates::ReadyForQuery => {
                        let query_string = Query {
                            bytes: read_buffer[..n].to_vec(),
                        }
                        .decode();
                        command_tag = match command_tag_from_query_str(&query_string) {
                            Some(tag) => tag,
                            None => return,
                        };
                        println!("Decoded query: {}", query_string);
                        if let Some(results) = execute_query(&query_string, postgres_pool).await {
                            if let Some(bytes) = create_response_bytes(results, &command_tag) {
                                stream_try_write(&stream, &bytes).await;
                            }
                        }
                        let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                        stream_try_write(&stream, &ready_for_query).await;
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                continue;
            }
            Err(e) => {
                eprintln!("Error occured: {}", e);
                return;
            }
        }
    }
}

async fn execute_query(query_string: &str, pool: &PgPool) -> Option<Vec<PgRow>> {
    let query = sqlx::query(&query_string);
    match query.fetch_all(pool).await {
        Ok(response) => Some(response),
        Err(err) => {
            println!("{}", err);
            None
        }
    }
}

fn create_response_bytes(query_results: Vec<PgRow>, command_tag: &SQLCommand) -> Option<Vec<u8>> {
    let columns = match query_results.first() {
        Some(row) => row.columns(),
        None => return None,
    };
    let mut response: Vec<u8> = Vec::new();
    response.extend(RowDescription { columns: columns }.encode());
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
    let (first, second, third) = (split[0], split[1], split[2]);
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
            if second == "TABLE" && third == "AS" {
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
            Err(e) => {
                eprintln!("Error occurred: {}", e);
                return None;
            }
        }
    }
    Some(written)
}
