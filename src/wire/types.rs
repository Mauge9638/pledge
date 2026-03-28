use tokio::net::TcpStream;

use crate::{AppState, wire::ReadBuffer};

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
    pub stream: TcpStream,
    pub app_state: AppState,
    pub state: WireProtocolStates,
    pub read_buffer: ReadBuffer,
}

pub(super) enum StateHandlingResult {
    Continue(WireProtocolStates),
    Break(String),
    Error(String),
}
