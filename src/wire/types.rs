use std::collections::HashMap;

use sqlx::postgres::{PgColumn, PgStatement};
use tokio::net::{
    TcpStream,
    tcp::{OwnedReadHalf, OwnedWriteHalf},
};

use crate::AppState;

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
}

pub(super) struct DBState {
    pub app_state: AppState,
    pub buffer: Vec<u8>,
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

pub(super) enum CacheCommand {
    Replay(Vec<u8>, CacheCommandMetadata), // cache hit: write these to client, skip DB
    Capture(String, CacheCommandMetadata), // cache miss: next DB response belongs to this key
}

pub(super) enum SectionType {
    ExtendedQuery,
    SimpleQuery,
}

pub(super) struct CacheCommandMetadata {
    pub section_type: SectionType,
    pub length: usize,
    pub message_number: i32,
}
