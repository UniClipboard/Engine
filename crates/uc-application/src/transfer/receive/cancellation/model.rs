#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelEntryReceiveOutcome {
    CancellationIncomplete,
    Cancelled,
    NotReceiving,
    TooLate,
    AlreadyTerminal,
    Superseded,
}
