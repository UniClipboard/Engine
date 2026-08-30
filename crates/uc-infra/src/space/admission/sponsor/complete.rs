use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    ActivateSponsorAdmissionError, ActivateSponsorAdmissionPort,
    ActivateSponsorAdmissionSecurityPort, ActivateSponsorAdmissionSecurityRequest,
    ApplyMembershipMemberFactsPort, CommitMembershipLedgerPort, CurrentMemberSignaturePort,
    LoadMembershipLedgerPort, MembershipEffectKind, MembershipEffectPhase,
    MembershipLedgerMutation, PeerHistorySyncState, PeerReconciliationRecord,
    PendingMembershipEffect, PrepareSponsorCompleteError, PrepareSponsorCompletePort,
    PreparedSponsorComplete,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivatedSecurityState, AdmissionActivationReceipt, AdmissionCompleteV1,
    AdmissionCompletionV1, HistoricalMembershipSignatureVerifier, MembershipHistoryRelationship,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SponsorCompletePreparation,
    VersionedMembershipHistory,
};

use super::candidate::SponsorCandidateStagedV1;

const SPONSOR_ACTIVATED_SECURITY_FORMAT_V1: u16 = 1;

pub struct DefaultSponsorCompletePreparation {
    local_device_id: DeviceId,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
}

impl DefaultSponsorCompletePreparation {
    pub fn new(
        local_device_id: DeviceId,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        Self {
            local_device_id,
            signatures,
            history_verifier,
        }
    }
}

#[derive(Serialize)]
struct SponsorActivatedSecurityV1<'a> {
    format_version: u16,
    space_id: &'a str,
    staged_state: &'a [u8],
    commit: &'a [u8],
    expected_commitment: &'a uc_core::membership::AdmissionSecurityCommitmentV1,
    committed_history: &'a [u8],
    security_commitment_id: [u8; 32],
}

#[derive(Deserialize)]
struct OwnedSponsorActivatedSecurityV1 {
    format_version: u16,
    space_id: String,
    staged_state: Vec<u8>,
    commit: Vec<u8>,
    expected_commitment: uc_core::membership::AdmissionSecurityCommitmentV1,
    committed_history: Vec<u8>,
    security_commitment_id: [u8; 32],
}

pub struct DefaultSponsorAdmissionActivation {
    security: Arc<dyn ActivateSponsorAdmissionSecurityPort>,
    loader: Arc<dyn LoadMembershipLedgerPort>,
    committer: Arc<dyn CommitMembershipLedgerPort>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
}

impl DefaultSponsorAdmissionActivation {
    pub fn new(
        security: Arc<dyn ActivateSponsorAdmissionSecurityPort>,
        loader: Arc<dyn LoadMembershipLedgerPort>,
        committer: Arc<dyn CommitMembershipLedgerPort>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    ) -> Self {
        Self {
            security,
            loader,
            committer,
            verifier,
            member_facts,
        }
    }
}

#[async_trait]
impl ActivateSponsorAdmissionPort for DefaultSponsorAdmissionActivation {
    async fn activate(
        &self,
        activated_security: &AdmissionActivatedSecurityState,
    ) -> Result<(), ActivateSponsorAdmissionError> {
        self.activate_inner(activated_security)
            .await
            .map_err(ActivateSponsorAdmissionError::new)
    }
}

impl DefaultSponsorAdmissionActivation {
    async fn activate_inner(
        &self,
        activated_security: &AdmissionActivatedSecurityState,
    ) -> anyhow::Result<()> {
        let activated: OwnedSponsorActivatedSecurityV1 =
            postcard::from_bytes(activated_security.as_bytes())?;
        if activated.format_version != SPONSOR_ACTIVATED_SECURITY_FORMAT_V1
            || activated.expected_commitment.security_commitment_id
                != activated.security_commitment_id
        {
            anyhow::bail!("the Sponsor activation material is inconsistent");
        }
        let space_id = uc_core::ids::SpaceId::from_str(&activated.space_id);
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &activated.committed_history,
            self.verifier.as_ref(),
        )?;
        if history.lineage_id() != activated.space_id {
            anyhow::bail!("the Sponsor activation history has a different lineage");
        }
        self.security
            .activate_sponsor_admission_security(ActivateSponsorAdmissionSecurityRequest {
                space_id,
                staged_state: activated.staged_state,
                commit: activated.commit,
                expected_commitment: activated.expected_commitment,
            })
            .await
            .map_err(anyhow::Error::new)?;

