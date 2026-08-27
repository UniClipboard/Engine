use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use uc_core::membership::{
    AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature, AdmissionJoinRequestV1,
    AdmissionKeyPackage, AdmissionMessageId, AdmissionRecoveryPublicKey, AdmissionRole,
    AdmissionSourceSnapshot, InvitationId, JoinId, MembershipCredential, SpaceAdmissionAggregate,
    SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute,
    UnreadableHistoryPolicy, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::SettingsPort;
use uc_core::DeviceId;

use super::ports::{JoinerStartStateError, JoinerStartStatePort};
use super::{
    JoinerStartMaterial, JoinerStartMaterialError, JoinerStartMaterialPort, JoinerStartMutation,
    LoadedJoinerStartState, SpaceAdmissionCommitToken, SpaceAdmissionProtocol,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtocolEvent {
    DeviceNameSaved,
    JoinerSavedJoinRequest,
}

pub(super) struct SpaceAdmissionProtocolTestPair {
    joiner: SpaceAdmissionProtocol,
    state: Arc<RecordingJoinerStartState>,
}

struct FixedJoinerStartMaterial;

struct RecordingJoinerStartState {
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
    current_join: Mutex<Option<SpaceAdmissionAggregate>>,
    created_join: Mutex<Option<SpaceAdmissionAggregate>>,
    superseded: AtomicBool,
}

struct RecordingSettings {
    value: Mutex<uc_core::settings::model::Settings>,
    events: Arc<Mutex<Vec<ProtocolEvent>>>,
}

#[async_trait]
impl SettingsPort for RecordingSettings {
    async fn load(&self) -> anyhow::Result<uc_core::settings::model::Settings> {
        Ok(self.value.lock().expect("settings are available").clone())
    }

    async fn save(&self, settings: &uc_core::settings::model::Settings) -> anyhow::Result<()> {
        *self.value.lock().expect("settings are available") = settings.clone();
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::DeviceNameSaved);
        Ok(())
    }
}

#[async_trait]
impl JoinerStartStatePort for RecordingJoinerStartState {
    async fn load(&self) -> Result<LoadedJoinerStartState, JoinerStartStateError> {
        Ok(LoadedJoinerStartState::new(
            7,
            AdmissionSourceSnapshot::from_bytes(vec![0x24; 32]).expect("valid source snapshot"),
            self.current_join
                .lock()
                .expect("current join is available")
                .take(),
            true,
            SpaceAdmissionCommitToken::from_bytes([0x25; 32]).expect("valid commit token"),
        ))
    }

    async fn commit(
        &self,
        token: SpaceAdmissionCommitToken,
        mutation: JoinerStartMutation,
    ) -> Result<(), JoinerStartStateError> {
        assert_eq!(token.as_bytes(), &[0x25; 32]);
        let (created, superseded) = mutation.into_parts();
        assert_eq!(created.record_version(), 0);
        self.superseded
            .store(superseded.is_some(), Ordering::SeqCst);
        *self.created_join.lock().expect("created join is available") =
            Some(created.into_replacement());
        self.events
            .lock()
            .expect("event recorder is available")
            .push(ProtocolEvent::JoinerSavedJoinRequest);
        Ok(())
    }
}

impl SpaceAdmissionProtocolTestPair {
    pub(super) async fn fresh() -> Self {
        Self::with_current_join(None).await
    }

    pub(super) async fn with_current_join(current_join: Option<SpaceAdmissionAggregate>) -> Self {
        let events = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(RecordingJoinerStartState {
            events: Arc::clone(&events),
            current_join: Mutex::new(current_join),
            created_join: Mutex::new(None),
            superseded: AtomicBool::new(false),
        });
        Self {
            joiner: SpaceAdmissionProtocol::new(
                Arc::new(RecordingSettings {
                    value: Mutex::new(Default::default()),
                    events: Arc::clone(&events),
                }),
                Arc::new(FixedJoinerStartMaterial),
                state.clone(),
            ),
            state,
        }
    }

    pub(super) fn joiner(&self) -> &SpaceAdmissionProtocol {
        &self.joiner
    }

    pub(super) fn events(&self) -> Vec<ProtocolEvent> {
        self.state.events.lock().unwrap().clone()
    }

    pub(super) fn take_created_join(&self) -> SpaceAdmissionAggregate {
        self.state
            .created_join
            .lock()
            .expect("created join is available")
            .take()
            .expect("one join was committed")
    }

    pub(super) fn superseded_previous_join(&self) -> bool {
        self.state.superseded.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JoinerStartMaterialPort for FixedJoinerStartMaterial {
    async fn create(
        &self,
        input: &crate::space::admission::JoinSpaceInput,
    ) -> Result<JoinerStartMaterial, JoinerStartMaterialError> {
        let admission_id = SpaceAdmissionId::from_bytes([0x11; 32]).expect("valid admission id");
        let join_id = JoinId::from_bytes([0x12; 16]).expect("valid join id");

        let request = AdmissionJoinRequestV1::new(
            InvitationId::from_bytes([0x13; 32]).expect("valid invitation id"),
            DeviceId::new("joining-device"),
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x14; 32]),
            AdmissionKeyPackage::from_bytes(vec![0x15; 48]).expect("valid key package"),
            AdmissionRecoveryPublicKey::from_bytes([0x16; 32]).expect("valid recovery public key"),
            AdmissionIdentitySignature::from_bytes(vec![0x17; 64])
                .expect("valid identity signature"),
            if input.preserve_unreadable_history {
                UnreadableHistoryPolicy::Preserve
            } else {
                UnreadableHistoryPolicy::Discard
            },
        )
        .expect("valid join request");
        let join_request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            AdmissionMessageId::from_bytes([0x18; 32]).expect("valid message id"),
            None,
            SpaceAdmissionBodyV1::JoinRequest(request),
        )
        .expect("valid join request envelope");

        Ok(JoinerStartMaterial::new(
            admission_id,
            join_id,
            SpaceAdmissionRoute::from_bytes(vec![0x19; 32]).expect("valid route"),
            join_request,
            AdmissionEncryptedPasswordEquivalent::from_bytes(vec![0x1a; 64])
                .expect("valid password material"),
        ))
    }
}
