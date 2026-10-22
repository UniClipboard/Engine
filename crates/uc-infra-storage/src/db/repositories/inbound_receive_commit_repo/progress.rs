//! 逐项持久交付与清理责任移交；密文读取后的并发变化由原子比较拒绝。
use super::{map_error, CommitInvariantError};
use crate::db::models::ReceiveArtifactLogRow;
use crate::db::ports::DbExecutor;
use crate::db::repositories::receive_artifact_log_repo::ReceiveArtifactProtection;
use crate::db::schema::{file_transfer, receive_artifact_log};
use anyhow::Result;
use diesel::prelude::*;
use uc_core::ports::inbound_receive_commit::{
    receive_artifacts_after_settlement, ReceiveArtifactSettlement,
};
use uc_core::ports::{
    InboundReceiveCommitError, ReceiveArtifact, ReceiveArtifactPhase, ReceiveArtifactRecord,
    ReceiveArtifactResolution,
};

pub(super) struct PreparedFileArtifacts {
    item_id: String,
    before: Vec<u8>,
    after: Vec<u8>,
}
impl PreparedFileArtifacts {
    pub(super) async fn prepare(
        executor: &impl DbExecutor,
        protection: &ReceiveArtifactProtection,
        entry_id: &str,
        attempt_id: &str,
        delivered: &ReceiveArtifact,
        settlement: ReceiveArtifactSettlement<'_>,
    ) -> Result<Self, InboundReceiveCommitError> {
        let entry = entry_id.to_owned();
        let attempt = attempt_id.to_owned();
        let row = executor
            .run(move |conn| {
                receive_artifact_log::table
                    .filter(receive_artifact_log::entry_id.eq(entry))
                    .filter(receive_artifact_log::attempt_id.eq(attempt))
                    .select(ReceiveArtifactLogRow::as_select())
                    .first(conn)
                    .optional()
                    .map_err(Into::into)
            })
            .map_err(map_error)?
            .ok_or(InboundReceiveCommitError::ArtifactRecordMissing)?;
        let phase = ReceiveArtifactPhase::parse(&row.phase)
            .map_err(|source| InboundReceiveCommitError::Backend(Box::new(source)))?;
        let resolution = ReceiveArtifactResolution::parse(&row.resolution)
            .map_err(|source| InboundReceiveCommitError::Backend(Box::new(source)))?;
        let artifacts = protection
            .open(entry_id, attempt_id, &row.artifact_ciphertext)
            .await
            .map_err(|source| InboundReceiveCommitError::Backend(Box::new(source)))?;
        let remaining = receive_artifacts_after_settlement(
            &ReceiveArtifactRecord {
                entry_id: entry_id.to_owned(),
                attempt_id: attempt_id.to_owned(),
                phase,
                resolution,
                artifacts,
                updated_at_ms: row.updated_at_ms,
            },
            delivered,
            settlement,
        )?;
        let after = protection
            .seal(entry_id, attempt_id, &remaining)
            .await
            .map_err(|source| InboundReceiveCommitError::Backend(Box::new(source)))?;
        Ok(Self {
            item_id: delivered.item_id.clone(),
            before: row.artifact_ciphertext,
            after,
        })
    }

    pub(super) fn commit(
        &self,
        conn: &mut SqliteConnection,
        entry_id: &str,
        attempt_id: &str,
        now_ms: i64,
    ) -> Result<()> {
        let updated = diesel::update(
            receive_artifact_log::table
                .filter(receive_artifact_log::entry_id.eq(entry_id))
                .filter(receive_artifact_log::attempt_id.eq(attempt_id))
                .filter(receive_artifact_log::phase.eq(ReceiveArtifactPhase::Preparing.as_str()))
                .filter(
                    receive_artifact_log::resolution
                        .eq(ReceiveArtifactResolution::Pending.as_str()),
                )
                .filter(receive_artifact_log::artifact_ciphertext.eq(&self.before)),
        )
        .set((
            receive_artifact_log::artifact_ciphertext.eq(&self.after),
            receive_artifact_log::updated_at_ms.eq(now_ms),
        ))
        .execute(conn)?;
        if updated != 1 {
            return Err(CommitInvariantError::ArtifactStateMismatch.into());
        }
        let delivered = file_transfer::table
            .filter(file_transfer::entry_id.eq(entry_id))
            .filter(file_transfer::attempt_id.eq(attempt_id))
            .filter(file_transfer::receive_item_id.eq(&self.item_id))
            .filter(file_transfer::item_role.eq("file"))
            .filter(file_transfer::status.eq("completed"))
            .count()
            .get_result::<i64>(conn)?;
        if delivered != 1 {
            return Err(CommitInvariantError::DeliveryItemMismatch.into());
        }
        Ok(())
    }
}
