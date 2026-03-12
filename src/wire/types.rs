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
