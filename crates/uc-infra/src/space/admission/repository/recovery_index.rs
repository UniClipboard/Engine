use diesel::connection::Connection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{Binary, Integer, Nullable};
use serde::{Deserialize, Serialize};
use uc_core::membership::{
    AdmissionRole, JoinerAdmission, SpaceAdmissionAggregate, SponsorAdmission,
    SponsorPairingConfirmationStatus,
};

use super::codec::{map_key_error, EncryptedRecordRow};
use super::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};
use crate::db::ports::DbExecutor;

const RECOVERY_SUMMARY_FORMAT_V2: u16 = 2;
const RECOVERY_INDEX_BATCH_SIZE: i32 = 64;
const RECOVERY_SUMMARY_MARKER: [u8; 8] = *b"UCARSV2\0";

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
struct LegacyRecoverySummaryV1 {
    admission_id: [u8; 32],
    pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RecoveryRecordRole {
    Joiner,
    Sponsor,
    CompletionHelper,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RecoveryAction {
    None,
    JoinerNetwork,
    JoinerExpiry,
    SponsorDeadline,
    SponsorAbandonment,
    CompletionHelper,
    SponsorConfirmation,
}

#[derive(Serialize, Deserialize)]
struct RecoverySummary {
    marker: [u8; 8],
    format_version: u16,
    role: RecoveryRecordRole,
    admission_id: [u8; 32],
    expires_at_ms: Option<i64>,
    legacy_no_deadline: bool,
    action: RecoveryAction,
    content_token: [u8; 32],
}

pub(crate) struct LoadedRecoveryIndex {
    pub(crate) joiners: Vec<JoinerAdmission>,
    pub(crate) sponsor_deadlines: Vec<SponsorAdmission>,
    pub(crate) sponsor_abandonments: Vec<SponsorAdmission>,
    pub(crate) next_deadline_ms: Option<i64>,
    pub(crate) sponsor_confirmation_pending: bool,
}

impl RecoverySummaryRow {
    fn purpose(&self) -> Vec<u8> {
        let mut purpose = b"space-admission-recovery-summary-v1".to_vec();
        purpose.extend_from_slice(&self.lookup_token);
        purpose.extend_from_slice(&self.content_token);
        purpose
    }
}

impl RecoverySummary {
    fn from_aggregate(
        aggregate: &SpaceAdmissionAggregate,
        content_token: &[u8],
    ) -> Result<Self, SpaceAdmissionStateStoreError> {
        let content_token: [u8; 32] = content_token
            .try_into()
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let role = match aggregate.record_role() {
            Some(AdmissionRole::Joiner) => RecoveryRecordRole::Joiner,
            Some(AdmissionRole::Sponsor) => RecoveryRecordRole::Sponsor,
            Some(AdmissionRole::CompletionHelper) => RecoveryRecordRole::CompletionHelper,
            None => RecoveryRecordRole::Unknown,
        };
        let sponsor_confirmation_pending =
            aggregate
                .sponsor_pairing_confirmation()
                .is_some_and(|summary| {
                    summary.status() == SponsorPairingConfirmationStatus::AwaitingPeerConfirmation
                });
        let action = if aggregate.has_pending_sponsor_abandonment() {
            RecoveryAction::SponsorAbandonment
        } else if sponsor_confirmation_pending {
            RecoveryAction::SponsorConfirmation
        } else if aggregate.has_expirable_sponsor() {
            RecoveryAction::SponsorDeadline
        } else if aggregate.pending_recovery().is_some()
            || aggregate.invitation_resolution().is_some()
            || aggregate.has_pending_local_termination()
        {
            RecoveryAction::JoinerNetwork
        } else if aggregate.has_expirable_local_join() {
            RecoveryAction::JoinerExpiry
        } else if role == RecoveryRecordRole::CompletionHelper
            && aggregate.expires_at_ms().is_some()
        {
            RecoveryAction::CompletionHelper
        } else {
            RecoveryAction::None
        };
        let expires_at_ms = match action {
            RecoveryAction::JoinerNetwork
            | RecoveryAction::JoinerExpiry
            | RecoveryAction::SponsorConfirmation
            | RecoveryAction::SponsorDeadline
            | RecoveryAction::CompletionHelper => aggregate.expires_at_ms(),
            RecoveryAction::None | RecoveryAction::SponsorAbandonment => None,
        };
        let legacy_no_deadline = !aggregate.is_terminal()
            && (aggregate.expires_at_ms().is_none()
                || (role == RecoveryRecordRole::Sponsor
                    && aggregate.sponsor_pairing_confirmation().is_none()
                    && !aggregate.has_expirable_sponsor()));
        Ok(Self {
            marker: RECOVERY_SUMMARY_MARKER,
            format_version: RECOVERY_SUMMARY_FORMAT_V2,
            role,
            admission_id: *aggregate.admission_id().as_bytes(),
            expires_at_ms,
            legacy_no_deadline,
            action,
            content_token,
        })
    }

    fn is_due(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|deadline| now_ms >= deadline)
    }

    fn needs_body(&self, now_ms: i64) -> bool {
        match self.action {
            RecoveryAction::JoinerNetwork => true,
            RecoveryAction::JoinerExpiry => self.is_due(now_ms),
            RecoveryAction::SponsorConfirmation
            | RecoveryAction::SponsorDeadline
            | RecoveryAction::CompletionHelper => self.is_due(now_ms),
            RecoveryAction::SponsorAbandonment => true,
            RecoveryAction::None => false,
        }
    }
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    // rust-style: allow-qualified-path -- 相邻 recovery 模块需要调用此仓储查询
    pub(in crate::space::admission) fn load_recovery_index_on(
        &self,
        conn: &mut SqliteConnection,
        now_ms: i64,
    ) -> Result<LoadedRecoveryIndex, SpaceAdmissionStateStoreError> {
        // Legacy migration must acquire write eligibility before the recovery read transaction.
        self.load_metadata_on(conn)?;
        conn.transaction(|conn| {
            if self.load_metadata_on(conn)?.is_none() {
                return Ok(LoadedRecoveryIndex {
                    joiners: Vec::new(),
                    sponsor_deadlines: Vec::new(),
                    sponsor_abandonments: Vec::new(),
                    next_deadline_ms: None,
                    sponsor_confirmation_pending: false,
                });
            }
            let mut cursor: Option<Vec<u8>> = None;
            let mut joiners = Vec::new();
            let mut sponsor_deadlines = Vec::new();
            let mut sponsor_abandonments = Vec::new();
            let mut next_deadline_ms: Option<i64> = None;
            let mut sponsor_confirmation_pending = false;
            loop {
                let rows = load_summary_batch(conn, cursor.as_deref())?;
                if rows.is_empty() {
                    break;
                }
                for row in &rows {
                    let mut rebuilt_aggregate = None;
                    let summary = match self.open_recovery_summary(row)? {
                        Some(summary) => summary,
                        None => {
                            let aggregate = self.load_recovery_aggregate(conn, row)?;
                            let summary =
                                RecoverySummary::from_aggregate(&aggregate, &row.content_token)?;
                            self.save_recovery_summary(conn, row, &summary)?;
                            rebuilt_aggregate = Some(aggregate);
                            summary
                        }
                    };
                    if summary.content_token.as_slice() != row.content_token.as_slice()
                        || self.record_lookup_token(summary.admission_id)?.as_slice()
                            != row.lookup_token.as_slice()
                    {
                        return Err(SpaceAdmissionStateStoreError::Corrupt);
                    }
                    if let Some(deadline) =
                        summary.expires_at_ms.filter(|deadline| *deadline > now_ms)
                    {
                        next_deadline_ms = Some(
                            next_deadline_ms.map_or(deadline, |current| current.min(deadline)),
                        );
                    }
                    if summary.action == RecoveryAction::SponsorConfirmation
                        && !summary.is_due(now_ms)
                    {
                        sponsor_confirmation_pending = true;
                    }
                    if !summary.needs_body(now_ms) {
                        continue;
                    }
                    let aggregate = match rebuilt_aggregate.take() {
                        Some(aggregate) => aggregate,
                        None => self.load_recovery_aggregate(conn, row)?,
                    };
                    match summary.action {
                        RecoveryAction::JoinerNetwork | RecoveryAction::JoinerExpiry => joiners
                            .push(
                                JoinerAdmission::try_from_record(aggregate)
                                    .ok_or(SpaceAdmissionStateStoreError::Corrupt)?,
                            ),
                        RecoveryAction::SponsorConfirmation | RecoveryAction::SponsorDeadline => {
                            sponsor_deadlines.push(
                                SponsorAdmission::try_from_record(aggregate)
                                    .ok_or(SpaceAdmissionStateStoreError::Corrupt)?,
                            )
                        }
                        RecoveryAction::SponsorAbandonment => sponsor_abandonments.push(
                            SponsorAdmission::try_from_record(aggregate)
                                .ok_or(SpaceAdmissionStateStoreError::Corrupt)?,
                        ),
                        // 旧实验 Helper 没有双方认可的共同期限，只保留记录，不臆造到期动作。
                        RecoveryAction::CompletionHelper | RecoveryAction::None => {}
                    }
                }
                cursor = rows.last().map(|row| row.lookup_token.clone());
                if rows.len() < RECOVERY_INDEX_BATCH_SIZE as usize {
                    break;
                }
            }
            Ok(LoadedRecoveryIndex {
                joiners,
                sponsor_deadlines,
                sponsor_abandonments,
                next_deadline_ms,
                sponsor_confirmation_pending,
            })
        })
    }

    fn open_recovery_summary(
        &self,
        row: &RecoverySummaryRow,
    ) -> Result<Option<RecoverySummary>, SpaceAdmissionStateStoreError> {
        let Some(encrypted) = row.encrypted_recovery_summary.as_ref() else {
            return Ok(None);
        };
        let plaintext = self
            .keys
            .profile_payload_reader(&row.purpose())
            .map_err(map_key_error)?
            .open_compact(encrypted)
            .map_err(map_key_error)?;
        if let Ok(summary) = postcard::from_bytes::<RecoverySummary>(&plaintext) {
            if summary.marker == RECOVERY_SUMMARY_MARKER
                && summary.format_version == RECOVERY_SUMMARY_FORMAT_V2
            {
                return Ok(Some(summary));
            }
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        postcard::from_bytes::<LegacyRecoverySummaryV1>(&plaintext)
            .map(|_| None)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)
    }

    fn load_recovery_aggregate(
        &self,
        conn: &mut SqliteConnection,
        row: &RecoverySummaryRow,
    ) -> Result<SpaceAdmissionAggregate, SpaceAdmissionStateStoreError> {
        let record_row = sql_query(
            "SELECT lookup_token, content_token, encrypted_payload \
             FROM admission_repository_record WHERE lookup_token = ? AND content_token = ?",
        )
        .bind::<Binary, _>(&row.lookup_token)
        .bind::<Binary, _>(&row.content_token)
        .get_result::<EncryptedRecordRow>(conn)?;
        let record = self.open_v3_record_row(record_row)?;
        self.open_record(record.admission_id, &record.stored)
    }

    fn save_recovery_summary(
        &self,
        conn: &mut SqliteConnection,
        row: &RecoverySummaryRow,
        summary: &RecoverySummary,
    ) -> Result<(), SpaceAdmissionStateStoreError> {
        let plaintext =
            postcard::to_stdvec(summary).map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let encrypted = self
            .keys
            .seal_profile_payload_compact(&row.purpose(), &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO admission_recovery_summary \
             (lookup_token, content_token, encrypted_payload) VALUES (?, ?, ?) \
             ON CONFLICT(lookup_token) DO UPDATE SET \
             content_token = excluded.content_token, encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(&row.lookup_token)
        .bind::<Binary, _>(&row.content_token)
        .bind::<Binary, _>(encrypted)
        .execute(conn)?;
        Ok(())
    }
}

fn load_summary_batch(
    conn: &mut SqliteConnection,
    cursor: Option<&[u8]>,
) -> Result<Vec<RecoverySummaryRow>, SpaceAdmissionStateStoreError> {
    let query = "SELECT record.lookup_token, record.content_token, \
                 summary.encrypted_payload AS encrypted_recovery_summary \
                 FROM admission_repository_record AS record \
                 LEFT JOIN admission_recovery_summary AS summary \
                 ON summary.lookup_token = record.lookup_token \
                 AND summary.content_token = record.content_token ";
    match cursor {
        Some(cursor) => sql_query(format!(
            "{query} WHERE record.lookup_token > ? ORDER BY record.lookup_token LIMIT ?"
        ))
        .bind::<Binary, _>(cursor)
        .bind::<Integer, _>(RECOVERY_INDEX_BATCH_SIZE)
        .load(conn)
        .map_err(Into::into),
        None => sql_query(format!("{query} ORDER BY record.lookup_token LIMIT ?"))
            .bind::<Integer, _>(RECOVERY_INDEX_BATCH_SIZE)
            .load(conn)
            .map_err(Into::into),
    }
}
