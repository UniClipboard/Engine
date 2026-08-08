//! 成员移除协调器测试用的内存端口实现。
//!
//! 这些 fake 不涉及密码学与 OpenMLS:签名与验证被替换为可配置的
//! 占位逻辑,恢复资料由 fake 直接构造。它们用于验证协调器的流程语义
//! (创建、验证、去重、合并、状态推进、恢复编排、幂等与失败路径)。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, MemberInstanceId,
    MemberRepositoryPort, MemberSyncPreferences, MembershipError, RemovalCausalProof,
    RemovalCausalProofMember, RemovalExchangeError, RemovalExchangeMessage, RemovalExchangePort,
    RemovalIntentId, RemovalIntentRepositoryError, RemovalIntentRepositoryPort,
    RemovalIntentVerificationError, RemovalIntentVerificationPort, RemovalLateAcceptance,
    RemovalLateSubmission, RemovalLateSubmissionPort, RemovalLateSubmissionTransportError,
    RemovalPersistedState, RemovalPreparedRecovery, RemovalRecoveryError, RemovalRecoveryMaterial,
    RemovalRecoveryPort, RemovalViewMember, RemovalViewSnapshot, SignedRemovalIntent, SpaceMember,
};
use uc_core::security::IdentityFingerprint;

/// 内存版意图仓库。
#[derive(Clone, Default)]
pub struct MemoryRemovalIntentRepository {
    state: Arc<Mutex<Option<RemovalPersistedState>>>,
    lineage: Arc<Mutex<String>>,
    atomic_save_count: Arc<Mutex<usize>>,
    save_new_failure: Arc<Mutex<Option<RemovalIntentRepositoryError>>>,
}

impl MemoryRemovalIntentRepository {
    pub fn with_lineage(lineage: &str) -> Self {
        Self {
            state: Arc::new(Mutex::new(None)),
            lineage: Arc::new(Mutex::new(lineage.to_owned())),
            atomic_save_count: Arc::new(Mutex::new(0)),
            save_new_failure: Arc::new(Mutex::new(None)),
        }
    }

    pub fn atomic_save_count(&self) -> usize {
        *self.atomic_save_count.lock().unwrap()
    }

    pub fn fail_next_new_intent_save(&self, error: RemovalIntentRepositoryError) {
        *self.save_new_failure.lock().unwrap() = Some(error);
    }
}

#[async_trait]
impl RemovalIntentRepositoryPort for MemoryRemovalIntentRepository {
    async fn current_space_lineage(&self) -> Result<String, RemovalIntentRepositoryError> {
        Ok(self.lineage.lock().unwrap().clone())
    }

    async fn save_new_intent_state(
        &self,
        intent: &SignedRemovalIntent,
        state: &RemovalPersistedState,
    ) -> Result<bool, RemovalIntentRepositoryError> {
        if let Some(error) = self.save_new_failure.lock().unwrap().take() {
            return Err(error);
        }
        if !state
            .intents
            .iter()
            .any(|known| known.intent_id == intent.intent_id)
            || !state.locally_removed.contains(&intent.content.target)
        {
            return Err(RemovalIntentRepositoryError::Corrupt);
        }
        let mut saved = self.state.lock().unwrap();
        if saved.as_ref().is_some_and(|current| {
            current
                .intents
                .iter()
                .any(|known| known.intent_id == intent.intent_id)
        }) {
            return Ok(false);
        }
        *saved = Some(state.clone());
        *self.atomic_save_count.lock().unwrap() += 1;
        Ok(true)
    }

    async fn save_state(
        &self,
        state: &RemovalPersistedState,
    ) -> Result<(), RemovalIntentRepositoryError> {
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }

    async fn load_state(
        &self,
    ) -> Result<Option<RemovalPersistedState>, RemovalIntentRepositoryError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

/// 可配置的验证端口。
#[derive(Clone, Default)]
pub struct ConfigurableVerifier {
    /// 验证失败时的固定错误;`None` 表示全部通过。
    pub failure: Arc<Mutex<Option<RemovalIntentVerificationError>>>,
}

#[async_trait]
impl RemovalIntentVerificationPort for ConfigurableVerifier {
    async fn verify_intent(
        &self,
        _intent: &SignedRemovalIntent,
    ) -> Result<(), RemovalIntentVerificationError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            Err(error)
        } else {
            Ok(())
        }
    }
}

