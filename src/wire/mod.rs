use std::collections::HashMap;

use tokio::{
    io::AsyncReadExt,
    net::tcp::OwnedWriteHalf,
    net::{TcpListener, TcpStream},
    sync::mpsc,
};

use types::{SQLCommand, WireProtocolStates};

use messages::Decode;

use crate::AppState;
use crate::config::DatabaseConfig;
use reader::ByteReader;
use types::{CacheCommand, ClientState, DBState, ReplayTrim};

use message_framer::MessageFramer;

mod cache_planner;
mod data_phase;
mod message_framer;
mod messages;
mod reader;
mod startup_phase;
pub mod types;
mod writer;

pub async fn listener_start(listener: TcpListener, app_state: &AppState) {
    loop {
        match listener.accept().await {
            Ok((client_stream, ip)) => {
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
                        client_stream.peer_addr(),
                        ip
                    );
                    spawn_tasks(client_stream, db_stream, &state).await
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
        framer: MessageFramer::new(),
    };
    let mut db_state = DBState {
        app_state: app_state.clone(),
        buffer: vec![0u8; 32 * 1024],
        framer: MessageFramer::new(),
    };

    startup_phase::startup_state_handling(
        &mut client_state,
        &mut db_state,
        &mut client_read,
        &client_write,
        &mut db_read,
        &db_write,
    )
    .await;

    let client_spawn = tokio::spawn(async move {
        println!("Accepted connection from {:?} ", client_read.peer_addr(),);
        let mut buffer_data_length;
        'outer_loop: loop {
            buffer_data_length = 0;
            'inner_loop: loop {
                buffer_data_length += match client_read
                    .read(&mut client_state.buffer[buffer_data_length..])
                    .await
                {
                    Ok(n) => n,
                    Err(err) => {
                        eprintln!("Failed to read from client: {}", err);
                        break 'outer_loop;
                    }
                };
                if buffer_data_length == 0 {
                    break 'inner_loop;
                }
                if buffer_data_length >= client_state.buffer.len() {
                    println!("Resizing client buffer");
                    client_state.buffer.resize(client_state.buffer.len() * 2, 0);
                } else {
                    break 'inner_loop;
                }
            }
            data_phase::handle_client(&mut client_state, &db_write, buffer_data_length, &tx).await;
        }
    });
    let db_spawn = tokio::spawn(async move {
        let mut buffer_data_length;
        'outer_loop: loop {
            buffer_data_length = 0;

            tokio::select! {
                biased; Some((cache_commands, should_hit_db)) = rx.recv() => {
                    if let Err(e) = data_phase::handle_db_cache_command(cache_commands, should_hit_db, &mut db_state, &client_write, &mut db_read, &mut buffer_data_length).await {
                        eprintln!("cache handling failed: {e}");
                        break 'outer_loop;
                    }
                }
                result = db_read.read(&mut db_state.buffer[buffer_data_length..]) => {
                    if let Err(e) = data_phase::handle_db_read(&result, &mut db_state, &client_write, &mut buffer_data_length).await {
                        eprintln!("db read failed: {e}");
                        break 'outer_loop;
                    }
                }
            }
        }
    });

    let handles = tokio::join!(client_spawn, db_spawn);
}

pub(super) async fn stream_try_write(stream: &OwnedWriteHalf, buf: &[u8]) -> Option<usize> {
    let mut written = 0;
    while written < buf.len() {
        stream.writable().await.ok()?;
        match stream.try_write(&buf[written..]) {
            Ok(n) => written += n,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(err) => {
                eprintln!("Error occurred: {}", err);
                return None;
            }
        }
    }
    Some(written)
}
