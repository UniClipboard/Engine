//! 原处理任务负责失败事实与清理的本地修复；不重新获取或保存业务内容。
use super::{ApplyInboundClipboardUseCase, ApplyInboundError};
use crate::transfer::receive::materialization::{remove_uncommitted_output, ReceiveOutputFailure};
use crate::transfer::receive::work::{ReceiveAction, ReceiveWorkError, UnsettledFile};
use anyhow::{anyhow, Error as SourceError};
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use uc_core::file_transfer::{
    receive_failure_recovery_action, FileTransferCancellationReason, OutboundProgressStatus,
    ReceiveFailureRecoveryAction, ReceiveFailureRecoveryRefusal,
};
use uc_core::ports::{
    AttemptState, BeginReceiveFailureOutcome, InboundReceiveSettlement, PartialReceiveTerminal,
};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

impl ApplyInboundClipboardUseCase {
    pub(super) async fn recover_file_settlement(
        &self,
        pending: UnsettledFile,
        action: &ReceiveAction,
        source: Arc<ReceiveWorkError>,
    ) {
        let mut output_cleaned = !pending.output_cleanup_pending;
        let mut file_recorded = false;
        let mut delay = Duration::from_secs(1);
        loop {
            match self
                .repair_file_settlement(&pending, &mut output_cleaned, &mut file_recorded)
                .await
            {
                Ok(terminal) => {
                    let status = if pending.failure.is_cancelled() {
                        OutboundProgressStatus::Cancelled {
                            reason: FileTransferCancellationReason::LocalUser,
                        }
                    } else {
                        OutboundProgressStatus::Failed
                    };
                    action.batch.settled(match terminal {
                        PartialReceiveTerminal::Failed => AttemptState::Failed,
                        PartialReceiveTerminal::Cancelled => AttemptState::Cancelled,
                    });
                    action.batch.item_result(
                        &pending.artifact.item_id,
                        0,
                        status,
                        Some(SourceError::new(source)),
                    );
                    return;
                }
                Err(error) => {
                    // discarded-source[business-outcome]: 本轮修复延期，原动作保留资源与初始失败链。
                    uc_warn!(
                        error_kind = "receive_file_settlement_repair",
                        io_error_kind = io_error_kind(&error),
                        "receive file settlement repair remains pending"
                    );
                    // 五秒期限只限制外部等待；已开始的本地 IO 不可被丢弃。
                    sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
            }
        }
    }

    async fn repair_file_settlement(
        &self,
        pending: &UnsettledFile,
        output_cleaned: &mut bool,
        file_recorded: &mut bool,
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
        if let ReceiveOutputFailure::FileCommit(failure) = pending.failure {
            if !*file_recorded {
                ports
                    .commit
                    .commit_inbound_receive(&InboundReceiveSettlement::FileFailed {
                        entry_id: pending.entry.clone(),
                        attempt_id: pending.attempt.clone(),
                        artifact: pending.artifact.clone(),
                        failure,
                        now_ms: ports.clock.now_ms(),
                    })
                    .await
                    .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?;
                *file_recorded = true;
            }
        }
        // CAS 后重新读取权威；取消可赢得父尝试，但不得覆盖已交付成功项。
        loop {
            let current = ports
                .get
                .get_entry_attempt(pending.entry.as_ref())
                .await
                .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?;
            match receive_failure_recovery_action(
                pending.entry.as_ref(),
                &pending.attempt,
                current.as_ref(),
            )
            .map_err(|error| ApplyInboundError::Internal(SourceError::new(error)))?
            {
                ReceiveFailureRecoveryAction::ClaimFailure => {
                    if pending.failure.is_cancelled() {
                        self.request_receive_cancellation(&pending.entry, Some(&pending.attempt))
                            .await?;
                        continue;
                    }
                    match ports
                        .begin_failure
                        .begin_receive_failure(
                            pending.entry.as_ref(),
                            &pending.attempt,
                            ports.clock.now_ms(),
                        )
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
                        &pending.entry,
                        Some(&pending.attempt),
                        terminal,
                        &pending.remaining,
                        pending.has_artifact_journal,
                    )
                    .await?;
                    return Ok(terminal);
                }
            }
        }
    }
}
