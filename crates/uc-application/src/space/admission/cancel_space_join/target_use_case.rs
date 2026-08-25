use std::sync::Arc;

use uc_core::membership::{
    AdmissionOutboxPurpose, AdmissionRejectionReason, AdmissionTerminalResult, JoinerAdmissionStage,
};

use crate::space::admission::CurrentJoinStatus;
use crate::space::membership_ledger::{MembershipLedger, MembershipLedgerError};
use crate::space::query_device_trust::project_current_join;
use crate::space::remove_space_member::WakeSpaceMembershipMaintenancePort;

use super::CancelSpaceJoinError;

pub(crate) struct CancelSpaceJoinUseCase {
    ledger: Arc<MembershipLedger>,
    maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    execution_lock: tokio::sync::Mutex<()>,
}

impl CancelSpaceJoinUseCase {
    pub(crate) fn new(
        ledger: Arc<MembershipLedger>,
        maintenance: Arc<dyn WakeSpaceMembershipMaintenancePort>,
    ) -> Self {
        Self {
            ledger,
            maintenance,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let _guard = self.execution_lock.lock().await;
        let mut record = self
            .ledger
            .current_local_admission_record()
            .await
            .map_err(map_ledger_error)?
            .filter(|record| record.join_id == Some(join_id))
            .ok_or(CancelSpaceJoinError::NotFound)?;
        if record.is_terminal() || record.stage_rank().is_some_and(|stage| stage >= 4) {
            return self.query_current_status().await;
        }
        let recipient = record
            .outboxes
            .iter()
            .find(|message| message.purpose == AdmissionOutboxPurpose::JoinRequest)
            .map(|message| message.recipient.clone())
            .ok_or_else(|| {
                CancelSpaceJoinError::State("pending join has no cancellation route".to_owned())
            })?;
        let predecessor = record
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .map(|message| message.message_id);
        for message in &mut record.outboxes {
            message.superseded = true;
        }
        let cancel = crate::space::admission::outbox::message(
            record.record_id,
            AdmissionOutboxPurpose::CancelRequested,
            &recipient,
            predecessor,
            b"cancel_requested",
        );
        record.cancel_request = Some(b"cancel_requested".to_vec());
        record.outboxes.push(cancel);
        record.terminal_result = Some(AdmissionTerminalResult::Rejected);
        record.rejection_reason = Some(AdmissionRejectionReason::Cancelled);
        if !record.set_joiner_stage(JoinerAdmissionStage::Rejected) {
            return Err(CancelSpaceJoinError::State(
                "current join role is invalid".to_owned(),
            ));
        }
        let expected_version = record.record_version;
        record.record_version = expected_version
            .checked_add(1)
            .ok_or_else(|| CancelSpaceJoinError::State("join version overflow".to_owned()))?;
        self.ledger
            .advance_admission_record(record.record_id, expected_version, record)
            .await
            .map_err(map_ledger_error)?;
        self.maintenance.wake();
        self.query_current_status().await
    }

    async fn query_current_status(&self) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let snapshot = self
            .ledger
            .load_verified()
            .await
            .map_err(map_ledger_error)?;
        project_current_join(snapshot.record())
            .map_err(|error| CancelSpaceJoinError::State(error.to_string()))?
            .ok_or(CancelSpaceJoinError::NotFound)
    }
}

fn map_ledger_error(error: MembershipLedgerError) -> CancelSpaceJoinError {
    CancelSpaceJoinError::State(error.to_string())
}
