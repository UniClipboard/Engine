use diesel::connection::Connection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{Binary, Nullable};
use serde::{Deserialize, Serialize};
use uc_core::membership::{JoinerAdmission, SpaceAdmissionAggregate};

use super::codec::{map_key_error, EncryptedRecordRow};
use super::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};
use crate::db::ports::DbExecutor;

#[derive(QueryableByName)]
struct RecoverySummaryRow {
    #[diesel(sql_type = Binary)]
    lookup_token: Vec<u8>,
    #[diesel(sql_type = Binary)]
    content_token: Vec<u8>,
    #[diesel(sql_type = Nullable<Binary>)]
    encrypted_recovery_summary: Option<Vec<u8>>,
}

#[derive(Serialize, Deserialize)]
struct RecoverySummary {
    admission_id: [u8; 32],
    pending: bool,
}

impl RecoverySummaryRow {
    fn purpose(&self) -> Vec<u8> {
        let mut purpose = b"space-admission-recovery-summary-v1".to_vec();
        purpose.extend_from_slice(&self.lookup_token);
        purpose.extend_from_slice(&self.content_token);
        purpose
    }
}

fn needs_recovery(aggregate: &SpaceAdmissionAggregate) -> bool {
    aggregate.pending_recovery().is_some()
        || aggregate.invitation_resolution().is_some()
        || aggregate.has_expirable_local_join()
        || aggregate.has_pending_local_termination()
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    // rust-style: allow-qualified-path -- 相邻 recovery 模块需要调用此仓储查询
    pub(in crate::space::admission) fn load_pending_recovery_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<Vec<JoinerAdmission>, SpaceAdmissionStateStoreError> {
        // 摘要与正文属于同一快照；缺失摘要只从认证后的当前正文重建。
        conn.transaction(|conn| {
            if self.load_metadata_on(conn)?.is_none() {
                return Ok(Vec::new());
            }
            let rows = sql_query(
                "SELECT record.lookup_token, record.content_token, \
                 summary.encrypted_payload AS encrypted_recovery_summary \
                 FROM admission_repository_record AS record \
                 LEFT JOIN admission_recovery_summary AS summary \
                 ON summary.lookup_token = record.lookup_token \
                 AND summary.content_token = record.content_token \
                 ORDER BY record.lookup_token",
            )
            .load::<RecoverySummaryRow>(conn)?;
            let mut pending = Vec::new();
            for row in rows {
                let purpose = row.purpose();
                let summary = match row.encrypted_recovery_summary.as_ref() {
                    Some(encrypted) => {
                        let plaintext = self
                            .keys
                            .profile_payload_reader(&purpose)
                            .map_err(map_key_error)?
                            .open_compact(encrypted)
                            .map_err(map_key_error)?;
                        postcard::from_bytes::<RecoverySummary>(&plaintext)
                            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?
                    }
                    None => {
                        let record_row = sql_query(
                            "SELECT lookup_token, content_token, encrypted_payload \
                             FROM admission_repository_record WHERE lookup_token = ?",
                        )
                        .bind::<Binary, _>(&row.lookup_token)
                        .get_result::<EncryptedRecordRow>(conn)?;
                        let record = self.open_v3_record_row(record_row)?;
                        let aggregate = self.open_record(record.admission_id, &record.stored)?;
                        let summary = RecoverySummary {
                            admission_id: record.admission_id,
                            pending: needs_recovery(&aggregate),
                        };
                        let plaintext = postcard::to_stdvec(&summary)
                            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
                        let encrypted = self
                            .keys
                            .seal_profile_payload_compact(&purpose, &plaintext)
                            .map_err(map_key_error)?;
                        sql_query(
                            "INSERT INTO admission_recovery_summary \
                             (lookup_token, content_token, encrypted_payload) VALUES (?, ?, ?) \
                             ON CONFLICT(lookup_token) DO UPDATE SET \
                             content_token = excluded.content_token, \
                             encrypted_payload = excluded.encrypted_payload",
                        )
                        .bind::<Binary, _>(&row.lookup_token)
                        .bind::<Binary, _>(&row.content_token)
                        .bind::<Binary, _>(encrypted)
                        .execute(conn)?;
                        if summary.pending {
                            pending.push(
                                JoinerAdmission::try_from_record(aggregate)
                                    .ok_or(SpaceAdmissionStateStoreError::Corrupt)?,
                            );
                        }
                        continue;
                    }
                };
                if summary.pending {
                    let stored = self.load_v3_record_on(conn, summary.admission_id)?;
                    let aggregate = self.open_record(summary.admission_id, &stored)?;
                    if !needs_recovery(&aggregate) {
                        return Err(SpaceAdmissionStateStoreError::Corrupt);
                    }
                    pending.push(
                        JoinerAdmission::try_from_record(aggregate)
                            .ok_or(SpaceAdmissionStateStoreError::Corrupt)?,
                    );
                }
            }
            Ok(pending)
        })
    }
}
