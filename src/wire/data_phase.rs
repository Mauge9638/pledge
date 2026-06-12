use std::collections::BTreeMap;

use super::{
    reader::ByteReader,
    types::{CacheCommand, ClientState, DBState},
    writer::ByteWriter,
};
use tokio::{
    io::AsyncReadExt,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
    sync::mpsc::Sender,
};

pub(super) async fn handle_client(
    client_state: &mut ClientState,
    db_write: &OwnedWriteHalf,
    buffer_data_length: usize,
    tx: &Sender<(BTreeMap<u16, CacheCommand>, bool)>,
) {
    let mut reader = ByteReader::new(client_state.buffer[..buffer_data_length].to_vec(), 0);
    if let Ok(messages) = reader.crawl_and_find_messages_client() {
        let (cache_commands, replay_trims, should_hit_db) =
            super::cache_planner::find_cache_related_messages(messages, client_state).await;

        if replay_trims.len() > 0 {
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            let mut writer = ByteWriter::new(&mut client_state.buffer, 0);
            writer.trim_from_pending_commands(replay_trims);
        }
        if cache_commands.keys().len() > 0 {
            for (key, command) in cache_commands.iter() {
                println!("cache command: key/order={}", key);
            }
            let _ = tx.send((cache_commands, should_hit_db)).await;
        }
    }
    super::stream_try_write(&db_write, &client_state.buffer[..buffer_data_length]).await;
}

pub(super) async fn handle_db_read(
    result: &Result<usize, std::io::Error>,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    buffer_data_length: &mut usize,
) -> Result<(), String> {
    match result {
        Ok(n) => {
            *buffer_data_length += n;
            loop {
                if *buffer_data_length == 0 {
                    break;
                }
                if *buffer_data_length >= db_state.buffer.len() {
                    println!("Resizing db buffer");
                    db_state.buffer.resize(db_state.buffer.len() * 2, 0);
                } else {
                    break;
                }
            }

            super::stream_try_write(&client_write, &db_state.buffer[..*buffer_data_length]).await;

            db_state
                .framer
                .add_buffer(&db_state.buffer[..*buffer_data_length]);

            loop {
                match db_state.framer.next_message() {
                    Ok(option) => match option {
                        Some(bytes) => {
                            println!("framed type byte:{}", bytes[0])
                        }
                        None => break,
                    },
                    Err(err) => {
                        eprintln!("Something went wrong while framing: {}", err);
                        return Err(format!("Something went wrong while framing: {}", err));
                    }
                }
            }
            Ok(())
        }
        Err(err) => {
            eprintln!("Failed to read from client: {}", err);
            Err(format!("Failed to read from client: {}", err))
        }
    }
}

pub(super) async fn handle_db_cache_command(
    cache_commands: BTreeMap<u16, CacheCommand>,
    should_hit_db: bool,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    db_read: &mut OwnedReadHalf,
    buffer_data_length: &mut usize,
) -> Result<(), String> {
    println!("LOOKING AT CACHE COMMANDS IN DB_CACHED HANDLER");

    if cache_commands.len() > 0 {
        let mut has_cache_miss: bool = false;
        'cache_commands_check: for (_, command) in &cache_commands {
            match command {
                CacheCommand::Replay { data, .. } => {
                    println!("replay: {:?}", data);
                }
                CacheCommand::Capture { key, .. } => {
                    println!("capture: {:?}", key);
                    has_cache_miss = true;
                    break 'cache_commands_check;
                }
            }
        }
        if has_cache_miss || should_hit_db {
            match db_read
                .read(&mut db_state.buffer[*buffer_data_length..])
                .await
            {
                Ok(n) => {
                    *buffer_data_length += n;
                    'inner_loop: loop {
                        if *buffer_data_length == 0 {
                            break 'inner_loop;
                        }
                        if *buffer_data_length >= db_state.buffer.len() {
                            println!("Resizing db buffer");
                            db_state.buffer.resize(db_state.buffer.len() * 2, 0);
                        } else {
                            break 'inner_loop;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Error occurred: {}", err);
                    return Err(err.to_string());
                }
            }
        }
        let mut writer = ByteWriter::new(&mut db_state.buffer, 0);
        //writer.merge_cache_commands(&cache_commands);
        super::stream_try_write(client_write, &db_state.buffer[..*buffer_data_length]).await;
    }
    // This should never happen, BUT as a precaution we handle it by sending the DB response to the client
    else {
        let result = db_read
            .read(&mut db_state.buffer[*buffer_data_length..])
            .await;
        handle_db_read(&result, db_state, client_write, buffer_data_length).await?;
    }
    Ok(())
}
