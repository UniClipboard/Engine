use std::sync::Arc;

use uc_core::membership::AdmissionOutboxPurpose;

use crate::space::admission::CurrentJoinStatus;
use crate::space::membership::project_current_join;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{MembershipLedger, MembershipLedgerError};

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
            .current_local_join_record()
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
        let cancel = crate::space::admission::outbox::message(
            record.record_id,
            AdmissionOutboxPurpose::CancelRequested,
            &recipient,
            predecessor,
            b"cancel_requested",
        );
        record = record
            .cancelled(cancel)
            .map_err(|error| CancelSpaceJoinError::State(error.to_string()))?;
        self.ledger
            .save_join_record_progress(record)
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
