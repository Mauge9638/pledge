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

use crate::AppState;
use crate::wire::messages::ErrorResponseSeverity;
use crate::wire::state_handling::{ready_for_query, waiting_for_ssl, waiting_for_startup};

mod messages;
mod state_handling;
pub mod types;

pub(super) type ReadBuffer = [u8; 10024];

pub async fn listener_start(listener: TcpListener, app_state: &AppState) {
    loop {
        match listener.accept().await {
            Ok((stream, ip)) => {
                let state = app_state.clone();
                tokio::spawn(async move {
                    println!(
                        "Accepted connection from {:?} on ip {:?}",
                        stream.peer_addr(),
                        ip
                    );
                    handle_connection(stream, &state).await
                })
            }
            Err(err) => tokio::spawn(async move {
                eprintln!("Failed to accept connection: {}", err);
            }),
        };
    }
}

async fn handle_connection(stream: TcpStream, app_state: &AppState) {
    let mut state = WireProtocolStates::WaitingForSSL;
    'mainloop: loop {
        let _ = stream.readable().await;
        let mut read_buffer: ReadBuffer = [0u8; 10024];

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
                        match waiting_for_ssl(&stream).await {
                            Ok(new_state) => state = new_state,
                            Err(_) => break 'mainloop,
                        };
                    }
                    WireProtocolStates::WaitingForStartup => {
                        match waiting_for_startup(&stream).await {
                            Ok(new_state) => state = new_state,
                            Err(_) => break 'mainloop,
                        }
                    }
                    WireProtocolStates::ReadyForQuery => {
                        match ready_for_query(read_buffer, n, &stream, app_state).await {
                            Ok(_) => {}
                            Err(_) => {
                                break 'mainloop;
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
                state_handling::stream_try_write(
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

pub async fn get_pg_type_lens(postgres_pool: &PgPool) -> Result<HashMap<u32, i16>, Error> {
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