/// 内存版交换端口:记录发出的消息,并允许测试把消息投递给另一端。
#[derive(Clone, Default)]
pub struct MemoryRemovalExchange {
    /// 发出的消息列表(按发送顺序)。
    pub sent: Arc<Mutex<Vec<(DeviceId, RemovalExchangeMessage)>>>,
    /// 下一次完成通知发送失败，用于验证执行者会保留并重试完成通知。
    pub fail_next_complete: Arc<Mutex<bool>>,
}

impl MemoryRemovalExchange {
    pub fn fail_next_complete(&self) {
        *self.fail_next_complete.lock().unwrap() = true;
    }
}

#[async_trait]
impl RemovalExchangePort for MemoryRemovalExchange {
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError> {
        if matches!(&message, RemovalExchangeMessage::Complete { .. })
            && std::mem::take(&mut *self.fail_next_complete.lock().unwrap())
        {
            return Err(RemovalExchangeError::Offline);
        }
        self.sent.lock().unwrap().push((recipient.clone(), message));
        Ok(RemovalExchangeMessage::IntentAck(
            RemovalIntentId::from_bytes([0; 32]),
        ))
    }
}

/// 内存版受限迟交通道：只记录历史提交并返回有界接收结果。
#[derive(Clone, Default)]
pub struct MemoryRemovalLateExchange {
    pub sent: Arc<Mutex<Vec<(DeviceId, RemovalLateSubmission)>>>,
}

#[async_trait]
impl RemovalLateSubmissionPort for MemoryRemovalLateExchange {
    async fn submit_late(
        &self,
        recipient: &DeviceId,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionTransportError> {
        let intent_id = match &submission {
            RemovalLateSubmission::Intent(intent) => intent.intent_id,
        };
        self.sent
            .lock()
            .unwrap()
            .push((recipient.clone(), submission));
        Ok(RemovalLateAcceptance::Accepted { intent_id })
    }
}

/// 假签名端口:对任何 payload 返回固定签名。
#[derive(Clone, Default)]
pub struct FixedSigner {
    pub signature: Arc<Mutex<Vec<u8>>>,
}

#[async_trait]
impl CurrentMemberSignaturePort for FixedSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(self.signature.lock().unwrap().clone())
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(true)
    }
}

