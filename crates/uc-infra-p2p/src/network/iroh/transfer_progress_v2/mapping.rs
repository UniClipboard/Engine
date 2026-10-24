use uc_application::deps::{ProgressBound, ProgressTaskDescriptor, ProgressTerminalNotice};
use uc_core::file_transfer::{
    FileTransferCancellationReason as Cancel, ProgressFailureReason as Failure, ProgressIdentity,
    ProgressItem, ProgressItemKind, ProgressTerminal, ProgressTerminalOutcome as Outcome,
};
use uc_sync_protocol::transfer_progress::v2::{
    CancelReason, FailureReason, ItemKind, LogicalItem, Message, ProducerDescriptor, TaskScope,
    TerminalOutcome, TerminalResult, WireId,
};

pub(super) fn identity(id: WireId) -> ProgressIdentity {
    ProgressIdentity::new(id.0)
}
pub(super) fn descriptor(value: ProducerDescriptor) -> ProgressTaskDescriptor {
    ProgressTaskDescriptor {
        producer: identity(value.producer),
        revision: value.owner_revision,
        source: identity(value.scope.source_entry),
        attempt: identity(value.scope.attempt),
        item_count: value.item_count,
    }
}
pub(super) fn wire_descriptor(value: ProgressTaskDescriptor) -> ProducerDescriptor {
    ProducerDescriptor {
        producer: WireId(value.producer.bytes()),
        owner_revision: value.revision,
        scope: TaskScope {
            source_entry: WireId(value.source.bytes()),
            attempt: WireId(value.attempt.bytes()),
        },
        item_count: value.item_count,
    }
}
pub(super) fn bound(value: ProgressBound) -> Message {
    Message::Bound {
        challenge: WireId(value.challenge.bytes()),
        binding: WireId(value.binding.bytes()),
        descriptor: wire_descriptor(value.descriptor),
    }
}
pub(super) fn terminal(
    binding: WireId,
    scope: TaskScope,
    revision: u64,
    value: TerminalResult,
) -> ProgressTerminalNotice {
    ProgressTerminalNotice {
        binding: identity(binding),
        source: identity(scope.source_entry),
        attempt: identity(scope.attempt),
        revision,
        result: terminal_result(value),
    }
}
pub(super) fn ack(value: ProgressTerminalNotice) -> Message {
    let (binding, scope, owner_revision, result) = wire_terminal(value);
    Message::TerminalAck {
        binding,
        scope,
        owner_revision,
        result,
    }
}
pub(super) fn outgoing_terminal(value: ProgressTerminalNotice) -> Message {
    let (binding, scope, owner_revision, result) = wire_terminal(value);
    Message::Terminal {
        binding,
        scope,
        owner_revision,
        result,
    }
}
fn wire_terminal(value: ProgressTerminalNotice) -> (WireId, TaskScope, u64, TerminalResult) {
    (
        WireId(value.binding.bytes()),
        TaskScope {
            source_entry: WireId(value.source.bytes()),
            attempt: WireId(value.attempt.bytes()),
        },
        value.revision,
        TerminalResult {
            item: LogicalItem {
                kind: match value.result.item.kind {
                    ProgressItemKind::File => ItemKind::File,
                    ProgressItemKind::Representation => ItemKind::Representation,
                    ProgressItemKind::DirectoryUnit => ItemKind::DirectoryUnit,
                },
                index: value.result.item.index,
            },
            round: value.result.round,
            sequence: value.result.sequence,
            result_revision: value.result.result_revision,
            bytes: value.result.bytes,
            total: value.result.total,
            outcome: match value.result.outcome {
                Outcome::Completed => TerminalOutcome::Completed,
                Outcome::Failed(reason) => TerminalOutcome::Failed(match reason {
                    Failure::NetworkUnavailable => FailureReason::NetworkUnavailable,
                    Failure::TimedOut => FailureReason::TimedOut,
                    Failure::AccessDenied => FailureReason::AccessDenied,
                    Failure::StorageUnavailable => FailureReason::StorageUnavailable,
                    Failure::IntegrityCheckFailed => FailureReason::IntegrityCheckFailed,
                    Failure::Interrupted => FailureReason::Interrupted,
                    Failure::Unknown => FailureReason::Unknown,
                }),
                Outcome::Cancelled(reason) => TerminalOutcome::Cancelled(match reason {
                    Cancel::LocalUser => CancelReason::LocalUser,
                    Cancel::RemotePeer => CancelReason::RemotePeer,
                    Cancel::Replaced => CancelReason::Replaced,
                    Cancel::Timeout => CancelReason::Timeout,
                    Cancel::ConnectivityRecovery => CancelReason::ConnectivityRecovery,
                    Cancel::Unknown => CancelReason::Unknown,
                }),
            },
        },
    )
}

