//! 本地保存失败与产物清理责任在同一事务内结算，网络终态不回退。
use super::super::file_transfer_repo::{receive_failure_code, ENTRY_DELIVERED_BINDING};
use super::progress::PreparedFileArtifacts;
use super::{map_error, CommitInvariantError, DieselInboundReceiveCommitRepository};
use crate::db::ports::DbExecutor;
use crate::db::schema::{entry_receive_attempt, file_transfer};
use diesel::prelude::*;
use std::str::FromStr;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ids::EntryId;
use uc_core::ports::inbound_receive_commit::ReceiveArtifactSettlement;
use uc_core::ports::{
    AttemptState, InboundReceiveCommitError, ReceiveArtifact, ReceiveFileFailure,
};

impl<E: DbExecutor> DieselInboundReceiveCommitRepository<E> {
    pub(super) async fn commit_file_failure(
        &self,
        entry: &EntryId,
        attempt: &str,
        artifact: &ReceiveArtifact,
        failure: ReceiveFileFailure,
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
        let code = match failure {
            ReceiveFileFailure::Failed(reason) => receive_failure_code(reason),
            ReceiveFileFailure::Cancelled => FileTransferCancellationReason::LocalUser.as_str(),
        };
        self.executor
            .run(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| {
                    let state = entry_receive_attempt::table
                        .filter(entry_receive_attempt::entry_id.eq(&entry))
                        .filter(entry_receive_attempt::current_attempt_id.eq(&attempt))
                        .select(entry_receive_attempt::attempt_state)
                        .first::<String>(conn)
                        .optional()?;
                    let state = state.as_deref().map(AttemptState::from_str).transpose()?;
                    if !state.is_some_and(|state| failure.accepts_attempt(state)) {
                        return Err(CommitInvariantError::AttemptStateMismatch.into());
                    }
                    prepared.commit(conn, &entry, &attempt, now_ms)?;
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
                    Ok(())
                })
            })
            .map_err(map_error)
    }
}
