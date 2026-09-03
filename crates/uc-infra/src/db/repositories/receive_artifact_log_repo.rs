use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::upsert::excluded;
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::{DeriveSpaceSubkeyPort, SpaceAccessError};
use uc_core::ports::{
    ListUnsettledReceiveArtifactsPort, ReceiveArtifactLogError, ReceiveArtifactPhase,
    ReceiveArtifactRecord, ReceiveArtifactResolution, RecordReceiveArtifactsPort,
};

use super::receive_artifact_cipher::{
    ReceiveArtifactCipher, V3ReceiveArtifactCipher, ARTIFACT_KEY_INFO,
};
use crate::db::models::{NewReceiveArtifactLogRow, ReceiveArtifactLogRow};
use crate::db::ports::DbExecutor;
use crate::db::schema::receive_artifact_log;
use crate::security::ContentProtection;

pub struct DieselReceiveArtifactLogRepository<E> {
    executor: E,
    protection: ReceiveArtifactProtection,
}

enum ReceiveArtifactProtection {
    Legacy {
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
        current_profile: Arc<dyn CurrentProfilePort>,
    },
    V3(V3ReceiveArtifactCipher),
}

impl<E> DieselReceiveArtifactLogRepository<E> {
    pub fn new(
        executor: E,
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
        current_profile: Arc<dyn CurrentProfilePort>,
    ) -> Self {
        Self {
            executor,
            protection: ReceiveArtifactProtection::Legacy {
                derive_subkey,
                current_profile,
            },
        }
    }

    pub fn new_v3(executor: E, protection: Arc<ContentProtection>) -> Self {
        Self {
            executor,
            protection: ReceiveArtifactProtection::V3(V3ReceiveArtifactCipher::new(protection)),
        }
    }
}

impl ReceiveArtifactProtection {
    async fn legacy_cipher(
        derive_subkey: &dyn DeriveSpaceSubkeyPort,
        current_profile: &dyn CurrentProfilePort,
    ) -> Result<ReceiveArtifactCipher, ReceiveArtifactLogError> {
        let profile = current_profile.current_profile().await.map_err(|error| {
            ReceiveArtifactLogError::EncryptionUnavailable(format!(
                "current profile unavailable: {error}"
            ))
        })?;
        let key = derive_subkey
            .derive_subkey(profile.as_ref().as_bytes(), ARTIFACT_KEY_INFO)
            .await
            .map_err(|error| match error {
                SpaceAccessError::NotUnlocked => ReceiveArtifactLogError::EncryptionUnavailable(
                    "session locked: receive artifact key unavailable".to_owned(),
                ),
                other => ReceiveArtifactLogError::EncryptionUnavailable(format!(
                    "derive receive artifact key: {other}"
                )),
            })?;
        Ok(ReceiveArtifactCipher::new(key))
    }

    async fn seal(
        &self,
        entry_id: &str,
        attempt_id: &str,
        artifacts: &[uc_core::ports::ReceiveArtifact],
    ) -> Result<Vec<u8>, ReceiveArtifactLogError> {
        match self {
            Self::Legacy {
                derive_subkey,
                current_profile,
            } => Self::legacy_cipher(derive_subkey.as_ref(), current_profile.as_ref())
                .await?
                .seal(entry_id, attempt_id, artifacts),
            Self::V3(cipher) => cipher.seal(entry_id, attempt_id, artifacts).await,
        }
        .map_err(|error| ReceiveArtifactLogError::Backend(error.to_string()))
    }

    async fn open(
        &self,
        entry_id: &str,
        attempt_id: &str,
        ciphertext: &[u8],
    ) -> Result<Vec<uc_core::ports::ReceiveArtifact>, ReceiveArtifactLogError> {
        match self {
            Self::Legacy {
                derive_subkey,
                current_profile,
            } => Self::legacy_cipher(derive_subkey.as_ref(), current_profile.as_ref())
                .await?
                .open(entry_id, attempt_id, ciphertext),
            Self::V3(cipher) => cipher.open(entry_id, attempt_id, ciphertext).await,
        }
        .map_err(|error| ReceiveArtifactLogError::Backend(error.to_string()))
    }
}

fn backend(error: anyhow::Error) -> ReceiveArtifactLogError {
    ReceiveArtifactLogError::Backend(error.to_string())
}

async fn decode_row(
    protection: &ReceiveArtifactProtection,
    row: ReceiveArtifactLogRow,
) -> Result<ReceiveArtifactRecord, ReceiveArtifactLogError> {
    let phase = ReceiveArtifactPhase::parse(&row.phase)?;
    let resolution = ReceiveArtifactResolution::parse(&row.resolution)?;
    let artifacts = protection
        .open(&row.entry_id, &row.attempt_id, &row.artifact_ciphertext)
        .await?;
    Ok(ReceiveArtifactRecord {
        entry_id: row.entry_id,
        attempt_id: row.attempt_id,
        phase,
        resolution,
        artifacts,
        updated_at_ms: row.updated_at_ms,
    })
}

#[async_trait]
impl<E: DbExecutor> RecordReceiveArtifactsPort for DieselReceiveArtifactLogRepository<E> {
    async fn record_receive_artifacts(
        &self,
        record: &ReceiveArtifactRecord,
    ) -> Result<(), ReceiveArtifactLogError> {
        let ciphertext = self
            .protection
            .seal(&record.entry_id, &record.attempt_id, &record.artifacts)
            .await?;
        let row = NewReceiveArtifactLogRow {
            entry_id: record.entry_id.clone(),
            attempt_id: record.attempt_id.clone(),
            phase: record.phase.as_str().to_owned(),
            resolution: record.resolution.as_str().to_owned(),
            artifact_ciphertext: ciphertext,
            updated_at_ms: record.updated_at_ms,
        };
        self.executor
            .run(move |conn| {
                diesel::insert_into(receive_artifact_log::table)
                    .values(&row)
                    .on_conflict((
                        receive_artifact_log::entry_id,
                        receive_artifact_log::attempt_id,
                    ))
                    .do_update()
                    .set((
                        receive_artifact_log::phase.eq(excluded(receive_artifact_log::phase)),
                        receive_artifact_log::resolution
                            .eq(excluded(receive_artifact_log::resolution)),
                        receive_artifact_log::artifact_ciphertext
                            .eq(excluded(receive_artifact_log::artifact_ciphertext)),
                        receive_artifact_log::updated_at_ms
                            .eq(excluded(receive_artifact_log::updated_at_ms)),
                    ))
                    .execute(conn)?;
                Ok(())
            })
            .map_err(backend)
    }
}

#[async_trait]
impl<E: DbExecutor> ListUnsettledReceiveArtifactsPort for DieselReceiveArtifactLogRepository<E> {
    async fn list_unsettled_receive_artifacts(
        &self,
    ) -> Result<Vec<ReceiveArtifactRecord>, ReceiveArtifactLogError> {
        let rows = self
            .executor
            .run(move |conn| {
                receive_artifact_log::table
                    .filter(
                        receive_artifact_log::resolution
                            .eq(ReceiveArtifactResolution::Pending.as_str()),
                    )
                    .order(receive_artifact_log::updated_at_ms.asc())
                    .select(ReceiveArtifactLogRow::as_select())
                    .load(conn)
                    .map_err(anyhow::Error::from)
            })
            .map_err(backend)?;
        let mut decoded = Vec::with_capacity(rows.len());
        for row in rows {
            decoded.push(decode_row(&self.protection, row).await?);
        }
        Ok(decoded)
    }
}
