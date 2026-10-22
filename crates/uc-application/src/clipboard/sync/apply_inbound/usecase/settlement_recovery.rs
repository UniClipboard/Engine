//! 原处理任务负责失败事实与清理的本地修复；不重新获取或保存业务内容。
use super::{ApplyInboundClipboardUseCase, ApplyInboundError};
use crate::transfer::blob::facade::FetchTerminalFailure;
use crate::transfer::receive::materialization::{remove_uncommitted_output, ReceiveOutputFailure};
use crate::transfer::receive::work::{
    ReceiveAction, ReceiveWorkError, UnsettledFile, UnsettledReceive,
};
use crate::transfer::receive::{LOCAL_SETTLEMENT_RETRY_INITIAL, LOCAL_SETTLEMENT_RETRY_MAX};
use anyhow::{anyhow, Error as SourceError};
use std::error::Error as StdError;
use std::sync::Arc;
use tokio::time::sleep;
use uc_core::file_transfer::{
    receive_failure_recovery_action, FileTransferCancellationReason, OutboundProgressStatus,
    ReceiveFailureRecoveryAction, ReceiveFailureRecoveryRefusal,
};
use uc_core::ids::EntryId;
use uc_core::ports::{
    AttemptState, BeginReceiveFailureOutcome, InboundReceiveSettlement, PartialReceiveTerminal,
    ReceiveArtifact,
};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

