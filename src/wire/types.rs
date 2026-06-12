use std::{collections::HashMap, ops::Range, sync::Arc};

use super::MessageFramer;
use crate::{AppState, cache::lfu::CachedResponse};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};

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
    pub buffer: Vec<u8>,
    pub prepared_statements: HashMap<String, PreparedStatement>,
    pub portals: HashMap<String, Portal>,
    pub framer: MessageFramer,
}

pub(super) struct DBState {
    pub app_state: AppState,
    pub buffer: Vec<u8>,
    pub framer: MessageFramer,
}

pub(super) struct Streams {
    pub client_read: OwnedReadHalf,
    pub client_write: OwnedWriteHalf,
    pub db_read: OwnedReadHalf,
    pub db_write: OwnedWriteHalf,
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
    Replay {
        data: Arc<CachedResponse>,
        describe_kind: DescribeKind,
        protocol_mode: ProtocolMode,
    }, // cache hit: write these to client, skip DB
    Capture {
        key: String,
        describe_kind: DescribeKind,
        protocol_mode: ProtocolMode,
    }, // cache miss: next DB response belongs to this key
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
}

impl ReplayTrimExtended {
    pub fn new() -> Self {
        Self {
            execute: 0..0,
            parse: None,
            bind: None,
            describe: None,
        }
    }
}

pub(super) struct ReplayTrimSimple {
    pub query: Range<usize>,
}
