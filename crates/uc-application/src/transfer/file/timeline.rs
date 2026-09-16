use uc_core::file_transfer::FileTransferEventStorePort;
use uc_core::FileTransferEvent;

use crate::transfer::file::errors::FileTransferApplicationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalState {
    Completed,
    Failed,
    Cancelled,
}

/// Reconstructed transfer state derived from event history.
///
/// 根据事件历史恢复出的传输状态快照。
///
/// 这里不是新的领域真相，只是应用层为了校验“下一步能不能做”
/// 临时整理出来的判断结果。
#[derive(Debug, Default)]
pub(crate) struct TransferTimeline {
    pub(crate) started: bool,
    peer_id: Option<String>,
    terminal_state: Option<TerminalState>,
    pub(crate) last_progress_bytes: Option<u64>,
}

impl TransferTimeline {
    pub(crate) fn is_finished(&self) -> bool {
        self.terminal_state.is_some()
    }

    /// Rebuild the minimal state needed by the application layer.
    ///
    /// 从事件历史中恢复应用层所需的最小状态。
    ///
    /// 它不试图还原全部传输细节，只关心：
    /// - 是否已开始
    /// - 属于哪个对端
    /// - 是否已经结束
    /// - 最近一次进度值
    pub(crate) fn from_history(
        transfer_id: &str,
        history: &[FileTransferEvent],
    ) -> Result<Self, FileTransferApplicationError> {
        let mut timeline = Self::default();

        for event in history {
            match event {
                FileTransferEvent::Started {
                    transfer_id: event_transfer_id,
                    peer_id,
                    ..
                } => {
                    timeline.ensure_transfer_id_matches(transfer_id, event_transfer_id)?;
                    if timeline.started {
                        return Err(FileTransferApplicationError::InvalidHistory {
                            transfer_id: transfer_id.to_owned(),
                            message: "duplicate Started event".to_owned(),
                        });
                    }
                    if timeline.terminal_state.is_some() {
                        return Err(FileTransferApplicationError::InvalidHistory {
                            transfer_id: transfer_id.to_owned(),
                            message: "Started event appears after terminal state".to_owned(),
                        });
                    }

                    timeline.started = true;
                    timeline.peer_id = Some(peer_id.clone());
                }
                FileTransferEvent::Progress {
                    transfer_id: event_transfer_id,
                    peer_id,
                    progress,
                } => {
                    timeline.ensure_transfer_id_matches(transfer_id, event_transfer_id)?;
                    timeline.ensure_active(transfer_id, peer_id)?;
                    timeline.last_progress_bytes = Some(progress.bytes_transferred);
                }
                FileTransferEvent::Completed {
                    transfer_id: event_transfer_id,
                    peer_id,
                } => {
                    timeline.ensure_transfer_id_matches(transfer_id, event_transfer_id)?;
                    timeline.ensure_active(transfer_id, peer_id)?;
                    timeline.terminal_state = Some(TerminalState::Completed);
                }
                FileTransferEvent::Failed {
                    transfer_id: event_transfer_id,
                    peer_id,
                    ..
                } => {
                    timeline.ensure_transfer_id_matches(transfer_id, event_transfer_id)?;
                    timeline.ensure_active(transfer_id, peer_id)?;
                    timeline.terminal_state = Some(TerminalState::Failed);
                }
                FileTransferEvent::Cancelled {
                    transfer_id: event_transfer_id,
                    peer_id,
                    ..
                } => {
                    timeline.ensure_transfer_id_matches(transfer_id, event_transfer_id)?;
                    timeline.ensure_active(transfer_id, peer_id)?;
                    timeline.terminal_state = Some(TerminalState::Cancelled);
                }
            }
        }

        Ok(timeline)
    }

    /// Ensure the transfer is still active for the given peer.
    ///
    /// 确认这条传输对指定对端来说仍然处于可推进状态。
    pub(crate) fn ensure_active(
        &self,
        transfer_id: &str,
        peer_id: &str,
    ) -> Result<(), FileTransferApplicationError> {
        if !self.started {
            return Err(FileTransferApplicationError::TransferNotStarted {
                transfer_id: transfer_id.to_owned(),
            });
        }

        if self.terminal_state.is_some() {
            return Err(FileTransferApplicationError::TransferAlreadyFinished {
                transfer_id: transfer_id.to_owned(),
            });
        }

        match &self.peer_id {
            Some(expected_peer_id) if expected_peer_id == peer_id => Ok(()),
            Some(expected_peer_id) => Err(FileTransferApplicationError::PeerMismatch {
                transfer_id: transfer_id.to_owned(),
                expected_peer_id: expected_peer_id.clone(),
                actual_peer_id: peer_id.to_owned(),
            }),
            None => Err(FileTransferApplicationError::InvalidHistory {
                transfer_id: transfer_id.to_owned(),
                message: "started transfer is missing peer_id".to_owned(),
            }),
        }
    }

    fn ensure_transfer_id_matches(
        &self,
        expected_transfer_id: &str,
        actual_transfer_id: &str,
    ) -> Result<(), FileTransferApplicationError> {
        if expected_transfer_id == actual_transfer_id {
            Ok(())
        } else {
            Err(FileTransferApplicationError::InvalidHistory {
                transfer_id: expected_transfer_id.to_owned(),
                message: format!("history contains event for `{actual_transfer_id}`"),
            })
        }
    }
}

pub(crate) async fn load_timeline(
    store: &dyn FileTransferEventStorePort,
    transfer_id: &str,
) -> Result<TransferTimeline, FileTransferApplicationError> {
    let history = store
        .load(transfer_id)
        .await
        .map_err(FileTransferApplicationError::Store)?;
    TransferTimeline::from_history(transfer_id, &history)
}

#[cfg(test)]
mod tests {
    use uc_core::{FileTransferCancellationReason, FileTransferFailureReason};

    use super::*;

    fn started() -> FileTransferEvent {
        FileTransferEvent::started("transfer-1", "peer-1", "file.bin", Some(4))
    }

    #[test]
    fn every_terminal_state_rejects_further_progress() {
        let histories = [
            vec![
                started(),
                FileTransferEvent::completed("transfer-1", "peer-1"),
            ],
            vec![
                started(),
                FileTransferEvent::failed(
                    "transfer-1",
                    "peer-1",
                    FileTransferFailureReason::StorageUnavailable,
                    None,
                ),
            ],
            vec![
                started(),
                FileTransferEvent::cancelled(
                    "transfer-1",
                    "peer-1",
                    FileTransferCancellationReason::LocalUser,
                ),
            ],
        ];

        for history in histories {
            let timeline = TransferTimeline::from_history("transfer-1", &history).unwrap();
            assert!(matches!(
                timeline.ensure_active("transfer-1", "peer-1"),
                Err(FileTransferApplicationError::TransferAlreadyFinished {
                    ref transfer_id,
                }) if transfer_id == "transfer-1"
            ));
        }
    }

    #[test]
    fn a_second_terminal_state_is_rejected() {
        let history = vec![
            started(),
            FileTransferEvent::completed("transfer-1", "peer-1"),
            FileTransferEvent::failed(
                "transfer-1",
                "peer-1",
                FileTransferFailureReason::Unknown,
                None,
            ),
        ];

        assert!(matches!(
            TransferTimeline::from_history("transfer-1", &history).unwrap_err(),
            FileTransferApplicationError::TransferAlreadyFinished {
                ref transfer_id,
            } if transfer_id == "transfer-1"
        ));
    }
}