impl ApplyInboundClipboardUseCase {
    pub(super) async fn recover_settlement(
        &self,
        pending: UnsettledReceive,
        action: &ReceiveAction,
        source: Arc<ReceiveWorkError>,
    ) {
        // 首次调用错误不是网络任务的完成屏障；原负责人仍可在提交同一终态。
        let terminal = {
            let mut current: Option<&(dyn StdError + 'static)> = Some(source.as_ref());
            let mut pending = None;
            for _ in 0..32 {
                let Some(error) = current else { break };
                if let Some(terminal) = error.downcast_ref::<FetchTerminalFailure>() {
                    pending = Some(terminal.clone());
                    break;
                }
                current = error.source();
            }
            pending
        };
        if let Some(terminal) = terminal {
            terminal.wait_settled().await;
        }
        let mut output_cleaned = match &pending {
            UnsettledReceive::File(file) => !file.output_cleanup_pending,
            UnsettledReceive::Representations(_) => true,
        };
        let mut item_acknowledged = false;
        let mut delay = LOCAL_SETTLEMENT_RETRY_INITIAL;
        loop {
            let result = match &pending {
                UnsettledReceive::File(file) => {
                    self.repair_file_settlement(file, &mut output_cleaned, &mut item_acknowledged)
                        .await
                }
                UnsettledReceive::Representations(unit) => {
                    self.repair_failure_settlement(
                        &unit.entry,
                        &unit.attempt,
                        unit.cancelled,
                        &[],
                        false,
                    )
                    .await
                }
            };
            match result {
                Ok(terminal) => {
                    action.batch.settled(match terminal {
                        PartialReceiveTerminal::Failed => AttemptState::Failed,
                        PartialReceiveTerminal::Cancelled => AttemptState::Cancelled,
                    });
                    if let UnsettledReceive::File(file) = &pending {
                        let status = if file.failure.is_cancelled() {
                            OutboundProgressStatus::Cancelled {
                                reason: FileTransferCancellationReason::LocalUser,
                            }
                        } else {
                            OutboundProgressStatus::Failed
                        };
                        action.batch.item_result(
                            &file.artifact.item_id,
                            0,
                            status,
                            Some(SourceError::new(source)),
                        );
                    }
                    return;
                }
                Err(error) => {
                    // discarded-source[business-outcome]: 本轮修复延期，原动作保留资源与初始失败链。
                    uc_warn!(
                        error_kind = "receive_settlement_repair",
                        io_error_kind = io_error_kind(&error),
                        "receive settlement repair remains pending"
                    );
                    // 五秒期限只限制外部等待；已开始的本地 IO 不可被丢弃。
                    sleep(delay).await;
                    delay = (delay * 2).min(LOCAL_SETTLEMENT_RETRY_MAX);
                }
            }
        }
    }

    async fn repair_file_settlement(
        &self,
        pending: &UnsettledFile,
        output_cleaned: &mut bool,
        item_acknowledged: &mut bool,
    ) -> Result<PartialReceiveTerminal, ApplyInboundError> {
        let ports = self.receive_attempts.as_ref().ok_or_else(|| {
            ApplyInboundError::Internal(anyhow!("file settlement ports are not wired"))
        })?;
        if !*output_cleaned {
            remove_uncommitted_output(&pending.artifact.final_path)
                .await
                .map_err(|source| ApplyInboundError::Internal(SourceError::new(source)))?;
            // 清理成功后不再触碰相同路径；存储重试期间该路径可能已被重新使用。
            *output_cleaned = true;
        }
        if !*item_acknowledged {
            let settlement = match pending.failure {
                ReceiveOutputFailure::FileCommit(failure) => {
                    Some(InboundReceiveSettlement::FileFailed {
                        entry_id: pending.entry.clone(),
                        attempt_id: pending.attempt.clone(),
                        artifact: pending.artifact.clone(),
                        failure,
                        now_ms: ports.clock.now_ms(),
                    })
                }
                ReceiveOutputFailure::FetchFailure | ReceiveOutputFailure::FetchCancellation => {
                    Some(InboundReceiveSettlement::FileCleaned {
                        entry_id: pending.entry.clone(),
                        attempt_id: pending.attempt.clone(),
                        artifact: pending.artifact.clone(),
                        now_ms: ports.clock.now_ms(),
                    })
                }
                ReceiveOutputFailure::PreparationFailure => None,
            };
            if let Some(settlement) = settlement {
                ports
                    .commit
                    .commit_inbound_receive(&settlement)
                    .await
                    .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?;
            }
            *item_acknowledged = true;
        }
        self.repair_failure_settlement(
            &pending.entry,
            &pending.attempt,
            pending.failure.is_cancelled(),
            &pending.remaining,
            pending.has_artifact_journal,
        )
        .await
    }

    pub(super) async fn repair_failure_settlement(
        &self,
        entry: &EntryId,
        attempt: &str,
        cancelled: bool,
        artifacts: &[ReceiveArtifact],
        has_artifact_journal: bool,
    ) -> Result<PartialReceiveTerminal, ApplyInboundError> {
        let ports = self.receive_attempts.as_ref().ok_or_else(|| {
            ApplyInboundError::Internal(anyhow!("receive settlement ports are not wired"))
        })?;
        // CAS 后重新读取权威；取消可赢得父尝试，但不得覆盖已交付成功项。
        loop {
            let current = ports
                .get
                .get_entry_attempt(entry.as_ref())
                .await
                .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?;
            match receive_failure_recovery_action(entry.as_ref(), attempt, current.as_ref())
                .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?
            {
                ReceiveFailureRecoveryAction::ClaimFailure => {
                    if cancelled {
                        self.request_receive_cancellation(entry, Some(attempt))
                            .await?;
                        continue;
                    }
                    match ports
                        .begin_failure
                        .begin_receive_failure(entry.as_ref(), attempt, ports.clock.now_ms())
                        .await
                        .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?
                    {
                        BeginReceiveFailureOutcome::Begun
                        | BeginReceiveFailureOutcome::CancellationWon
                        | BeginReceiveFailureOutcome::Terminal => {}
                        BeginReceiveFailureOutcome::Superseded => {
                            return Err(ApplyInboundError::Internal(SourceError::new(
                                ReceiveFailureRecoveryRefusal::Superseded,
                            )));
                        }
                    }
                }
                ReceiveFailureRecoveryAction::Settle(terminal) => {
                    self.settle_receive_without_entry(
                        entry,
                        Some(attempt),
                        terminal,
                        artifacts,
                        has_artifact_journal,
                    )
                    .await?;
                    return Ok(terminal);
                }
            }
        }
    }
}
