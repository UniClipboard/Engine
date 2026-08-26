use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::*;

use super::admission::*;
use super::application::{SpaceApplication, SpaceApplicationDeps};
use super::membership::*;

struct MemoryLedger(Mutex<LoadedMembershipLedger>);

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(self.0.lock().unwrap().clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedger {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut loaded = self.0.lock().unwrap();
        if loaded.revision != mutation.expected_revision {
            return Err(MembershipLedgerError::Conflict);
        }
        *loaded = mutation.replacement;
        Ok(loaded.clone())
    }
}

struct PassivePorts;

impl HistoricalMembershipSignatureVerifier for PassivePorts {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

#[async_trait]
impl CurrentMemberSignaturePort for PassivePorts {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn current_member_instance(
        &self,
        _device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        unreachable!()
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        unreachable!()
    }
}

#[async_trait]
impl CurrentMembershipIdentityPort for PassivePorts {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        unreachable!()
    }
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for PassivePorts {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        unreachable!()
    }

    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        unreachable!()
    }
}

impl uc_core::ports::DeviceIdentityPort for PassivePorts {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("local")
    }
}

impl uc_core::ports::ClockPort for PassivePorts {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

#[async_trait]
impl uc_core::ports::SettingsPort for PassivePorts {
    async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
        Ok(uc_core::settings::model::Settings::default())
    }

    async fn save(&self, _settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl GroupBootstrapPort for PassivePorts {
    async fn bootstrap_legacy_space(
        &self,
        _sponsor: &DeviceId,
        _retained_members: &[DeviceId],
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn acknowledge_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn withdraw_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<GroupBootstrapResult, BootstrapError> {
        unreachable!()
    }

    async fn query_legacy_bootstrap(
        &self,
        _bootstrap_id: &BootstrapId,
    ) -> Result<Option<GroupBootstrapResult>, BootstrapError> {
        unreachable!()
    }

    async fn resume_legacy_bootstraps(
        &self,
        _now_ms: i64,
    ) -> Result<Vec<GroupBootstrapResult>, BootstrapError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareJoinSpacePort for PassivePorts {
    async fn prepare(&self, _input: &JoinSpaceInput) -> Result<PreparedJoinSpace, JoinSpaceError> {
        unreachable!()
    }
}

#[async_trait]
impl PrepareSpaceAdmissionMessagePort for PassivePorts {
    async fn prepare(
        &self,
        _message: &AuthenticatedSpaceAdmissionMessage,
        _context: &SpaceAdmissionPreparationContext,
    ) -> Result<PreparedSpaceAdmissionMessage, HandleSpaceAdmissionMessageError> {
        unreachable!()
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for PassivePorts {
    async fn load(
        &self,
        _device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for PassivePorts {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        unreachable!()
    }
}

#[async_trait]
impl AdmissionOutboxDeliveryPort for PassivePorts {
    async fn deliver(
        &self,
        _attempt_id: SpaceJoinRecordId,
        _message: &AdmissionOutboxMessage,
        _route: Option<&AdmissionOutboxDeliveryRoute>,
    ) -> Result<AdmissionOutboxDeliveryResult, AdmissionOutboxDeliveryError> {
        unreachable!()
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for PassivePorts {
    async fn prepare_if_needed(
        &self,
        _input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        unreachable!()
    }

    async fn advance(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        unreachable!()
    }

    async fn discard_pre_activation(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        unreachable!()
    }
}

#[async_trait]
impl ApplyMembershipMemberFactsPort for PassivePorts {
    async fn apply_member_facts(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for PassivePorts {
    async fn apply_membership_security(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for PassivePorts {
    async fn activate_membership_effect(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        unreachable!()
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for PassivePorts {
    async fn deliver_restricted_membership(
        &self,
        _peer: &DeviceId,
        _delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        unreachable!()
    }
}

#[async_trait]
impl CleanupLegacyMembershipDataPort for PassivePorts {
    async fn cleanup_legacy_membership_data(&self) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Completed
    }
}

impl MembershipNetworkActivityPort for PassivePorts {
    fn pause_network_work(&self) {}
    fn resume_network_work(&self) {}
}

#[async_trait]
impl ResolveRePairingPort for PassivePorts {
    async fn resolve_after_successful_pairing(&self) -> Result<(), RePairingStateError> {
        Ok(())
    }
}

#[tokio::test]
async fn complete_application_starts_from_only_target_ports() {
    let mut initial = LoadedMembershipLedger::no_current_space();
    initial.admission_profile = Some(AdmissionProfileMetadata::fresh([0x71; 16]));
    let repository = Arc::new(MemoryLedger(Mutex::new(initial)));
    let passive = Arc::new(PassivePorts);
    let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
    let application = SpaceApplication::start(
        SpaceApplicationDeps {
            load_membership_ledger: repository.clone(),
            commit_membership_ledger: repository,
            historical_membership_signatures: passive.clone(),
            current_member_signatures: passive.clone(),
            membership_identity: passive.clone(),
            membership_announcement: passive.clone(),
            device_identity: passive.clone(),
            group_bootstrap: passive.clone(),
            clock: passive.clone(),
            settings: passive.clone(),
            prepare_join_space: passive.clone(),
            prepare_space_admission_message: passive.clone(),
            device_trust_observations: passive.clone(),
            membership_history_transport: passive.clone(),
            admission_outbox_delivery: passive.clone(),
            admission_space_transition: passive.clone(),
            apply_membership_member_facts: passive.clone(),
            apply_membership_security: passive.clone(),
            activate_membership_effect: passive.clone(),
            restricted_membership_delivery: passive.clone(),
            cleanup_legacy_membership_data: passive.clone(),
            membership_network_activity: passive.clone(),
        },
        presence_rx,
        Arc::new(InMemoryPairingInvitationHolder::new()),
        passive,
    );

    let _ = application.membership_history_endpoint();
    let _ = application.space_admission_endpoint();
    application.shutdown().await;
}
