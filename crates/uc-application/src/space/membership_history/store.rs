use std::sync::Arc;

use uc_core::membership::{
    HistoricalMembershipSignatureVerifier, MemberInstanceId, MembershipDecisionStoreOutcome,
    MembershipDecisionV2, MembershipEventV2, MembershipHistoryV2ReceiveOutcome,
    VersionedMembershipHistory,
};

use super::{MembershipHistoryRepositoryError, MembershipHistoryRepositoryPort};

pub(crate) struct MembershipHistoryStore {
    repository: Arc<dyn MembershipHistoryRepositoryPort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

pub(crate) struct LoadedMembershipHistory {
    expected_bytes: Vec<u8>,
    versioned_history: VersionedMembershipHistory,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

pub(crate) struct CommittedMembershipHistory {
    versioned_history: VersionedMembershipHistory,
    revision: u64,
}

impl MembershipHistoryStore {
    pub(crate) fn new(
        repository: Arc<dyn MembershipHistoryRepositoryPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            repository,
            verifier,
        }
    }

    pub(crate) async fn load_verified_history(
        &self,
    ) -> Result<Option<LoadedMembershipHistory>, MembershipHistoryRepositoryError> {
        let expected_bytes = match self.repository.load_membership_history().await? {
            Some(bytes) => bytes,
            None => return Ok(None),
        };
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &expected_bytes,
            self.verifier.as_ref(),
        )
        .map_err(|_| MembershipHistoryRepositoryError::Corrupt)?;
        Ok(Some(LoadedMembershipHistory {
            expected_bytes,
            versioned_history: history,
            verifier: Arc::clone(&self.verifier),
        }))
    }

    pub(crate) async fn commit(
        &self,
        loaded: LoadedMembershipHistory,
    ) -> Result<CommittedMembershipHistory, MembershipHistoryRepositoryError> {
        let replacement_bytes = loaded
            .versioned_history
            .encode_persisted_v2()
            .map_err(|_| MembershipHistoryRepositoryError::Corrupt)?;
        let revision = self
            .repository
            .compare_and_replace_membership_history(
                Some(&loaded.expected_bytes),
                &replacement_bytes,
            )
            .await?;
        Ok(CommittedMembershipHistory {
            versioned_history: loaded.versioned_history,
            revision,
        })
    }
}

impl CommittedMembershipHistory {
    pub(crate) fn history(&self) -> &VersionedMembershipHistory {
        &self.versioned_history
    }

    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn into_history(self) -> VersionedMembershipHistory {
        self.versioned_history
    }
}

impl LoadedMembershipHistory {
    pub(crate) fn history(&self) -> &VersionedMembershipHistory {
        &self.versioned_history
    }

    pub(crate) fn apply_signed_event(
        &mut self,
        event: MembershipEventV2,
    ) -> Result<MembershipHistoryV2ReceiveOutcome, MembershipHistoryRepositoryError> {
        self.versioned_history
            .verify_and_receive_event(event, self.verifier.as_ref())
            .map_err(|_| MembershipHistoryRepositoryError::Corrupt)
    }

    pub(crate) fn apply_signed_local_removal_decision(
        &mut self,
        decision: MembershipDecisionV2,
        local_member: MemberInstanceId,
    ) -> Result<MembershipDecisionStoreOutcome, MembershipHistoryRepositoryError> {
        self.versioned_history
            .apply_signed_local_removal_decision(decision, local_member, self.verifier.as_ref())
            .map_err(|_| MembershipHistoryRepositoryError::Corrupt)
    }

    pub(crate) fn into_history(self) -> VersionedMembershipHistory {
        self.versioned_history
    }
}