/// 假恢复端口:维护成员集合与设备映射,生成假恢复资料。
#[derive(Clone)]
pub struct FakeRemovalRecovery {
    /// 当前视图成员(设备 → 实例)。
    pub members: Arc<Mutex<Vec<(DeviceId, MemberInstanceId)>>>,
    pub epoch: u64,
    /// 本机设备(用于 own_instance)。
    pub own_device: DeviceId,
    /// 每次生成的 key package 内容。
    pub key_package_counter: Arc<Mutex<u64>>,
    /// 拒绝应用恢复资料时的错误。
    pub apply_failure: Arc<Mutex<Option<RemovalRecoveryError>>>,
    /// 已应用的恢复资料(供断言)。
    pub applied: Arc<Mutex<Vec<RemovalRecoveryMaterial>>>,
    /// 已准备的执行者私有检查点。
    pub prepared_checkpoints: Arc<Mutex<Vec<Vec<u8>>>>,
    /// 安装执行者私有检查点时的可注入失败。
    pub install_failure: Arc<Mutex<Option<RemovalRecoveryError>>>,
    /// 已安装的执行者私有检查点。
    pub installed_checkpoints: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl FakeRemovalRecovery {
    pub fn new(own_device: DeviceId, members: Vec<(DeviceId, MemberInstanceId)>) -> Self {
        Self {
            members: Arc::new(Mutex::new(members)),
            epoch: 1,
            own_device,
            key_package_counter: Arc::new(Mutex::new(0)),
            apply_failure: Arc::new(Mutex::new(None)),
            applied: Arc::new(Mutex::new(Vec::new())),
            prepared_checkpoints: Arc::new(Mutex::new(Vec::new())),
            install_failure: Arc::new(Mutex::new(None)),
            installed_checkpoints: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl RemovalRecoveryPort for FakeRemovalRecovery {
    async fn current_view(&self) -> Result<RemovalViewSnapshot, RemovalRecoveryError> {
        let members = self
            .members
            .lock()
            .unwrap()
            .iter()
            .map(|(device_id, instance)| RemovalViewMember {
                device_id: device_id.clone(),
                instance: *instance,
                signing_public_key: instance.as_bytes().to_vec(),
            })
            .collect::<Vec<_>>();
        let causal_proof = RemovalCausalProof::new(
            self.epoch,
            members
                .iter()
                .map(|member| RemovalCausalProofMember {
                    device_id: member.device_id.clone(),
                    instance: member.instance,
                    signing_public_key: member.signing_public_key.clone(),
                })
                .collect(),
        );
        Ok(RemovalViewSnapshot {
            epoch: self.epoch,
            members,
            causal_proof,
        })
    }

    async fn own_instance(&self) -> Result<Option<MemberInstanceId>, RemovalRecoveryError> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .find(|(device_id, _)| *device_id == self.own_device)
            .map(|(_, instance)| *instance))
    }

    async fn prepare_key_package(&self) -> Result<Vec<u8>, RemovalRecoveryError> {
        let mut counter = self.key_package_counter.lock().unwrap();
        *counter += 1;
        Ok(format!("kp-{counter}").into_bytes())
    }

    async fn prepare_forward_recovery(
        &self,
        convergence_digest: &[u8; 32],
        effective_members: &[MemberInstanceId],
        key_packages: &[(MemberInstanceId, Vec<u8>)],
    ) -> Result<RemovalPreparedRecovery, RemovalRecoveryError> {
        let _ = key_packages;
        let local_checkpoint = b"fake-local-checkpoint".to_vec();
        self.prepared_checkpoints
            .lock()
            .unwrap()
            .push(local_checkpoint.clone());
        Ok(RemovalPreparedRecovery {
            material: RemovalRecoveryMaterial {
                convergence_digest: *convergence_digest,
                effective_members: effective_members.to_vec(),
                epoch: self.epoch + 1,
                commit: b"fake-commit".to_vec(),
                welcome: Some(b"fake-welcome".to_vec()),
                encrypted_key_catalog: b"fake-catalog".to_vec(),
            },
            local_checkpoint,
        })
    }

    async fn install_prepared_forward_recovery(
        &self,
        local_checkpoint: &[u8],
    ) -> Result<(), RemovalRecoveryError> {
        if let Some(error) = self.install_failure.lock().unwrap().clone() {
            return Err(error);
        }
        self.installed_checkpoints
            .lock()
            .unwrap()
            .push(local_checkpoint.to_vec());
        Ok(())
    }

    async fn apply_forward_recovery(
        &self,
        material: &RemovalRecoveryMaterial,
        expected_convergence_digest: &[u8; 32],
        expected_effective_members: &[MemberInstanceId],
    ) -> Result<(), RemovalRecoveryError> {
        if let Some(error) = self.apply_failure.lock().unwrap().clone() {
            return Err(error);
        }
        if material.convergence_digest != *expected_convergence_digest {
            return Err(RemovalRecoveryError::InvalidMaterial);
        }
        let mut actual = material.effective_members.clone();
        actual.sort_unstable();
        let mut expected = expected_effective_members.to_vec();
        expected.sort_unstable();
        if actual != expected {
            return Err(RemovalRecoveryError::InvalidMaterial);
        }
        self.applied.lock().unwrap().push(material.clone());
        Ok(())
    }
}

/// 内存版成员仓库。
#[derive(Clone, Default)]
pub struct MemoryMemberRepository {
    members: Arc<Mutex<Vec<SpaceMember>>>,
}

impl MemoryMemberRepository {
    pub fn add(&self, device_id: DeviceId) {
        let fingerprint = IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap();
        self.members.lock().unwrap().push(SpaceMember {
            device_id,
            device_name: String::new(),
            identity_fingerprint: fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        });
    }
}

#[async_trait]
impl MemberRepositoryPort for MemoryMemberRepository {
    async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .find(|member| member.device_id == *device_id)
            .cloned())
    }

    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        Ok(self.members.lock().unwrap().clone())
    }

    async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
        let mut members = self.members.lock().unwrap();
        if let Some(existing) = members
            .iter_mut()
            .find(|existing| existing.device_id == member.device_id)
        {
            *existing = member.clone();
        } else {
            members.push(member.clone());
        }
        Ok(())
    }

    async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
        let mut members = self.members.lock().unwrap();
        let before = members.len();
        members.retain(|member| member.device_id != *device_id);
        Ok(members.len() != before)
    }
}
