//! 网络准入直接读取已验证成员账本，不借助无版本来源的快照缓存。
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, PeerAdmissionError, PeerAdmissionPort,
};

use super::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation, VerifiedMembershipLedger,
};

struct ReadOnlyCommitter;

#[async_trait]
impl CommitMembershipLedgerPort for ReadOnlyCommitter {
    async fn compare_and_commit(
        &self,
        _: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

pub fn build_membership_peer_admission(
    loader: Arc<dyn LoadMembershipLedgerPort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
) -> Arc<dyn PeerAdmissionPort> {
    Arc::new(MembershipLedger::new(
        loader,
        Arc::new(ReadOnlyCommitter),
        verifier,
    ))
}

#[async_trait]
impl PeerAdmissionPort for MembershipLedger {
    async fn is_admitted(&self, device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
        let record = self.loader.load().await.map_err(admission_error)?;
        let cached = self.cached_verified().map_err(admission_error)?;
        let history = self
            .validate_loaded(&record, cached.as_deref())
            .map_err(admission_error)?;
        let snapshot = VerifiedMembershipLedger { record, history };
        self.cache_verified(&snapshot).map_err(admission_error)?;
        Ok(snapshot.admits_peer(device_id))
    }
}

fn admission_error(error: MembershipLedgerError) -> PeerAdmissionError {
    match error {
        MembershipLedgerError::Locked
        | MembershipLedgerError::Conflict
        | MembershipLedgerError::Unavailable => PeerAdmissionError::Unavailable,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            PeerAdmissionError::InvalidState
        }
    }
}