        let event_id = history
            .current_position()?
            .event_id
            .ok_or_else(|| anyhow::anyhow!("the Sponsor activation history has no head"))?;
        let event = history
            .event(event_id)
            .ok_or_else(|| anyhow::anyhow!("the Sponsor activation event is unavailable"))?;
        let affected_device_ids = match &event.operation {
            uc_core::membership::MembershipOperationV2::AddDevice { admission } => {
                vec![admission.facts.device_id.clone()]
            }
            uc_core::membership::MembershipOperationV2::RemoveDevice { .. } => {
                anyhow::bail!("the Sponsor activation event is not an admission")
            }
        };
        self.member_facts
            .apply_member_facts(&PendingMembershipEffect {
                event_id: *event_id.as_bytes(),
                kind: MembershipEffectKind::AddDevice,
                phase: MembershipEffectPhase::Prepared,
                affected_device_ids,
                payload: postcard::to_stdvec(event)?,
            })
            .await
            .map_err(anyhow::Error::new)?;

        let mut ledger = self.loader.load().await.map_err(anyhow::Error::new)?;
        if ledger.membership_history.as_deref() == Some(activated.committed_history.as_slice()) {
            tracing::debug!("Sponsor 成员历史激活命中幂等提交");
            return Ok(());
        }
        if ledger.lineage_id.as_deref() != Some(activated.space_id.as_str()) {
            anyhow::bail!("the Sponsor membership ledger has a different lineage");
        }
        let local_device_id = ledger
            .local_device_id
            .clone()
            .ok_or_else(|| anyhow::anyhow!("the Sponsor membership ledger has no local device"))?;
        let mut previous_reconciliation = std::mem::take(&mut ledger.peer_reconciliation);
        let previous_peer_count = previous_reconciliation.len();
        ledger.peer_reconciliation = history
            .active_members()
            .into_iter()
            .filter_map(|member| history.admission_facts_for(member))
            .filter(|facts| facts.device_id != local_device_id)
            .map(|facts| {
                let previous = previous_reconciliation.remove(&facts.device_id);
                (
                    facts.device_id.clone(),
                    PeerReconciliationRecord {
                        peer_device_id: facts.device_id.clone(),
                        relationship: previous
                            .as_ref()
                            .map_or(MembershipHistoryRelationship::Consistent, |record| {
                                record.relationship
                            }),
                        // 本次提交产生了新的正式 head；旧 ACK 只证明旧目标，不能证明
                        // 对端已经拥有新成员。清空后由认证 ACK 重新推进水位。
                        confirmed_position: None,
                        sync_state: previous.as_ref().map_or_else(
                            || PeerHistorySyncState {
                                pending_since_revision: Some(ledger.revision.saturating_add(1)),
                                ..Default::default()
                            },
                            |record| record.sync_state.clone(),
                        ),
                        restricted_delivery: previous
                            .as_ref()
                            .map_or_else(Vec::new, |record| record.restricted_delivery.clone()),
                        updated_at_ms: previous.map_or(0, |record| record.updated_at_ms),
                    },
                )
            })
            .collect();
        let expected_revision = ledger.revision;
        let expected_history_digest = ledger
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        ledger.revision = expected_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("the Sponsor membership revision overflowed"))?;
        ledger.membership_history = Some(activated.committed_history);
        tracing::debug!(
            previous_peer_count,
            active_peer_count = ledger.peer_reconciliation.len(),
            "Sponsor 新历史已建立逐 peer 传播欠账"
        );
        self.committer
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision,
                expected_history_digest,
                replacement: ledger,
            })
            .await
            .map_err(anyhow::Error::new)?;
        Ok(())
    }
}

