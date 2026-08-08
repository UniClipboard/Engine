//! 应用层分布式成员移除模块(ADR-015 唯一负责人)。
//!
//! `RemovalCoordinator` 完整负责移除意图的创建、签名、验证、去重、加密持久化、
//! 交换、合并、有效成员集合与收敛摘要计算、统一安全状态推进、重启恢复与
//! 后台重试。调用方只做三件事:提交一次目标成员移除、查询当前状态、订阅变化。
//!
//! 任何完成结论都以保留成员实际取得同一安全状态为准,不以发送成功或发起方
//! 返回为准;事件丢失后通过一次查询恢复完整视图。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use tokio::sync::Notify;
use tracing::{info, warn};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, MemberInstanceId, MemberRemovalSummary, MemberRepositoryPort,
    RemovalAdmissionDecision, RemovalAdmissionGatePort, RemovalCausalCheckpoint,
    RemovalCompletionReceipt, RemovalExchangeEndpointPort, RemovalExchangeError,
    RemovalExchangeMessage, RemovalExchangePort, RemovalIntentContent, RemovalIntentId,
    RemovalIntentRepositoryPort, RemovalIntentVerificationError, RemovalIntentVerificationPort,
    RemovalLateAcceptance, RemovalLateRejectionReason, RemovalLateSubmission,
    RemovalLateSubmissionEndpointPort, RemovalLateSubmissionError, RemovalLateSubmissionPort,
    RemovalLateSubmissionTransportError, RemovalPersistedState, RemovalPhase,
    RemovalRecoveryPersisted, RemovalRecoveryPort, RemovalTargetGatePort, SignedRemovalIntent,
};

pub use errors::RemovalCoordinatorError;
pub use runtime::MemberRemovalRuntime;

mod errors;
mod runtime;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

/// 协调器的依赖集合。
pub struct RemovalCoordinatorDeps {
    pub repository: Arc<dyn RemovalIntentRepositoryPort>,
    pub verification: Arc<dyn RemovalIntentVerificationPort>,
    pub exchange: Arc<dyn RemovalExchangePort>,
    pub late_submission: Arc<dyn RemovalLateSubmissionPort>,
    pub recovery: Arc<dyn RemovalRecoveryPort>,
    pub member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
}

/// 分布式成员移除协调器。
pub struct RemovalCoordinator {
    deps: RemovalCoordinatorDeps,
    wake: Arc<Notify>,
    state_lock: Arc<tokio::sync::Mutex<()>>,
}

/// 后台推进计划中的一条待发送消息。
enum Outgoing {
    /// 普通成员通道:意图、key package、恢复资料与完成确认。
    Exchange {
        recipient: DeviceId,
        message: RemovalExchangeMessage,
    },
    /// 受限迟交入口:已被移除设备提交的历史意图。
    Late {
        recipient: DeviceId,
        intent: SignedRemovalIntent,
    },
}

/// 一次锁外网络交互的结果。
enum ExchangeOutcome {
    /// 普通成员通道的响应消息。
    Message(RemovalExchangeMessage),
    /// 受限迟交入口已接受(结果有界,不读取具体内容)。
    LateAccepted,
}

