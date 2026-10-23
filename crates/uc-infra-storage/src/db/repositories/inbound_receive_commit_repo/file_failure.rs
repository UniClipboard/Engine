//! 已失败文件的产物责任与本地保存失败事实原子结算，网络终态不回退。
use super::super::file_transfer_repo::{receive_failure_code, ENTRY_DELIVERED_BINDING};
use super::progress::PreparedFileArtifacts;
use super::{map_error, CommitInvariantError, DieselInboundReceiveCommitRepository};
use crate::db::ports::DbExecutor;
use crate::db::schema::{entry_receive_attempt, file_transfer};
use anyhow::Result;
use diesel::prelude::*;
use std::str::FromStr;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ids::EntryId;
use uc_core::ports::inbound_receive_commit::ReceiveArtifactSettlement;
use uc_core::ports::{
    receive_file_settlement_decision, AttemptState, InboundReceiveCommitError, ReceiveArtifact,
    ReceiveFileFailure, ReceiveFileSettlementDecision, ReceiveFileSettlementRecord,
    TrackedFileTransferStatus,
};

impl<E: DbExecutor> DieselInboundReceiveCommitRepository<E> {
    pub(super) async fn commit_failed_file_artifacts(
        &self,
        entry: &EntryId,
        attempt: &str,
        artifact: &ReceiveArtifact,
        failure: Option<ReceiveFileFailure>,
        now_ms: i64,
    ) -> Result<(), InboundReceiveCommitError> {
        let prepared = PreparedFileArtifacts::prepare(
            &self.executor,
            &self.artifact_protection,
            entry.as_ref(),
            attempt,
            artifact,
            ReceiveArtifactSettlement::Cleaned,
        )
        .await?;
        let entry = entry.to_string();
        let attempt = attempt.to_owned();
        let item = artifact.item_id.clone();
        let code = failure.map(|failure| match failure {
            ReceiveFileFailure::Failed(reason) => receive_failure_code(reason),
            ReceiveFileFailure::Cancelled => FileTransferCancellationReason::LocalUser.as_str(),
        });
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    validate_file_settlement(conn, &entry, &attempt, &item, failure)?;
                    prepared.commit_log(conn, &entry, &attempt, now_ms)?;
                    if let Some(code) = code {
                        let changed = diesel::update(
                            file_transfer::table
                                .filter(file_transfer::entry_id.eq(&entry))
                                .filter(file_transfer::attempt_id.eq(&attempt))
                                .filter(file_transfer::receive_item_id.eq(&item))
                                .filter(file_transfer::status.eq("completed"))
                                .filter(file_transfer::binding_state.ne(ENTRY_DELIVERED_BINDING))
                                .filter(file_transfer::failure_code.is_null()),
                        )
                        .set((
                            file_transfer::failure_code.eq(code),
                            file_transfer::updated_at_ms.eq(now_ms),
                        ))
                        .execute(conn)?;
                        if changed != 1 {
                            return Err(CommitInvariantError::DeliveryItemMismatch.into());
                        }
                    }
                    Ok(())
                })
            })
            .map_err(map_error)
    }
}

fn validate_file_settlement(
    conn: &mut SqliteConnection,
    entry: &str,
    attempt: &str,
    item: &str,
    failure: Option<ReceiveFileFailure>,
) -> Result<()> {
    let state = entry_receive_attempt::table
        .filter(entry_receive_attempt::entry_id.eq(entry))
        .filter(entry_receive_attempt::current_attempt_id.eq(attempt))
        .select(entry_receive_attempt::attempt_state)
        .first::<String>(conn)
        .optional()?;
    let state = state.as_deref().map(AttemptState::from_str).transpose()?;
    let transfers = file_transfer::table
        .filter(file_transfer::entry_id.eq(entry))
        .filter(file_transfer::attempt_id.eq(attempt))
        .filter(file_transfer::receive_item_id.eq(item))
        .filter(file_transfer::item_role.eq("file"))
        .select((file_transfer::status, file_transfer::binding_state))
        // 两行已经足以拒绝不唯一的文件事实，损坏记录不能放大读取量。
        .limit(2)
        .load::<(String, String)>(conn)?;
    let file = match transfers.as_slice() {
        [] => ReceiveFileSettlementRecord::Missing,
        [(status, binding)] => TrackedFileTransferStatus::from_str_value(status)
            .map(|network| ReceiveFileSettlementRecord::Tracked {
                network,
                delivered: binding == ENTRY_DELIVERED_BINDING,
            })
            .unwrap_or(ReceiveFileSettlementRecord::Unusable),
        _ => ReceiveFileSettlementRecord::Unusable,
    };
    match receive_file_settlement_decision(state, file, failure) {
        ReceiveFileSettlementDecision::Allowed => Ok(()),
        ReceiveFileSettlementDecision::AttemptMismatch => {
            Err(CommitInvariantError::AttemptStateMismatch.into())
        }
        ReceiveFileSettlementDecision::FileMismatch => {
            Err(CommitInvariantError::DeliveryItemMismatch.into())
        }
    }
}