#[async_trait]
impl PrepareSponsorCompletePort for DefaultSponsorCompletePreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorCompletePreparation<'_>,
        applied: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorComplete, PrepareSponsorCompleteError> {
        let commit = match preparation.commit_reply().body() {
            SpaceAdmissionBodyV1::Commit(commit) => commit,
            _ => return Err(invalid("the saved Sponsor message is not Commit")),
        };
        let receipt = match applied.body() {
            SpaceAdmissionBodyV1::Applied(applied) => applied.activation_receipt(),
            _ => return Err(invalid("the Sponsor Complete input is not Applied")),
        };
        if applied.header().admission_id() != admission_id
            || applied.header().predecessor_message_id()
                != Some(preparation.commit_reply().header().message_id())
        {
            return Err(invalid("the Applied envelope is not bound to Commit"));
        }
        let candidate = commit.exact_candidate();
        if receipt.attempt_id != *admission_id.as_bytes()
            || receipt.event_id != candidate.candidate_event().event_id()
            || receipt.installed_security_commitment_id
                != candidate.security_commitment().security_commitment_id
        {
            return Err(invalid("the Applied receipt differs from Commit"));
        }
        let mut history = VersionedMembershipHistory::decode_persisted_v2(
            preparation.committed_history().as_bytes(),
            self.history_verifier.as_ref(),
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        history
            .verify_and_record_activation_receipt(receipt.clone(), self.history_verifier.as_ref())
            .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        let committed_history = history
            .encode_persisted_v2()
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let completed_position = history
            .current_position()
            .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;

        let staged: SponsorCandidateStagedV1 =
            postcard::from_bytes(preparation.sealed_security().as_bytes())
                .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        if staged.format_version != 1 {
            return Err(invalid("the sealed Sponsor security format is unsupported"));
        }
        let member_instance = self
            .signatures
            .current_member_instance(&self.local_device_id)
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let credential = self
            .signatures
            .current_membership_credential(&self.local_device_id)
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let mut completion = AdmissionCompletionV1::new(
            *admission_id.as_bytes(),
            receipt.event_id,
            activation_receipt_digest(receipt),
            receipt.installed_security_commitment_id,
            member_instance,
            credential.credential_id,
            completed_position,
            Vec::new(),
        );
        completion.signature = self
            .signatures
            .sign_current_member_payload(&completion.signing_payload())
            .await
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?;
        let complete_reply = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            uc_core::membership::AdmissionRole::Sponsor,
            2,
            mint_message_id(),
            Some(applied.header().message_id()),
            SpaceAdmissionBodyV1::Complete(AdmissionCompleteV1::new(completion)),
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        let activated_security = AdmissionActivatedSecurityState::from_bytes(
            postcard::to_stdvec(&SponsorActivatedSecurityV1 {
                format_version: SPONSOR_ACTIVATED_SECURITY_FORMAT_V1,
                space_id: &candidate.security_commitment().lineage_id,
                staged_state: &staged.staged_state,
                commit: candidate.mls_commit().as_bytes(),
                expected_commitment: candidate.security_commitment(),
                committed_history: &committed_history,
                security_commitment_id: receipt.installed_security_commitment_id,
            })
            .map_err(|error| PrepareSponsorCompleteError::unavailable(anyhow::Error::new(error)))?,
        )
        .map_err(|error| PrepareSponsorCompleteError::invalid(anyhow::Error::new(error)))?;
        Ok(PreparedSponsorComplete::new(
            activated_security,
            complete_reply,
        ))
    }
}

pub(crate) fn activation_receipt_digest(receipt: &AdmissionActivationReceipt) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-activation-receipt-digest/v1\0");
    hasher.update(receipt.signing_payload());
    hasher.update((receipt.signature.len() as u64).to_be_bytes());
    hasher.update(&receipt.signature);
    hasher.finalize().into()
}

fn invalid(message: &'static str) -> PrepareSponsorCompleteError {
    PrepareSponsorCompleteError::invalid(anyhow::anyhow!(message))
}

fn mint_message_id() -> uc_core::membership::AdmissionMessageId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = uc_core::membership::AdmissionMessageId::from_bytes(bytes) {
            return id;
        }
    }
}