impl RemovalCoordinator {
    pub fn new(deps: RemovalCoordinatorDeps) -> Self {
        Self {
            deps,
            wake: Arc::new(Notify::new()),
            state_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// 唤醒后台推进(事件或周期任务触发)。
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    fn empty_state(space_lineage: &str) -> RemovalPersistedState {
        RemovalPersistedState {
            space_lineage: space_lineage.to_owned(),
            intents: Vec::new(),
            locally_removed: BTreeSet::new(),
            locally_removed_devices: BTreeSet::new(),
            member_devices: BTreeMap::new(),
            retired_members: BTreeSet::new(),
            causal_history: Vec::new(),
            peer_exchanges: BTreeMap::new(),
            recovery: None,
            applied_digest: None,
            completed_member_count: None,
            admission_generation: 0,
            phase: RemovalPhase::Applied,
            updated_at_ms: 0,
        }
    }

    /// 加载(或初始化)当前空间的收敛状态。
    async fn load_state(&self) -> Result<RemovalPersistedState, RemovalCoordinatorError> {
        let space_lineage = self.deps.repository.current_space_lineage().await?;
        match self.deps.repository.load_state().await? {
            Some(state) => {
                if !space_lineage.is_empty() && state.space_lineage != space_lineage {
                    return Err(RemovalCoordinatorError::SpaceMismatch);
                }
                Ok(state)
            }
            None => Ok(Self::empty_state(&space_lineage)),
        }
    }

    async fn persist(&self, state: &RemovalPersistedState) -> Result<(), RemovalCoordinatorError> {
        self.deps.repository.save_state(state).await?;
        Ok(())
    }

    /// 提交一次目标成员移除(规格:调用方提交入口)。
    ///
    /// 意图持久化成功后本机立即停止信任目标(写入 `locally_removed`),
    /// 返回的摘要只表示"本机已生效,正在收敛"。
    pub async fn submit_removal(
        &self,
        target: &DeviceId,
        now_ms: i64,
    ) -> Result<MemberRemovalSummary, RemovalCoordinatorError> {
        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        let view = self.deps.recovery.current_view().await?;
        let own_member = self
            .deps
            .recovery
            .own_instance()
            .await?
            .and_then(|own| {
                view.members
                    .iter()
                    .find(|member| member.instance == own)
                    .cloned()
            })
            .ok_or(RemovalCoordinatorError::NotAMember)?;
        if state.own_instance_removed(&own_member.instance) {
            // L07:本机已观察到自己被移除,拒绝创建新意图。
            return Err(RemovalCoordinatorError::OwnInstanceRemoved);
        }
        let target_member = view
            .members
            .iter()
            .find(|member| member.device_id == *target)
            .cloned()
            .ok_or(RemovalCoordinatorError::UnknownTarget)?;
        if target_member.instance == own_member.instance {
            // V03:不能移除自己的成员实例。
            return Err(RemovalCoordinatorError::SelfTarget);
        }
        // B04:同一视图上重复移除同一目标只产生一个不可变意图。
        if state.intents.iter().any(|known| {
            known.content.initiator == own_member.instance
                && known.content.target == target_member.instance
                && known.content.view_epoch == view.epoch
        }) {
            return Ok(self.summary(&state, now_ms));
        }
        for member in &view.members {
            state
                .member_devices
                .entry(member.instance)
                .or_insert_with(|| member.device_id.clone());
        }
        let mut view_members = view
            .members
            .iter()
            .map(|member| member.instance)
            .collect::<Vec<_>>();
        view_members.sort_unstable();
        view_members.dedup();
        let content = RemovalIntentContent {
            space_lineage: state.space_lineage.clone(),
            view_epoch: view.epoch,
            view_members,
            initiator: own_member.instance,
            target: target_member.instance,
        };
        content.validate()?;
        let payload = content.canonical_bytes();
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&payload)
            .await?;
        let intent = SignedRemovalIntent::new(content, signature, view.causal_proof.clone());
        state.remember_causal_history(RemovalCausalCheckpoint::from_view(&view));
        state.intents.push(intent.clone());
        state.completed_member_count = None;
        state.admission_generation = state.admission_generation.saturating_add(1);
        state.locally_removed.insert(target_member.instance);
        state.locally_removed_devices.insert(target.clone());
        // 新意图会形成新的摘要；只有上一轮已经完成时才需要重新打开收敛。
        if state.phase == RemovalPhase::Complete {
            state.phase = RemovalPhase::Converging;
        }
        state.updated_at_ms = now_ms;
        let saved = self
            .deps
            .repository
            .save_new_intent_state(&intent, &state)
            .await?;
        if !saved {
            return Ok(self.summary(&self.load_state().await?, now_ms));
        }
        self.wake.notify_one();
        info!(
            intent_count = state.intents.len(),
            effective_count = state.effective_members().len(),
            "member removal intent created and applied locally"
        );
        Ok(self.summary(&state, now_ms))
    }

    /// 普通成员通道上的意图/恢复资料交换入口。
    ///
    /// 与后台推进串行执行:入站消息的读-改-写必须在 `state_lock` 内完成,
    /// 否则保存的意图可能被并发的 reconcile 旧快照覆盖。
    pub async fn ingest_exchange(
        &self,
        source_device_id: &DeviceId,
        message: RemovalExchangeMessage,
        now_ms: i64,
    ) -> Result<RemovalExchangeMessage, RemovalCoordinatorError> {
        let _guard = self.state_lock.lock().await;
        let admission_state = self.load_any_state().await?;
        let source_instance = self
            .current_member_instance(source_device_id)
            .await?
            .ok_or(RemovalCoordinatorError::NotAMember)?;
        if admission_state.locally_removed.contains(&source_instance) {
            return Err(RemovalCoordinatorError::OwnInstanceRemoved);
        }
        match message {
            RemovalExchangeMessage::Intent(intent) => {
                let accepted = self.accept_intent(*intent, now_ms).await?;
                if accepted {
                    Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ))
                } else {
                    Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ))
                }
            }
            RemovalExchangeMessage::IntentAck(intent_id) => {
                let mut state = self.load_any_state().await?;
                state
                    .peer_exchanges
                    .insert((source_device_id.clone(), intent_id), now_ms);
                self.persist(&state).await?;
                Ok(RemovalExchangeMessage::IntentAck(intent_id))
            }
            RemovalExchangeMessage::KeyPackageRequest { convergence_digest } => {
                let state = self.load_any_state().await?;
                if state.applied_digest == Some(convergence_digest)
                    || state.convergence().convergence_digest() == convergence_digest
                {
                    if let Ok(key_package) = self.deps.recovery.prepare_key_package().await {
                        return Ok(RemovalExchangeMessage::KeyPackageOffer {
                            convergence_digest,
                            key_package,
                        });
                    }
                }
                warn!(
                    reason = "digest mismatch or key package unavailable",
                    "key package request ignored"
                );
                Ok(RemovalExchangeMessage::IntentAck(
                    self.latest_intent_id().await,
                ))
            }
            RemovalExchangeMessage::KeyPackageOffer {
                convergence_digest,
                key_package,
            } => {
                let mut state = self.load_any_state().await?;
                let source_instance = self.device_instance(&state, source_device_id).await;
                let Some(recovery) = state.recovery.as_mut() else {
                    return Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ));
                };
                if recovery.convergence_digest == convergence_digest {
                    if let Some(instance) = source_instance {
                        if recovery.effective_members.contains(&instance) {
                            recovery.key_packages.insert(instance, key_package);
                            self.persist(&state).await?;
                            self.wake.notify_one();
                        }
                    }
                }
                Ok(RemovalExchangeMessage::IntentAck(
                    self.latest_intent_id().await,
                ))
            }
            RemovalExchangeMessage::RecoveryMaterial(material) => {
                let state = self.load_any_state().await?;
                let own = self
                    .deps
                    .recovery
                    .own_instance()
                    .await?
                    .ok_or(RemovalCoordinatorError::NotAMember)?;
                let expected = state.convergence().convergence_digest();
                let mut expected_members =
                    state.effective_members().into_iter().collect::<Vec<_>>();
                expected_members.sort_unstable();
                let mut material_members = material.effective_members.clone();
                material_members.sort_unstable();
                if material.convergence_digest != expected || material_members != expected_members {
                    // R08:摘要匹配但目标集合不同(或两者都不同)→ 拒绝。
                    warn!(
                        reason = "recovery material mismatch",
                        "forward recovery rejected"
                    );
                    return Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ));
                }
                if state.effective_members().iter().next().copied() != Some(source_instance) {
                    // 同一摘要只能由确定的执行者分发资料；否则不同成员可能从同一
                    // 旧状态生成并行的向前状态。
                    warn!(
                        reason = "recovery source is not the executor",
                        "forward recovery rejected"
                    );
                    return Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ));
                }
                if !material_members.contains(&own) {
                    // 本机已被移除,不应用恢复资料。
                    return Ok(RemovalExchangeMessage::IntentAck(
                        self.latest_intent_id().await,
                    ));
                }
                if let Err(error) = self
                    .deps
                    .recovery
                    .apply_forward_recovery(&material, &expected, &material_members)
                    .await
                {
                    warn!(
                        failure = recovery_failure_category(&error),
                        "forward recovery application rejected"
                    );
                    return Err(error.into());
                }
                let mut state = self.load_any_state().await?;
                state.applied_digest = Some(expected);
                state.phase = RemovalPhase::Converging;
                state.updated_at_ms = now_ms;
                let receipt = self.completion_receipt(own, &expected).await?;
                self.persist(&state).await?;
                self.wake.notify_one();
                Ok(RemovalExchangeMessage::RecoveryAck { receipt })
            }
            RemovalExchangeMessage::RecoveryAck { receipt } => {
                let mut state = self.load_any_state().await?;
                let source_instance = self.device_instance(&state, source_device_id).await;
                let valid = source_instance == Some(receipt.member)
                    && self
                        .deps
                        .member_signatures
                        .verify_current_member_payload(
                            source_device_id,
                            &receipt.payload(),
                            &receipt.signature,
                        )
                        .await
                        .unwrap_or(false);
                if let Some(recovery) = state.recovery.as_mut() {
                    if valid
                        && recovery.convergence_digest == receipt.convergence_digest
                        && recovery.effective_members.contains(&receipt.member)
                    {
                        recovery.delivery_acks.insert(receipt.member, receipt);
                        self.persist(&state).await?;
                        self.wake.notify_one();
                    }
                }
                Ok(RemovalExchangeMessage::IntentAck(
                    self.latest_intent_id().await,
                ))
            }
            RemovalExchangeMessage::Complete {
                convergence_digest,
                receipts,
            } => {
                let mut state = self.load_any_state().await?;
                let expected_executor = state.effective_members().iter().next().copied();
                let source_instance = self.device_instance(&state, source_device_id).await;
                if state.applied_digest == Some(convergence_digest)
                    && source_instance == expected_executor
                    && self
                        .receipts_cover_effective_members(&state, &convergence_digest, &receipts)
                        .await
                {
                    let completed_member_count = state.effective_members().len();
                    state.phase = RemovalPhase::Complete;
                    state.completed_member_count = Some(completed_member_count);
                    if let Some(executor) = source_instance {
                        state.retired_members.extend(
                            state
                                .effective_members()
                                .into_iter()
                                .filter(|member| *member != executor),
                        );
                    }
                    state.updated_at_ms = now_ms;
                    self.persist(&state).await?;
                    self.wake.notify_one();
                }
                Ok(RemovalExchangeMessage::IntentAck(
                    self.latest_intent_id().await,
                ))
            }
        }
    }

    /// 受限迟交入口:接收已被移除发起者的历史意图。
    ///
    /// 只返回有界接收结果;不返回成员列表、摘要、代次、密钥或内容。
    /// 与后台推进串行执行:迟交验收的读-改-写必须在 `state_lock` 内完成。
    pub async fn handle_late_submission(
        &self,
        submission: RemovalLateSubmission,
        now_ms: i64,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionError> {
        let _guard = self.state_lock.lock().await;
        let RemovalLateSubmission::Intent(intent) = submission;
        let state = self
            .load_any_state()
            .await
            .map_err(|_| RemovalLateSubmissionError::Unavailable)?;
        if state.space_lineage.is_empty() || !intent.content.space_lineage.eq(&state.space_lineage)
        {
            return Ok(RemovalLateAcceptance::Rejected {
                reason: RemovalLateRejectionReason::InvalidSpaceLineage,
            });
        }
        match self.accept_intent(*intent, now_ms).await {
            Ok(true) => Ok(RemovalLateAcceptance::Accepted {
                intent_id: self.latest_intent_id().await,
            }),
            Ok(false) => Ok(RemovalLateAcceptance::AlreadyKnown {
                intent_id: self.latest_intent_id().await,
            }),
            Err(error) => {
                warn!(failure = "late_intent_rejected", "late intent rejected");
                Ok(RemovalLateAcceptance::Rejected {
                    reason: match error {
                        RemovalCoordinatorError::Verification(
                            RemovalIntentVerificationError::InvalidSpaceLineage,
                        ) => RemovalLateRejectionReason::InvalidSpaceLineage,
                        RemovalCoordinatorError::MissingCausalHistory => {
                            RemovalLateRejectionReason::MissingCausalHistory
                        }
                        _ => RemovalLateRejectionReason::Invalid,
                    },
                })
            }
        }
    }

    /// 后台推进:传播意图、收集恢复所需 key package、生成并分发恢复资料、
    /// 收集完成确认、广播完成。任何失败保持状态,由下一次调用重试。
    ///
    /// 状态修改在 `state_lock` 内串行执行;网络交换在锁外进行,响应再回到
    /// 锁内应用,避免后台推进用旧快照覆盖入站消息写入的更新。
    pub async fn reconcile(&self, now_ms: i64) -> Result<(), RemovalCoordinatorError> {
        loop {
            let outgoing = {
                let _guard = self.state_lock.lock().await;
                self.reconcile_plan(now_ms).await?
            };
            if outgoing.is_empty() {
                return Ok(());
            }
            let mut responses = Vec::with_capacity(outgoing.len());
            for item in outgoing {
                let outcome = match &item {
                    Outgoing::Exchange { recipient, message } => self
                        .deps
                        .exchange
                        .exchange(recipient, message.clone())
                        .await
                        .map(ExchangeOutcome::Message),
                    Outgoing::Late { recipient, intent } => self
                        .deps
                        .late_submission
                        .submit_late(
                            recipient,
                            RemovalLateSubmission::Intent(Box::new(intent.clone())),
                        )
                        .await
                        .map(|_| ExchangeOutcome::LateAccepted)
                        .map_err(|error| match error {
                            RemovalLateSubmissionTransportError::Transport => {
                                RemovalExchangeError::Transport
                            }
                            RemovalLateSubmissionTransportError::Offline => {
                                RemovalExchangeError::Offline
                            }
                        }),
                };
                responses.push((item, outcome));
            }
            let progressed = {
                let _guard = self.state_lock.lock().await;
                self.apply_exchange_responses(responses, now_ms).await?
            };
            if !progressed {
                return Ok(());
            }
        }
    }

    /// 锁内决策:根据最新状态决定本轮要发送的消息。
    ///
    /// 只做本地状态推进与持久化,不发起网络;网络交换统一在锁外进行。
    async fn reconcile_plan(&self, now_ms: i64) -> Result<Vec<Outgoing>, RemovalCoordinatorError> {
        let mut state = self.load_any_state().await?;
        if state.phase == RemovalPhase::Complete {
            return self.completion_broadcast_plan(&mut state, now_ms).await;
        }
        if state.phase == RemovalPhase::RecoveryRequired {
            return Ok(Vec::new());
        }
        let convergence = state.convergence();
        if convergence.is_empty() {
            state.phase = RemovalPhase::Applied;
            state.updated_at_ms = now_ms;
            self.persist(&state).await?;
            return Ok(Vec::new());
        }
        let effective = state.effective_members();
        if effective.is_empty() {
            // R09:合法意图合并后移除全部成员。
            state.phase = RemovalPhase::RecoveryRequired;
            state.updated_at_ms = now_ms;
            self.persist(&state).await?;
            info!("member removal converged to an empty member set");
            return Ok(Vec::new());
        }
        if state.phase == RemovalPhase::Converging {
            if let Some(recovery) = state.recovery.as_ref() {
                let digest = convergence.convergence_digest();
                let effective_len = recovery.effective_members.len();
                if recovery.convergence_digest == digest
                    && effective_len > 0
                    && recovery.delivery_acks.len() == effective_len
                    && state.applied_digest == Some(digest)
                {
                    // 执行者或保留成员已收集齐全部确认:完成本轮收敛。
                    // 恢复资料已用新的成员实例替换旧实例;除本机当前实例外,
                    // 其余旧实例不得再参与下一轮成员计算。
                    let own = self
                        .deps
                        .recovery
                        .own_instance()
                        .await?
                        .ok_or(RemovalCoordinatorError::NotAMember)?;
                    state.retired_members.extend(
                        recovery
                            .effective_members
                            .iter()
                            .copied()
                            .filter(|member| *member != own),
                    );
                    state.phase = RemovalPhase::Complete;
                    state.completed_member_count = Some(effective_len);
                    state.updated_at_ms = now_ms;
                    self.persist(&state).await?;
                    info!(effective_count = effective_len, "member removal converged");
                    return self.completion_broadcast_plan(&mut state, now_ms).await;
                }
            }
        }
        let digest = convergence.convergence_digest();
        let own = self
            .deps
            .recovery
            .own_instance()
            .await?
            .ok_or(RemovalCoordinatorError::NotAMember)?;
        let still_effective = self.device_still_effective(&state, own).await?;
        if !still_effective {
            // 本机已被移除:不参与恢复，只能通过受限入口迟交已保存的历史意图。
            return self.late_submission_plan(&mut state, own, now_ms).await;
        }
        let mut outgoing = self.exchange_intents_plan(&mut state, now_ms).await?;
        let executor = effective
            .iter()
            .next()
            .copied()
            .ok_or(RemovalCoordinatorError::NoExecutor)?;
        if self.device_is_executor(&state, own, executor).await? {
            outgoing.extend(
                self.executor_plan(&mut state, &effective, &digest, now_ms)
                    .await?,
            );
        } else {
            // 非执行者:恢复资料在 ingest_exchange 中处理。
            state.phase = RemovalPhase::Converging;
            state.updated_at_ms = now_ms;
            self.persist(&state).await?;
        }
        Ok(outgoing)
    }

    /// 本机是否仍是有效成员。优先按实例判断;恢复资料应用后本机实例可能
    /// 变化(重新加入产生新签名密钥),此时按设备是否仍属于有效成员集合判断。
    async fn device_still_effective(
        &self,
        state: &RemovalPersistedState,
        own: MemberInstanceId,
    ) -> Result<bool, RemovalCoordinatorError> {
        if state.effective_members().contains(&own) {
            return Ok(true);
        }
        let Some(own_device) = self.device_for_instance(state, own).await else {
            return Ok(false);
        };
        for instance in state.effective_members() {
            if self.device_for_instance(state, instance).await.as_ref() == Some(&own_device) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 本机是否本轮执行者。恢复资料应用后本机实例可能变化,按设备比较。
    async fn device_is_executor(
        &self,
        state: &RemovalPersistedState,
        own: MemberInstanceId,
        executor: MemberInstanceId,
    ) -> Result<bool, RemovalCoordinatorError> {
        if own == executor {
            return Ok(true);
        }
        let Some(own_device) = self.device_for_instance(state, own).await else {
            return Ok(false);
        };
        Ok(self.device_for_instance(state, executor).await.as_ref() == Some(&own_device))
    }

    /// 执行者锁内决策:重置过时恢复(R06)、收集 key package、生成并分发恢复资料、
    /// 收集完成确认并广播完成。所有网络消息进入返回列表,由调用方锁外发送。
    async fn executor_plan(
        &self,
        state: &mut RemovalPersistedState,
        effective: &BTreeSet<MemberInstanceId>,
        digest: &[u8; 32],
        now_ms: i64,
    ) -> Result<Vec<Outgoing>, RemovalCoordinatorError> {
        let needs_reset = match state.recovery.as_ref() {
            Some(recovery) => {
                recovery.convergence_digest != *digest
                    || recovery.effective_members != effective.iter().copied().collect::<Vec<_>>()
            }
            None => true,
        };
        if needs_reset {
            state.recovery = Some(RemovalRecoveryPersisted {
                convergence_digest: *digest,
                effective_members: effective.iter().copied().collect(),
                key_packages: BTreeMap::new(),
                material: None,
                local_checkpoint: None,
                delivery_acks: BTreeMap::new(),
                completion_deliveries: BTreeMap::new(),
            });
            self.persist(state).await?;
        }
        let own = self
            .deps
            .recovery
            .own_instance()
            .await?
            .ok_or(RemovalCoordinatorError::NotAMember)?;
        let pending_members: Vec<MemberInstanceId> = effective
            .iter()
            .copied()
            .filter(|instance| *instance != own)
            .collect();
        let mut outgoing = Vec::new();
        if state
            .recovery
            .as_ref()
            .is_some_and(|recovery| recovery.material.is_none())
        {
            // 向尚未提供 key package 的有效成员请求。
            for instance in &pending_members {
                let recovery = state
                    .recovery
                    .as_ref()
                    .ok_or(RemovalCoordinatorError::NoExecutor)?;
                if recovery.key_packages.contains_key(instance) {
                    continue;
                }
                if let Some(device_id) = self.device_for_instance(state, *instance).await {
                    outgoing.push(Outgoing::Exchange {
                        recipient: device_id,
                        message: RemovalExchangeMessage::KeyPackageRequest {
                            convergence_digest: *digest,
                        },
                    });
                }
            }
            if !outgoing.is_empty() {
                state.updated_at_ms = now_ms;
                self.persist(state).await?;
                return Ok(outgoing);
            }
            // key package 已齐:生成恢复资料。
            let key_packages = state
                .recovery
                .as_ref()
                .ok_or(RemovalCoordinatorError::NoExecutor)?
                .key_packages
                .iter()
                .map(|(instance, key_package)| (*instance, key_package.clone()))
                .collect::<Vec<_>>();
            let prepared = self
                .deps
                .recovery
                .prepare_forward_recovery(
                    digest,
                    &effective.iter().copied().collect::<Vec<_>>(),
                    &key_packages,
                )
                .await?;
            let recovery = state
                .recovery
                .as_mut()
                .ok_or(RemovalCoordinatorError::NoExecutor)?;
            recovery.material = Some(prepared.material);
            recovery.local_checkpoint = Some(prepared.local_checkpoint);
            // 先把资料与执行者检查点写入持久状态。这里之后崩溃时，重启会安装同一份
            // 检查点，而不是重新生成竞争的安全状态。
            state.recovery = Some(recovery.clone());
            self.persist(state).await?;
        }
        // 执行者每次推进都安装同一份本机检查点(幂等):生成后、重启后都从这里
        // 继续,绝不重新生成第二份恢复资料。
        let local_checkpoint = state
            .recovery
            .as_ref()
            .and_then(|recovery| recovery.local_checkpoint.clone())
            .ok_or(RemovalCoordinatorError::NoExecutor)?;
        self.deps
            .recovery
            .install_prepared_forward_recovery(&local_checkpoint)
            .await?;
        let recovery = state
            .recovery
            .as_mut()
            .ok_or(RemovalCoordinatorError::NoExecutor)?;
        if !recovery.delivery_acks.contains_key(&own) {
            recovery
                .delivery_acks
                .insert(own, self.completion_receipt(own, digest).await?);
        }
        state.applied_digest = Some(*digest);
        state.phase = RemovalPhase::Converging;
        state.recovery = Some(recovery.clone());
        self.persist(state).await?;
        let material = state
            .recovery
            .as_ref()
            .and_then(|recovery| recovery.material.clone())
            .ok_or(RemovalCoordinatorError::NoExecutor)?;
        let recovery = state
            .recovery
            .as_ref()
            .ok_or(RemovalCoordinatorError::NoExecutor)?;
        for instance in &pending_members {
            if recovery.delivery_acks.contains_key(instance) {
                continue;
            }
            if let Some(device_id) = self.device_for_instance(state, *instance).await {
                outgoing.push(Outgoing::Exchange {
                    recipient: device_id,
                    message: RemovalExchangeMessage::RecoveryMaterial(material.clone()),
                });
            }
        }
        Ok(outgoing)
    }

    /// 完成事实已经保存后，继续向尚未答复的保留成员发送完整确认。资料分发成功
    /// 不代表完成通知一定送达；该进度必须和恢复资料一起跨重启保留。
    async fn completion_broadcast_plan(
        &self,
        state: &mut RemovalPersistedState,
        now_ms: i64,
    ) -> Result<Vec<Outgoing>, RemovalCoordinatorError> {
        let Some(recovery) = state.recovery.as_ref() else {
            return Ok(Vec::new());
        };
        let Some(executor) = recovery.effective_members.iter().copied().min() else {
            return Ok(Vec::new());
        };
        let recipients = recovery
            .effective_members
            .iter()
            .copied()
            .filter(|member| {
                *member != executor && !recovery.completion_deliveries.contains_key(member)
            })
            .collect::<Vec<_>>();
        let message = RemovalExchangeMessage::Complete {
            convergence_digest: recovery.convergence_digest,
            receipts: recovery.delivery_acks.values().cloned().collect(),
        };
        let mut outgoing = Vec::new();
        for member in recipients {
            if let Some(device_id) = self.device_for_instance(state, member).await {
                outgoing.push(Outgoing::Exchange {
                    recipient: device_id,
                    message: message.clone(),
                });
            }
        }
        if !outgoing.is_empty() {
            state.updated_at_ms = now_ms;
            self.persist(state).await?;
        }
        Ok(outgoing)
    }

    /// 锁内应用网络响应:把投递成功记录、key package、恢复确认和完成投递
    /// 写回最新持久化状态。返回 `true` 表示本轮有进展,值得继续下一轮计划。
    async fn apply_exchange_responses(
        &self,
        responses: Vec<(Outgoing, Result<ExchangeOutcome, RemovalExchangeError>)>,
        now_ms: i64,
    ) -> Result<bool, RemovalCoordinatorError> {
        let mut state = self.load_any_state().await?;
        let mut progressed = false;
        for (item, outcome) in responses {
            match (item, outcome) {
                (
                    Outgoing::Exchange {
                        recipient,
                        message: RemovalExchangeMessage::Intent(intent),
                    },
                    Ok(ExchangeOutcome::Message(_)),
                ) => {
                    state
                        .peer_exchanges
                        .insert((recipient, intent.intent_id), now_ms);
                    progressed = true;
                }
                (
                    Outgoing::Exchange {
                        recipient,
                        message: RemovalExchangeMessage::KeyPackageRequest { convergence_digest },
                    },
                    Ok(ExchangeOutcome::Message(RemovalExchangeMessage::KeyPackageOffer {
                        convergence_digest: response_digest,
                        key_package,
                    })),
                ) if response_digest == convergence_digest => {
                    if let Some(instance) = self.device_instance(&state, &recipient).await {
                        if let Some(recovery) = state.recovery.as_mut() {
                            if recovery.convergence_digest == convergence_digest {
                                recovery.key_packages.insert(instance, key_package);
                                progressed = true;
                            }
                        }
                    }
                }
                (
                    Outgoing::Exchange {
                        recipient,
                        message: RemovalExchangeMessage::RecoveryMaterial(_),
                    },
                    Ok(ExchangeOutcome::Message(RemovalExchangeMessage::RecoveryAck { receipt })),
                ) => {
                    let valid = self.verify_recovery_ack(&state, &recipient, &receipt).await;
                    if valid {
                        if let Some(recovery) = state.recovery.as_mut() {
                            if recovery.delivery_acks.contains_key(&receipt.member) {
                                continue;
                            }
                            recovery.delivery_acks.insert(receipt.member, receipt);
                            progressed = true;
                        }
                    } else {
                        warn!(
                            failure = "recovery_ack_invalid",
                            "recovery material received an invalid confirmation"
                        );
                    }
                }
                (
                    Outgoing::Exchange {
                        recipient,
                        message: RemovalExchangeMessage::Complete { .. },
                    },
                    Ok(ExchangeOutcome::Message(_)),
                ) => {
                    // 投递记录使用发送计划中的成员实例,与广播过滤条件一致;
                    // 恢复后设备实例可能变化,不能重新解析。
                    let mut member = None;
                    for instance in state
                        .recovery
                        .as_ref()
                        .map(|recovery| {
                            recovery
                                .effective_members
                                .iter()
                                .copied()
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                    {
                        if self.device_for_instance(&state, instance).await.as_ref()
                            == Some(&recipient)
                        {
                            member = Some(instance);
                            break;
                        }
                    }
                    if let Some(member) = member {
                        if let Some(recovery) = state.recovery.as_mut() {
                            recovery.completion_deliveries.insert(member, now_ms);
                            progressed = true;
                        }
                    }
                }
                (Outgoing::Late { recipient, intent }, Ok(_)) => {
                    state
                        .peer_exchanges
                        .insert((recipient, intent.intent_id), now_ms);
                    progressed = true;
                }
                (Outgoing::Exchange { .. }, Ok(_)) | (Outgoing::Exchange { .. }, Err(_)) => {}
                (Outgoing::Late { .. }, Err(_)) => {}
            }
        }
        if state.phase == RemovalPhase::Converging {
            // 完成判定在锁内 reconcile_plan 中进行;这里只负责把确认写回。
        }
        if progressed {
            state.updated_at_ms = now_ms;
            self.persist(&state).await?;
            self.wake.notify_one();
        }
        Ok(progressed)
    }

    /// 校验恢复确认:目标实例必须属于本轮有效成员集合,摘要匹配,且签名
    /// 由该设备当前成员凭据签发。不按当前实例解析——恢复资料应用后成员
    /// 实例会因重新加入而变化,receipt 固定携带意图视图中的旧实例。
    async fn verify_recovery_ack(
        &self,
        state: &RemovalPersistedState,
        device_id: &DeviceId,
        receipt: &RemovalCompletionReceipt,
    ) -> bool {
        let Some(recovery) = state.recovery.as_ref() else {
            return false;
        };
        recovery.effective_members.contains(&receipt.member)
            && receipt.convergence_digest == recovery.convergence_digest
            && self
                .deps
                .member_signatures
                .verify_current_member_payload(device_id, &receipt.payload(), &receipt.signature)
                .await
                .unwrap_or(false)
    }

    /// 把尚未确认的意图逐条发给已知成员(幂等,可重试)。只生成发送计划,
    /// 投递记录由 [`RemovalCoordinator::apply_exchange_responses`] 在锁内更新。
    async fn exchange_intents_plan(
        &self,
        state: &mut RemovalPersistedState,
        now_ms: i64,
    ) -> Result<Vec<Outgoing>, RemovalCoordinatorError> {
        let members = self.deps.member_repo.list().await?;
        let intents = state.intents.clone();
        let mut outgoing = Vec::new();
        for intent in &intents {
            let intent_id = intent.intent_id;
            for member in &members {
                if state.device_is_locally_removed(&member.device_id)
                    || state
                        .peer_exchanges
                        .contains_key(&(member.device_id.clone(), intent_id))
                {
                    continue;
                }
                outgoing.push(Outgoing::Exchange {
                    recipient: member.device_id.clone(),
                    message: RemovalExchangeMessage::Intent(Box::new(intent.clone())),
                });
            }
        }
        if !outgoing.is_empty() {
            state.updated_at_ms = now_ms;
            self.persist(state).await?;
        }
        Ok(outgoing)
    }

    /// 已被移除后的唯一网络动作：把已保存的历史意图通过受限入口迟交给其他
    /// 已知设备。任何有界回应都记为已投递，网络失败才留给下次后台触发。
    async fn late_submission_plan(
        &self,
        state: &mut RemovalPersistedState,
        own: MemberInstanceId,
        now_ms: i64,
    ) -> Result<Vec<Outgoing>, RemovalCoordinatorError> {
        let members = self.deps.member_repo.list().await?;
        let intents = state.intents.clone();
        let mut outgoing = Vec::new();
        for intent in intents {
            for member in &members {
                if self.device_instance(state, &member.device_id).await == Some(own)
                    || state
                        .peer_exchanges
                        .contains_key(&(member.device_id.clone(), intent.intent_id))
                {
                    continue;
                }
                outgoing.push(Outgoing::Late {
                    recipient: member.device_id.clone(),
                    intent: intent.clone(),
                });
            }
        }
        if !outgoing.is_empty() {
            state.updated_at_ms = now_ms;
            self.persist(state).await?;
        }
        Ok(outgoing)
    }

    /// 验收一条意图:验证、去重、持久化,并立即应用本机安全限制。
    /// 返回 `true` 表示新意图。
    async fn accept_intent(
        &self,
        intent: SignedRemovalIntent,
        now_ms: i64,
    ) -> Result<bool, RemovalCoordinatorError> {
        let mut state = self.load_any_state().await?;
        if !state.space_lineage.is_empty() && state.space_lineage != intent.content.space_lineage {
            return Err(RemovalCoordinatorError::SpaceMismatch);
        }
        intent.validate_content()?;
        self.deps.verification.verify_intent(&intent).await?;
        if !state.remembers_causal_history(&intent) {
            let view = self.deps.recovery.current_view().await?;
            let current_checkpoint = RemovalCausalCheckpoint::from_view(&view);
            let intent_checkpoint = RemovalCausalCheckpoint::from_intent(&intent);
            if current_checkpoint != intent_checkpoint {
                // L05:签名可以正确验证，但本机没有可作为基准的同一历史。不能
                // 猜测两条历史的关系，也不能把未锚定意图写入已知集合。
                state.phase = RemovalPhase::RecoveryRequired;
                state.updated_at_ms = now_ms;
                self.persist(&state).await?;
                self.wake.notify_one();
                return Err(RemovalCoordinatorError::MissingCausalHistory);
            }
            state.remember_causal_history(current_checkpoint);
        }
        self.remember_current_view_members(&mut state).await;
        if state.space_lineage.is_empty() {
            state.space_lineage = self.deps.repository.current_space_lineage().await?;
        }
        if state
            .intents
            .iter()
            .any(|known| known.intent_id == intent.intent_id)
        {
            return Ok(false);
        }
        state.locally_removed.insert(intent.content.target);
        if let Some(target) = self
            .device_for_instance(&state, intent.content.target)
            .await
        {
            state.locally_removed_devices.insert(target);
        }
        state.intents.push(intent.clone());
        state.completed_member_count = None;
        state.admission_generation = state.admission_generation.saturating_add(1);
        // 迟到合法意图重新打开已完成状态(P04/L08)。
        state.phase = RemovalPhase::Converging;
        state.updated_at_ms = now_ms;
        let fresh = self
            .deps
            .repository
            .save_new_intent_state(&intent, &state)
            .await?;
        if !fresh {
            return Ok(false);
        }
        self.wake.notify_one();
        info!(
            intent_count = state.intents.len(),
            "remote removal intent accepted"
        );
        Ok(true)
    }

    /// 查询当前完整状态(一次查询恢复完整视图)。
    pub async fn query(
        &self,
        now_ms: i64,
    ) -> Result<MemberRemovalSummary, RemovalCoordinatorError> {
        let state = self.load_any_state().await?;
        let mut summary = self.summary(&state, now_ms);
        if state.intents.is_empty() {
            summary.effective_member_count = self.deps.member_repo.list().await?.len();
        }
        Ok(summary)
    }

    fn summary(&self, state: &RemovalPersistedState, now_ms: i64) -> MemberRemovalSummary {
        let convergence = state.convergence();
        let effective = state.effective_members();
        // Complete 只对当前已知意图集合成立:任何与 applied_digest 不一致的
        // 新意图都会把阶段拉回 Converging(L08/P04)。
        let phase = if state.phase == RemovalPhase::RecoveryRequired {
            RemovalPhase::RecoveryRequired
        } else if state.phase == RemovalPhase::Complete
            && state.applied_digest == Some(convergence.convergence_digest())
        {
            RemovalPhase::Complete
        } else if state.phase == RemovalPhase::Complete {
            // 迟到合法意图:完成结论被重新打开。
            RemovalPhase::Converging
        } else if convergence.is_empty() {
            RemovalPhase::Applied
        } else {
            state.phase
        };
        MemberRemovalSummary::new(
            phase,
            state.intents.len(),
            if phase == RemovalPhase::Complete {
                state.completed_member_count.unwrap_or(effective.len())
            } else {
                effective.len()
            },
            if convergence.is_empty() {
                None
            } else {
                Some(convergence.convergence_digest())
            },
            now_ms.max(state.updated_at_ms),
        )
    }

    async fn latest_intent_id(&self) -> RemovalIntentId {
        self.load_any_state()
            .await
            .map(|state| {
                state
                    .intents
                    .last()
                    .map(|intent| intent.intent_id)
                    .unwrap_or_else(|| RemovalIntentId::from_bytes([0; 32]))
            })
            .unwrap_or_else(|_| RemovalIntentId::from_bytes([0; 32]))
    }

    async fn load_any_state(&self) -> Result<RemovalPersistedState, RemovalCoordinatorError> {
        self.load_state().await
    }

    async fn completion_receipt(
        &self,
        member: MemberInstanceId,
        convergence_digest: &[u8; 32],
    ) -> Result<RemovalCompletionReceipt, RemovalCoordinatorError> {
        let payload = RemovalCompletionReceipt::signing_payload(member, convergence_digest);
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&payload)
            .await?;
        Ok(RemovalCompletionReceipt {
            member,
            convergence_digest: *convergence_digest,
            signature,
        })
    }

    async fn receipts_cover_effective_members(
        &self,
        state: &RemovalPersistedState,
        convergence_digest: &[u8; 32],
        receipts: &[RemovalCompletionReceipt],
    ) -> bool {
        let effective = state.effective_members();
        if receipts.len() != effective.len() {
            return false;
        }
        let mut confirmed = BTreeSet::new();
        for receipt in receipts {
            if receipt.convergence_digest != *convergence_digest
                || !effective.contains(&receipt.member)
                || !confirmed.insert(receipt.member)
            {
                return false;
            }
            let Some(device_id) = self.device_for_instance(state, receipt.member).await else {
                return false;
            };
            let valid = self
                .deps
                .member_signatures
                .verify_current_member_payload(&device_id, &receipt.payload(), &receipt.signature)
                .await
                .unwrap_or(false);
            if !valid {
                return false;
            }
        }
        confirmed == effective
    }

    async fn remember_current_view_members(&self, state: &mut RemovalPersistedState) {
        if let Ok(view) = self.deps.recovery.current_view().await {
            for member in view.members {
                state
                    .member_devices
                    .entry(member.instance)
                    .or_insert(member.device_id);
            }
        }
    }

    /// 通过已验证的因果视图把成员实例解析为设备标识。当前视图只作为
    /// 尚未落盘的新成员映射的补充，恢复后不能覆盖历史成员实例的映射。
    async fn device_for_instance(
        &self,
        state: &RemovalPersistedState,
        instance: MemberInstanceId,
    ) -> Option<DeviceId> {
        if let Some(device_id) = state.member_devices.get(&instance) {
            return Some(device_id.clone());
        }
        self.deps
            .recovery
            .current_view()
            .await
            .ok()
            .and_then(|view| {
                view.members
                    .into_iter()
                    .find(|member| member.instance == instance)
                    .map(|member| member.device_id)
            })
    }

    /// 通过已保存的因果映射把设备标识解析为成员实例。
    async fn device_instance(
        &self,
        state: &RemovalPersistedState,
        device_id: &DeviceId,
    ) -> Option<MemberInstanceId> {
        if let Ok(Some(instance)) = self.current_member_instance(device_id).await {
            return Some(instance);
        }
        if let Some((instance, _)) = state
            .member_devices
            .iter()
            .find(|(_, mapped_device_id)| *mapped_device_id == device_id)
        {
            return Some(*instance);
        }
        self.deps
            .recovery
            .current_view()
            .await
            .ok()
            .and_then(|view| {
                view.members
                    .into_iter()
                    .find(|member| member.device_id == *device_id)
                    .map(|member| member.instance)
            })
    }

    /// 普通交换只接受当前成员实例。先读取当前视图可避免旧实例映射把重新准入的
    /// 同一设备误判为已移除成员。
    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<MemberInstanceId>, RemovalCoordinatorError> {
        Ok(self
            .deps
            .recovery
            .current_view()
            .await?
            .members
            .into_iter()
            .find(|member| member.device_id == *device_id)
            .map(|member| member.instance))
    }
}

fn recovery_failure_category(error: &uc_core::membership::RemovalRecoveryError) -> &'static str {
    match error {
        uc_core::membership::RemovalRecoveryError::Unavailable => "unavailable",
        uc_core::membership::RemovalRecoveryError::InvalidMaterial => "invalid_material",
        uc_core::membership::RemovalRecoveryError::OutOfOrder => "out_of_order",
        uc_core::membership::RemovalRecoveryError::Repository(_) => "persistence",
    }
}

#[async_trait::async_trait]
impl RemovalExchangeEndpointPort for RemovalCoordinator {
    async fn handle_exchange(
        &self,
        source_device_id: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError> {
        self.ingest_exchange(
            source_device_id,
            message,
            chrono::Utc::now().timestamp_millis(),
        )
        .await
        .map_err(|_| RemovalExchangeError::Rejected)
    }
}

#[async_trait::async_trait]
impl RemovalLateSubmissionEndpointPort for RemovalCoordinator {
    async fn handle_late_submission(
        &self,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionError> {
        self.handle_late_submission(submission, chrono::Utc::now().timestamp_millis())
            .await
    }
}

#[async_trait::async_trait]
impl RemovalTargetGatePort for RemovalCoordinator {
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool {
        match self.load_any_state().await {
            Ok(state) => state.device_is_locally_removed(device_id),
            Err(_) => true,
        }
    }
}

#[async_trait::async_trait]
impl RemovalAdmissionGatePort for RemovalCoordinator {
    async fn admission_decision(&self, invitation_generation: u64) -> RemovalAdmissionDecision {
        let state = match self.load_any_state().await {
            Ok(state) => state,
            Err(_) => return RemovalAdmissionDecision::Unavailable,
        };
        if state.intents.is_empty() {
            return RemovalAdmissionDecision::Allowed;
        }
        match self.summary(&state, state.updated_at_ms).phase {
            RemovalPhase::Applied | RemovalPhase::Converging => {
                RemovalAdmissionDecision::AwaitingConvergence
            }
            RemovalPhase::RecoveryRequired => RemovalAdmissionDecision::RecoveryRequired,
            RemovalPhase::Complete if invitation_generation == state.admission_generation => {
                RemovalAdmissionDecision::Allowed
            }
            RemovalPhase::Complete => RemovalAdmissionDecision::SupersededInvitation,
        }
    }

    async fn invitation_generation(&self) -> Result<u64, RemovalAdmissionDecision> {
        let state = self
            .load_any_state()
            .await
            .map_err(|_| RemovalAdmissionDecision::Unavailable)?;
        if state.intents.is_empty() {
            return Ok(state.admission_generation);
        }
        match self.summary(&state, state.updated_at_ms).phase {
            RemovalPhase::Complete => Ok(state.admission_generation),
            RemovalPhase::Applied | RemovalPhase::Converging => {
                Err(RemovalAdmissionDecision::AwaitingConvergence)
            }
            RemovalPhase::RecoveryRequired => Err(RemovalAdmissionDecision::RecoveryRequired),
        }
    }
}
