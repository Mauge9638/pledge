use std::collections::BTreeMap;

use crate::{
    cache::lfu::CachedResponse,
    wire::types::{CacheCommandCapture, DescribeKind},
};

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
    tx: &Sender<(BTreeMap<u16, CacheCommand>, bool)>,
) {
    client_state
        .framer
        .add_buffer(client_state.buffer_state.pending_data());
    while let Ok(Some(msg)) = client_state.framer.next_message() {
        // println!("message: {:?}", msg);
    }

    let mut reader = ByteReader::new(client_state.buffer_state.pending_data().to_vec(), 0);
    if let Ok(messages) = reader.crawl_and_find_messages_client() {
        let (cache_commands, replay_trims, should_hit_db) =
            super::cache_planner::find_cache_related_messages(messages, client_state).await;

        if cache_commands.keys().len() > 0 {
            for (key, command) in cache_commands.iter() {
                println!("cache command: key/order={}", key);
            }
            let _ = tx.send((cache_commands, should_hit_db)).await;
        }
        if replay_trims.len() > 0 {
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            println!("TRIMMING BUFFER");
            let mut writer = ByteWriter::new(&mut client_state.buffer_state, 0);
            println!("length before trimming: {}", writer.get_buffer().len());
            writer.trim_from_pending_commands(replay_trims);
            println!("length after trimming: {}", writer.get_buffer().len());
        } else {
            super::stream_try_write(&db_write, client_state.buffer_state.pending_data()).await;
        }
    }
}

pub(super) async fn handle_db_read(
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
) -> Result<(), String> {
    let db_buffer = db_state.buffer_state.pending_data();
    println!(
        "Buffer isn't empty its this long: {} and contains this data:{:?}",
        db_buffer.len(),
        db_buffer
    );

    super::stream_try_write(&client_write, db_buffer).await;
    db_state.framer.add_buffer(db_buffer);

    loop {
        match db_state.framer.next_message() {
            Ok(option) => match option {
                Some(bytes) => {
                    // println!("framed type byte:{}", bytes[0])
                }
                None => break,
            },
            Err(err) => {
                eprintln!("Something went wrong while framing: {}", err);
                return Err(format!("Something went wrong while framing: {}", err));
            }
        }
    }
    if let Err(err) = db_state.buffer_state.consume(&db_buffer.len()) {
        eprintln!("Error occurred: {}", err);
        return Err(err.to_string());
    }
    Ok(())
}

pub(super) async fn handle_db_cache_command(
    cache_commands: BTreeMap<u16, CacheCommand>,
    should_hit_db: bool,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    db_read: &mut OwnedReadHalf,
) -> Result<(), String> {
    println!("LOOKING AT CACHE COMMANDS IN DB_CACHED HANDLER");

    if cache_commands.len() > 0 {
        // let mut capture_target: Option<(String, String)> = None;
        // let mut has_cache_miss: bool = false;
        for (_, command) in &cache_commands {
            match command {
                CacheCommand::Replay(cmd) => {
                    println!("replay: {:?}", cmd.data);
                    let mut buf = Vec::new();
                    // THE ORDER MATTERS IN THE MATCH BELOW!!
                    match cmd.describe_kind {
                        DescribeKind::Statement => {
                            if let Some(param_desc) = &cmd.data.param_desc
                                && let Some(row_desc) = &cmd.data.row_desc
                            {
                                buf.extend_from_slice(param_desc);
                                buf.extend_from_slice(row_desc);
                            } else {
                                return Err(format!(
                                    "Statement cache replay missing param_desc or row_desc"
                                ));
                            }
                        }
                        DescribeKind::Portal => {
                            if let Some(row_desc) = &cmd.data.row_desc {
                                buf.extend_from_slice(row_desc);
                            } else {
                                return Err(format!("Portal cache replay missing row_desc"));
                            }
                        }
                        _ => {}
                    }
                    buf.extend_from_slice(&cmd.data.data);
                    buf.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
                    super::stream_try_write(client_write, &buf).await;

                    // println!("{:?}", &[b'Z', 0, 0, 0, 5, b'I']);

                    // super::stream_try_write(client_write, &[b'Z', 0, 0, 0, 5, b'I']).await;
                }
                CacheCommand::Capture(cmd) => {
                    println!("capture: {:?}", cmd.key);
                    let _ = handle_db_capture_command(cmd, db_state, client_write, db_read).await;
                    // capture_target = Some((cmd.key.clone(), cmd.query.clone()));
                    // has_cache_miss = true;
                    break;
                }
            }
        }
    }
    Ok(())
}

pub(super) async fn handle_db_capture_command(
    cache_command: &CacheCommandCapture,
    db_state: &mut DBState,
    client_write: &OwnedWriteHalf,
    db_read: &mut OwnedReadHalf,
) -> Result<(), String> {
    if let Err(err) = db_state.buffer_state.read_from_stream(db_read).await {
        eprintln!("Error occurred: {}", err);
        return Err(err.to_string());
    };

    super::stream_try_write(client_write, db_state.buffer_state.pending_data()).await;

    db_state
        .framer
        .add_buffer(db_state.buffer_state.pending_data());

    let mut cached_response = CachedResponse {
        data: Vec::new(),
        param_desc: None,
        row_desc: None,
    };
    let mut success = false;
    loop {
        match db_state.framer.next_message() {
            Ok(option) => match option {
                Some(bytes) => {
                    println!("framed type byte:{}", bytes[0]);
                    match bytes[0] {
                        // ReadyForQuery
                        b'Z' => {
                            println!("{:?}", &bytes);
                            break;
                        }
                        //ParameterDescription
                        b't' => cached_response.param_desc = Some(bytes),
                        //RowDescription
                        b'T' => cached_response.row_desc = Some(bytes),
                        //CommandComplete
                        b'C' => {
                            cached_response.data.extend_from_slice(&bytes);
                            success = true;
                        }
                        _ => {
                            cached_response.data.extend_from_slice(&bytes);
                        }
                    }
                }
                None => break,
            },
            Err(err) => {
                eprintln!("Something went wrong while framing: {}", err);
                return Err(format!("Something went wrong while framing: {}", err));
            }
        }
    }
    if success {
        super::cache_planner::set_in_cache(
            &db_state.app_state,
            cache_command.ttl,
            &cache_command.key,
            cached_response,
        );
    }

    if let Err(err) = db_state
        .buffer_state
        .consume(&db_state.buffer_state.pending_data().len())
    {
        eprintln!("Error occurred: {}", err);
        return Err(err.to_string());
    }
    Ok(())
}