pub(super) fn terminal_result(value: TerminalResult) -> ProgressTerminal {
    ProgressTerminal {
        item: item(value.item),
        round: value.round,
        sequence: value.sequence,
        result_revision: value.result_revision,
        bytes: value.bytes,
        total: value.total,
        outcome: match value.outcome {
            TerminalOutcome::Completed => Outcome::Completed,
            TerminalOutcome::Failed(reason) => Outcome::Failed(match reason {
                FailureReason::NetworkUnavailable => Failure::NetworkUnavailable,
                FailureReason::TimedOut => Failure::TimedOut,
                FailureReason::AccessDenied => Failure::AccessDenied,
                FailureReason::StorageUnavailable => Failure::StorageUnavailable,
                FailureReason::IntegrityCheckFailed => Failure::IntegrityCheckFailed,
                FailureReason::Interrupted => Failure::Interrupted,
                FailureReason::Unknown => Failure::Unknown,
            }),
            TerminalOutcome::Cancelled(reason) => Outcome::Cancelled(match reason {
                CancelReason::LocalUser => Cancel::LocalUser,
                CancelReason::RemotePeer => Cancel::RemotePeer,
                CancelReason::Replaced => Cancel::Replaced,
                CancelReason::Timeout => Cancel::Timeout,
                CancelReason::ConnectivityRecovery => Cancel::ConnectivityRecovery,
                CancelReason::Unknown => Cancel::Unknown,
            }),
        },
    }
}

pub(super) fn item(value: LogicalItem) -> ProgressItem {
    ProgressItem {
        kind: match value.kind {
            ItemKind::File => ProgressItemKind::File,
            ItemKind::Representation => ProgressItemKind::Representation,
            ItemKind::DirectoryUnit => ProgressItemKind::DirectoryUnit,
        },
        index: value.index,
    }
}

#[cfg(test)]
mod tests {
    use super::{ack, terminal};
    use uc_sync_protocol::transfer_progress::v2::{
        CancelReason, FailureReason, ItemKind, LogicalItem, Message, TaskScope, TerminalOutcome,
        TerminalResult, WireId,
    };
    #[test]
    fn exact_terminal_ack_preserves_all_causes_and_identity_fields() {
        let outcomes = [
            TerminalOutcome::Completed,
            TerminalOutcome::Failed(FailureReason::NetworkUnavailable),
            TerminalOutcome::Failed(FailureReason::TimedOut),
            TerminalOutcome::Failed(FailureReason::AccessDenied),
            TerminalOutcome::Failed(FailureReason::StorageUnavailable),
            TerminalOutcome::Failed(FailureReason::IntegrityCheckFailed),
            TerminalOutcome::Failed(FailureReason::Interrupted),
            TerminalOutcome::Failed(FailureReason::Unknown),
            TerminalOutcome::Cancelled(CancelReason::LocalUser),
            TerminalOutcome::Cancelled(CancelReason::RemotePeer),
            TerminalOutcome::Cancelled(CancelReason::Replaced),
            TerminalOutcome::Cancelled(CancelReason::Timeout),
            TerminalOutcome::Cancelled(CancelReason::ConnectivityRecovery),
            TerminalOutcome::Cancelled(CancelReason::Unknown),
        ];
        for kind in [
            ItemKind::File,
            ItemKind::Representation,
            ItemKind::DirectoryUnit,
        ] {
            for outcome in &outcomes {
                let scope = TaskScope {
                    source_entry: WireId([1; 16]),
                    attempt: WireId([2; 16]),
                };
                let result = TerminalResult {
                    item: LogicalItem { kind, index: 0 },
                    round: 3,
                    sequence: 4,
                    result_revision: 5,
                    bytes: 0,
                    total: Some(0),
                    outcome: outcome.clone(),
                };
                assert_eq!(
                    ack(terminal(WireId([9; 16]), scope.clone(), 8, result.clone())),
                    Message::TerminalAck {
                        binding: WireId([9; 16]),
                        scope,
                        owner_revision: 8,
                        result
                    }
                );
            }
        }
    }
}
