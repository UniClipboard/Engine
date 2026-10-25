mod event;
mod outbound_progress;
mod ports;

pub use event::{
    FileTransferCancellationReason, FileTransferDirection, FileTransferEvent,
    FileTransferFailureReason, FileTransferProgress,
};
pub use outbound_progress::{OutboundProgressReporterPort, OutboundProgressStatus};
pub use ports::{FileTransferEventPublisherPort, FileTransferEventStorePort};

mod receive_settlement;
pub use receive_settlement::{
    ReceiveFileSettlement, ReceiveFileSettlementEffect, ReceiveFileSettlementInput,
};

mod receive_recovery;
pub use receive_recovery::{
    receive_failure_recovery_action, receive_no_entry_confirmation, ReceiveFailureRecoveryAction,
    ReceiveFailureRecoveryRefusal,
};

mod progress_delivery;
pub use progress_delivery::{
    BudgetStatus, ElapsedBudget, ExchangePriority, ExchangeQueue, ExchangeRefusal,
    RetransmitEffect, RetransmitInput, RetransmitStop, TerminalLedger, TerminalRetransmit,
    EXCHANGE_GLOBAL_ACTIVE, EXCHANGE_GLOBAL_QUEUED, EXCHANGE_PEER_QUEUED, TERMINAL_GLOBAL_CAPACITY,
    TERMINAL_PEER_CAPACITY, TERMINAL_RETENTION, TERMINAL_ROUND_LIMIT,
};

mod progress_registration;
pub use progress_registration::{
    ProgressRegistration, RegistrationDescriptor, RegistrationEffect, RegistrationInput,
    RegistrationRefusal,
};

mod progress_terminal;
pub use progress_terminal::{
    valid_progress_plan, ProgressFailureReason, ProgressIdentity, ProgressItem, ProgressItemKind,
    ProgressTerminal, ProgressTerminalEffect, ProgressTerminalOutcome, ProgressTerminalSettlement,
    MAX_PROGRESS_ITEMS,
};

mod progress_snapshot;
pub use progress_snapshot::{
    merge_progress_snapshot, ProgressActive, ProgressItemState, ProgressPhase,
    ProgressSnapshotCoverage, ProgressSnapshotEffect, ProgressSnapshotInput, ProgressSnapshotMerge,
    ProgressSnapshotRefusal, MAX_SNAPSHOT_PAGE_ITEMS,
};

mod progress_projection;
pub use progress_projection::{ProgressItemEffect, ProgressItemInput, ProgressItemProjection};

mod progress_freshness;
pub use progress_freshness::{
    ProgressFreshness, ProgressFreshnessEffect, ProgressFreshnessInput, ProgressFreshnessView,
    PROGRESS_CONTACT_GAP, PROGRESS_UNKNOWN_AFTER,
};

mod progress_capacity;
pub use progress_capacity::{
    ProgressProjectionCapacity, ProjectionCapacityInput, ProjectionRetention,
    PROJECTION_GLOBAL_CAPACITY, PROJECTION_PEER_CAPACITY,
};

mod progress_connections;
pub use progress_connections::{PROGRESS_IDLE_RETENTION, PROGRESS_RETAINED_CONNECTIONS};

mod progress_summary;
pub use progress_summary::{
    summarize_progress, ProgressBatchCounts, ProgressBatchState, ProgressBatchSummary,
};
