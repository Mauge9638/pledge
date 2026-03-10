pub(super) enum WireProtocolStates {
    WaitingForSSL,
    WaitingForStartup,
    ReadyForQuery,
}
