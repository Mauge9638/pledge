use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    ops::Range,
    sync::Arc,
    time::Duration,
};

use super::MessageFramer;
use crate::{AppState, cache::lfu::CachedResponse};
use tokio::{
    io::AsyncReadExt,
    net::tcp::{OwnedReadHalf, OwnedWriteHalf},
};
use uuid::serde::compact;

#[derive(Debug)]
pub(super) enum WireProtocolStates {
    WaitingForSSL,
    WaitingForStartup,
    ReadyForQuery,
}

pub(super) enum SQLCommand {
    Insert,
    Delete,
    Update,
    Merge,
    Select,
    CreateTableAs,
    Move,
    Fetch,
    Copy,
}

pub(super) struct ProtocolState {
    pub app_state: AppState,
    pub client_state: WireProtocolStates,
    pub db_state: WireProtocolStates,
    pub client_buffer: Vec<u8>,
    pub db_buffer: Vec<u8>,
    pub prepared_statements: HashMap<String, PreparedStatement>,
    pub portals: HashMap<String, Portal>,
}

pub(super) struct ClientState {
    pub app_state: AppState,
    pub prepared_statements: HashMap<String, PreparedStatement>,
    pub portals: HashMap<String, Portal>,
    pub framer: MessageFramer,
    pub buffer_state: BufferState,
}

pub(super) struct DBState {
    pub app_state: AppState,
    pub framer: MessageFramer,
    pub buffer_state: BufferState,
}

pub(super) struct BufferState {
    pub buffer: Vec<u8>,
    pub read_cursor: usize,
    pub write_cursor: usize,
}

impl BufferState {
    pub async fn read_from_stream(&mut self, stream: &mut OwnedReadHalf) -> Result<usize, Error> {
        let free_space = self.buffer.len() - self.write_cursor;
        if free_space < self.buffer.len() / 4 {
            self.compact();
        }
        if self.write_cursor >= self.buffer.len() {
            self.buffer.resize(self.buffer.len() * 2, 0);
        }
        let read = stream.read(&mut self.buffer[self.write_cursor..]).await?;
        self.write_cursor += read;
        Ok(read)
    }
    pub fn compact(&mut self) {
        if self.read_cursor > 0 {
            self.buffer
                .copy_within(self.read_cursor..self.write_cursor, 0);
            self.write_cursor -= self.read_cursor;
            self.read_cursor = 0;
        }
    }
    pub fn pending_data(&self) -> &[u8] {
        &self.buffer[self.read_cursor..self.write_cursor]
    }
    pub fn consume(&mut self, n: &usize) -> Result<(), Error> {
        match self.read_cursor + n <= self.write_cursor {
            true => {
                self.read_cursor += n;
                Ok(())
            }
            false => Err(Error::new(
                ErrorKind::InvalidData,
                "consume: n is larger than pending data",
            )),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.read_cursor == self.write_cursor
    }
}

pub(super) enum StateHandlingResult {
    Continue(WireProtocolStates),
    Break(String),
    Error(String),
}

pub(super) struct PreparedStatement {
    pub query: String,
    pub parameter_data_types: Vec<i32>,
}

#[derive(Clone)]
pub(super) struct ColumnMetadata {
    pub name: String,
    pub table_oid: i32,
    pub attribute_number: i16,
    pub type_oid: i32,
    pub type_len: i16,
}

pub(super) struct Portal {
    pub source_prepared_statement_name: String,
    pub parameter_format_codes: Vec<i16>,
    pub parameter_values: Vec<Option<Vec<u8>>>,
    pub result_column_format_codes: Vec<i16>,
}

#[derive(Clone)]
pub(super) enum ProtocolMode {
    Simple,
    Extended,
}

#[derive(Clone)]
pub(super) enum CacheCommand {
    Replay(CacheCommandReplay),   // cache hit: write these to client, skip DB
    Capture(CacheCommandCapture), // cache miss: next DB response belongs to this key
}

#[derive(Clone)]
pub(super) struct CacheCommandReplay {
    pub key: String,
    pub data: Arc<CachedResponse>,
    pub describe_kind: DescribeKind,
    pub protocol_mode: ProtocolMode,
    pub query: String,
}

#[derive(Clone)]
pub(super) struct CacheCommandCapture {
    pub key: String,
    pub describe_kind: DescribeKind,
    pub protocol_mode: ProtocolMode,
    pub query: String,
    pub ttl: Duration,
}

#[derive(Clone)]
pub(super) enum DescribeKind {
    None,
    Portal,
    Statement,
}

pub(super) enum ReplayTrim {
    Extended(ReplayTrimExtended),
    Simple(ReplayTrimSimple),
}

pub(super) struct ReplayTrimExtended {
    pub execute: Range<usize>,
    pub parse: Option<Range<usize>>,
    pub bind: Option<Range<usize>>,
    pub describe: Option<Range<usize>>,
    pub sync: Option<Range<usize>>,
}

impl ReplayTrimExtended {
    pub fn new() -> Self {
        Self {
            execute: 0..0,
            parse: None,
            bind: None,
            describe: None,
            sync: None,
        }
    }
}

pub(super) struct ReplayTrimSimple {
    pub query: Range<usize>,
}

pub(super) struct CachePlan {
    pub key: String,
    pub ttl: Duration,
}
