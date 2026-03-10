use std::{io, sync::Arc};

use sqlx::{PgPool, Row, postgres::PgRow};
use tokio::net::{TcpListener, TcpStream};

use types::WireProtocolStates;

use messages::{AuthenticationOk, CommandComplete, DataRow, Encode, Query, ReadyForQuery};

use crate::wire::messages::{Decode, RowDescription};

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
    loop {
        let _ = stream.readable().await;
        let mut buffer = [0u8; 1024];

        match stream.try_read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                println!(
                    "-- RECEIVED -- byte length: {}, with raw content {:?}",
                    n,
                    &buffer[..n]
                );

                match state {
                    WireProtocolStates::WaitingForSSL => {
                        let response = b"N";
                        match stream.try_write(response) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!(
                                    "-- SENT -- byte length: {}, bytes sent: {:?}",
                                    n,
                                    &response[..n]
                                );
                                state = WireProtocolStates::WaitingForStartup;
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
                    WireProtocolStates::WaitingForStartup => {
                        let auth_ok = &AuthenticationOk.encode();
                        match stream.try_write(auth_ok) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!(
                                    "-- SENT -- byte length: {}, bytes sent: {:?}",
                                    n,
                                    &auth_ok[..n]
                                );
                            }
                            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                                continue;
                            }
                            Err(e) => {
                                eprintln!("Error occured: {}", e);
                                return;
                            }
                        }
                        let ready_for_query = &ReadyForQuery { status: b'I' }.encode();
                        match stream.try_write(ready_for_query) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!(
                                    "-- SENT -- byte length: {}, bytes sent: {:?}",
                                    n,
                                    &ready_for_query[..n]
                                );
                                state = WireProtocolStates::ReadyForQuery;
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
                    WireProtocolStates::ReadyForQuery => {
                        let query_string = Query {
                            bytes: buffer[..n].to_vec(),
                        }
                        .decode();
                        println!("Decoded query: {}", query_string);
                        match execute_query(&query_string, postgres_pool).await {
                            Some(results) => {
                                let _ = stream.writable().await;
                                respond_to_query(&stream, results).await
                            }
                            None => {}
                        }
                        let response = &ReadyForQuery { status: b'I' }.encode();
                        match stream.try_write(response) {
                            Ok(0) => break,
                            Ok(n) => {
                                println!(
                                    "-- SENT -- byte length: {}, bytes sent: {:?}",
                                    n,
                                    &response[..n]
                                );
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

async fn respond_to_query(stream: &TcpStream, query_results: Vec<PgRow>) {
    let columns = match query_results.first() {
        Some(row) => row.columns(),
        None => return,
    };
    let row_description = RowDescription { columns: columns }.encode();
    println!("response: {:?}", query_results);
    match stream.try_write(&row_description) {
        Ok(n) => {
            println!(
                "-- SENT -- byte length: {}, bytes sent: {:?}",
                n,
                &row_description[..n]
            );
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            return;
        }
        Err(e) => {
            eprintln!("Error occured: {}", e);
            return;
        }
    }
    for row in &query_results {
        let data_row = DataRow { row: row }.encode();
        match stream.try_write(&data_row) {
            Ok(n) => {
                println!(
                    "-- SENT -- byte length: {}, bytes sent: {:?}",
                    n,
                    &data_row[..n]
                );
            }
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                return;
            }
            Err(e) => {
                eprintln!("Error occured: {}", e);
                return;
            }
        }
    }
    let command_complete = CommandComplete {
        rows: query_results.len() as u16,
    };
    match stream.try_write(&command_complete.encode()) {
        Ok(n) => {
            println!(
                "-- SENT -- byte length: {}, bytes sent: {:?}",
                n,
                &command_complete.encode()[..n]
            );
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
            return;
        }
        Err(e) => {
            eprintln!("Error occured: {}", e);
            return;
        }
    }
}
