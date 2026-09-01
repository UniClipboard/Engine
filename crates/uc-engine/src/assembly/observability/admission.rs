use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use uc_application::deps::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AdmissionSpaceTransitionError,
    AdmissionSpaceTransitionPort, AdmissionSpaceTransitionPreparationV2,
    AdmissionSpaceTransitionStepV2, AuthenticatedAdmissionExchangePort,
    AuthenticatedAdmissionReply, AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationStateError,
    JoinerActivationStatePort, LoadedJoinerActivation, LoadedPendingAdmission,
    LoadedSponsorAdmission, PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PreparedJoinerActivation, PreparedJoinerCandidateMaterial,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionStateError, SponsorAdmissionStatePort,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    AdmissionSpaceTransitionV2, JoinerActivationPreparation, JoinerAdmissionTransition,
    JoinerCandidatePreparation, JoinerCompletePreparation, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId, SpaceAdmissionRoute,
};

/// Engine 交给准入观测装配的真实能力集合。
pub(crate) struct AdmissionPortImplementations {
    pub(crate) recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(crate) transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub(crate) sponsor_state: Arc<dyn SponsorAdmissionStatePort>,
    pub(crate) candidate_preparation: Arc<dyn PrepareJoinerCandidatePort>,
    pub(crate) activation_preparation: Arc<dyn PrepareJoinerActivationPort>,
    pub(crate) activation_state: Arc<dyn JoinerActivationStatePort>,
    pub(crate) activation_executor: Arc<dyn ExecuteJoinerActivationPort>,
}

/// 完整准入链路的已观测能力；调用方只负责将这些 port 注入 Application。
pub(crate) struct ObservedAdmissionPorts {
    pub(crate) recovery_state: Arc<dyn PendingAdmissionRecoveryStatePort>,
    pub(crate) transport: Arc<dyn SpaceAdmissionTransportPort>,
    pub(crate) sponsor_state: Arc<dyn SponsorAdmissionStatePort>,
    pub(crate) candidate_preparation: Arc<dyn PrepareJoinerCandidatePort>,
    pub(crate) activation_preparation: Arc<dyn PrepareJoinerActivationPort>,
    pub(crate) activation_state: Arc<dyn JoinerActivationStatePort>,
    pub(crate) activation_executor: Arc<dyn ExecuteJoinerActivationPort>,
}

impl ObservedAdmissionPorts {
    pub(crate) fn observe_session_transition(
        inner: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Arc<dyn AdmissionSpaceTransitionPort> {
        Arc::new(ObservedSpaceSessionTransition::new(
            inner,
            SpaceSessionTransitionObservationPolicy::record_all(),
        ))
    }

    pub(crate) fn assemble(inner: AdmissionPortImplementations) -> Self {
        let activation_policy =
            JoinerActivationObservationPolicy::suppress_successful_empty_loads();
        Self {
            recovery_state: Arc::new(ObservedAdmissionRecoveryState::new(
                inner.recovery_state,
                AdmissionRecoveryObservationPolicy::suppress_successful_empty_loads(),
            )),
            transport: Arc::new(ObservedSpaceAdmissionTransport::new(
                inner.transport,
                SpaceAdmissionTransportObservationPolicy::record_safe_message_kind(),
            )),
            sponsor_state: Arc::new(ObservedSponsorAdmissionState::new(
                inner.sponsor_state,
                SponsorAdmissionStateObservationPolicy::record_all(),
            )),
            candidate_preparation: Arc::new(ObservedJoinerCandidatePreparation::new(
                inner.candidate_preparation,
                JoinerCandidateObservationPolicy::record_all(),
            )),
            activation_preparation: Arc::new(ObservedJoinerActivationPreparation::new(
                inner.activation_preparation,
                activation_policy,
            )),
            activation_state: Arc::new(ObservedJoinerActivationState::new(
                inner.activation_state,
                activation_policy,
            )),
            activation_executor: Arc::new(ObservedJoinerActivationExecutor::new(
                inner.activation_executor,
                activation_policy,
            )),
        }
    }
}

#[derive(Clone, Copy)]
enum AdmissionRecoveryStateOperation {
    Load,
    Commit,
}

impl AdmissionRecoveryStateOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "recovery_state_load",
            Self::Commit => "recovery_state_commit",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct AdmissionRecoveryObservationPolicy {
    suppress_successful_empty_loads: bool,
}

impl AdmissionRecoveryObservationPolicy {
    pub(crate) const fn suppress_successful_empty_loads() -> Self {
        Self {
            suppress_successful_empty_loads: true,
        }
    }

    const fn should_record_load(self, successful: bool, loaded_count: Option<usize>) -> bool {
        !successful || !self.suppress_successful_empty_loads || !matches!(loaded_count, Some(0))
    }
}

pub(crate) struct ObservedAdmissionRecoveryState {
    inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
    policy: AdmissionRecoveryObservationPolicy,
}

impl ObservedAdmissionRecoveryState {
    pub(crate) fn new(
        inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
        policy: AdmissionRecoveryObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record_load(started: Instant, loaded_count: usize) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = "ok",
            loaded_count,
            "admission recovery state load completed"
        );
    }

    fn record_load_error(started: Instant) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = "error",
            "admission recovery state load failed"
        );
    }

    fn record_commit(started: Instant, success: bool) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Commit.as_str(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = if success { "ok" } else { "error" },
            "admission recovery state commit completed"
        );
    }
}

#[async_trait]
impl PendingAdmissionRecoveryStatePort for ObservedAdmissionRecoveryState {
    async fn load(
        &self,
        trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        let started = Instant::now();
        let result = self.inner.load(trigger).await;
        let loaded_count = result.as_ref().ok().map(Vec::len);
        if self.policy.should_record_load(result.is_ok(), loaded_count) {
            match loaded_count {
                Some(loaded_count) => Self::record_load(started, loaded_count),
                None => Self::record_load_error(started),
            }
        }
        result
    }

    async fn commit(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let started = Instant::now();
        let result = self.inner.commit(token, transition).await;
        Self::record_commit(started, result.is_ok());
        result
    }
}

/// 在 Engine 组装边界观测认证信道；成功结果继续包装消息交换对象，避免只测到建链而丢失往返阶段。
pub(crate) struct ObservedSpaceAdmissionTransport {
    inner: Arc<dyn SpaceAdmissionTransportPort>,
    policy: SpaceAdmissionTransportObservationPolicy,
}

#[derive(Clone, Copy)]
enum SpaceAdmissionTransportOperation {
    Establish,
    Exchange,
}

impl SpaceAdmissionTransportOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Establish => "admission_channel_establish",
            Self::Exchange => "admission_message_exchange",
        }
    }
}

impl ObservedSpaceAdmissionTransport {
    pub(crate) fn new(
        inner: Arc<dyn SpaceAdmissionTransportPort>,
        policy: SpaceAdmissionTransportObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record_establish(started: Instant, channel: &'static str, success: bool) {
        tracing::info!(
            target: "admission.performance",
            operation = SpaceAdmissionTransportOperation::Establish.as_str(),
            channel,
            elapsed_ms = started.elapsed().as_millis() as u64,
            outcome = if success { "ok" } else { "error" },
            "admission channel establishment completed"
        );
    }
}

struct ObservedAuthenticatedAdmissionExchange {
    inner: Box<dyn AuthenticatedAdmissionExchangePort>,
    policy: SpaceAdmissionTransportObservationPolicy,
}

#[derive(Clone, Copy)]
pub(crate) struct SpaceAdmissionTransportObservationPolicy {
    record_exchanges: bool,
}

impl SpaceAdmissionTransportObservationPolicy {
    pub(crate) const fn record_safe_message_kind() -> Self {
        Self {
            record_exchanges: true,
        }
    }

    const fn should_record_exchange(self) -> bool {
        self.record_exchanges
    }
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for ObservedAuthenticatedAdmissionExchange {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        self.inner.peer_binding()
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.inner.take_newly_established_continuation()
    }

    async fn exchange(
        self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let result = self.inner.exchange(request).await;
        if self.policy.should_record_exchange() {
            tracing::info!(
                target: "admission.performance",
                operation = SpaceAdmissionTransportOperation::Exchange.as_str(),
                message_kind = ?request.kind(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                outcome = if result.is_ok() { "ok" } else { "error" },
                "admission message exchange completed"
            );
        }
        result
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for ObservedSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let result = self
            .inner
            .establish_initial(admission_id, route, encrypted_password_equivalent)
            .await;
        Self::record_establish(started, "initial", result.is_ok());
        result.map(|inner| {
            Box::new(ObservedAuthenticatedAdmissionExchange {
                inner,
                policy: self.policy,
            }) as _
        })
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let result = self
            .inner
            .resume(admission_id, route, peer_binding, continuation_credential)
            .await;
        Self::record_establish(started, "continuation", result.is_ok());
        result.map(|inner| {
            Box::new(ObservedAuthenticatedAdmissionExchange {
                inner,
                policy: self.policy,
            }) as _
        })
    }
}

#[derive(Clone, Copy)]
enum SponsorAdmissionStateOperation {
    Load,
    Commit,
}

impl SponsorAdmissionStateOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Load => "sponsor_state_load",
            Self::Commit => "sponsor_state_commit",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SponsorAdmissionStateObservationPolicy;

impl SponsorAdmissionStateObservationPolicy {
    pub(crate) const fn record_all() -> Self {
        Self
    }

    const fn should_record(self) -> bool {
        true
    }
}

pub(crate) struct ObservedSponsorAdmissionState {
    inner: Arc<dyn SponsorAdmissionStatePort>,
    policy: SponsorAdmissionStateObservationPolicy,
}

impl ObservedSponsorAdmissionState {
    pub(crate) fn new(
        inner: Arc<dyn SponsorAdmissionStatePort>,
        policy: SponsorAdmissionStateObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record(&self, operation: SponsorAdmissionStateOperation, started: Instant, success: bool) {
        if self.policy.should_record() {
            tracing::info!(
                target: "admission.performance",
                operation = operation.as_str(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                outcome = if success { "ok" } else { "error" },
                "sponsor admission state operation completed"
            );
        }
    }
}

#[async_trait]
impl SponsorAdmissionStatePort for ObservedSponsorAdmissionState {
    async fn load(
        &self,
        message: &AuthenticatedSpaceAdmissionMessage,
    ) -> Result<LoadedSponsorAdmission, SponsorAdmissionStateError> {
        let started = Instant::now();
        let result = self.inner.load(message).await;
        self.record(
            SponsorAdmissionStateOperation::Load,
            started,
            result.is_ok(),
        );
        result
    }

    async fn commit(
        &self,
        token: SponsorAdmissionCommitToken,
        mutation: SponsorAdmissionMutation,
    ) -> Result<CommittedSponsorAdmission, SponsorAdmissionStateError> {
        let started = Instant::now();
        let result = self.inner.commit(token, mutation).await;
        self.record(
            SponsorAdmissionStateOperation::Commit,
            started,
            result.is_ok(),
        );
        result
    }
}

#[derive(Clone, Copy)]
enum JoinerCandidateOperation {
    Prepare,
}

impl JoinerCandidateOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "joiner_candidate_prepare",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct JoinerCandidateObservationPolicy;

impl JoinerCandidateObservationPolicy {
    pub(crate) const fn record_all() -> Self {
        Self
    }

    const fn should_record(self) -> bool {
        true
    }
}

pub(crate) struct ObservedJoinerCandidatePreparation {
    inner: Arc<dyn PrepareJoinerCandidatePort>,
    policy: JoinerCandidateObservationPolicy,
}

impl ObservedJoinerCandidatePreparation {
    pub(crate) fn new(
        inner: Arc<dyn PrepareJoinerCandidatePort>,
        policy: JoinerCandidateObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl PrepareJoinerCandidatePort for ObservedJoinerCandidatePreparation {
    async fn prepare(
        &self,
        preparation: JoinerCandidatePreparation<'_>,
        candidate: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerCandidateMaterial, PrepareJoinerCandidateError> {
        let started = Instant::now();
        let result = self.inner.prepare(preparation, candidate).await;
        if self.policy.should_record() {
            tracing::info!(
                target: "admission.performance",
                operation = JoinerCandidateOperation::Prepare.as_str(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                outcome = if result.is_ok() { "ok" } else { "error" },
                "joiner candidate preparation completed"
            );
        }
        result
    }
}

#[derive(Clone, Copy)]
enum JoinerActivationOperation {
    Prepare,
    StateLoad,
    StateCommit,
    Execute,
}

impl JoinerActivationOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "joiner_activation_prepare",
            Self::StateLoad => "joiner_activation_state_load",
            Self::StateCommit => "joiner_activation_state_commit",
            Self::Execute => "joiner_activation_execute",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct JoinerActivationObservationPolicy {
    suppress_successful_empty_loads: bool,
}

impl JoinerActivationObservationPolicy {
    pub(crate) const fn suppress_successful_empty_loads() -> Self {
        Self {
            suppress_successful_empty_loads: true,
        }
    }

    const fn should_record(self) -> bool {
        true
    }

    const fn should_record_state_load(self, successful: bool, has_value: bool) -> bool {
        !successful || has_value || !self.suppress_successful_empty_loads
    }
}

pub(crate) struct ObservedJoinerActivationPreparation {
    inner: Arc<dyn PrepareJoinerActivationPort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationPreparation {
    pub(crate) fn new(
        inner: Arc<dyn PrepareJoinerActivationPort>,
        policy: JoinerActivationObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl PrepareJoinerActivationPort for ObservedJoinerActivationPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerCompletePreparation<'_>,
        complete: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedJoinerActivation, PrepareJoinerActivationError> {
        let started = Instant::now();
        let result = self
            .inner
            .prepare(admission_id, preparation, complete)
            .await;
        if self.policy.should_record() {
            record_joiner_activation(JoinerActivationOperation::Prepare, started, result.is_ok());
        }
        result
    }
}

pub(crate) struct ObservedJoinerActivationState {
    inner: Arc<dyn JoinerActivationStatePort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationState {
    pub(crate) fn new(
        inner: Arc<dyn JoinerActivationStatePort>,
        policy: JoinerActivationObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl JoinerActivationStatePort for ObservedJoinerActivationState {
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError> {
        let started = Instant::now();
        let result = self.inner.load().await;
        if self
            .policy
            .should_record_state_load(result.is_ok(), matches!(result, Ok(Some(_))))
        {
            record_joiner_activation(
                JoinerActivationOperation::StateLoad,
                started,
                result.is_ok(),
            );
        }
        result
    }

    async fn commit(
        &self,
        token: JoinerActivationCommitToken,
        mutation: JoinerActivationMutation,
    ) -> Result<(), JoinerActivationStateError> {
        let started = Instant::now();
        let result = self.inner.commit(token, mutation).await;
        if self.policy.should_record() {
            record_joiner_activation(
                JoinerActivationOperation::StateCommit,
                started,
                result.is_ok(),
            );
        }
        result
    }
}

pub(crate) struct ObservedJoinerActivationExecutor {
    inner: Arc<dyn ExecuteJoinerActivationPort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationExecutor {
    pub(crate) fn new(
        inner: Arc<dyn ExecuteJoinerActivationPort>,
        policy: JoinerActivationObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }
}

#[async_trait]
impl ExecuteJoinerActivationPort for ObservedJoinerActivationExecutor {
    async fn execute(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: JoinerActivationPreparation<'_>,
    ) -> Result<CompletedJoinerActivation, ExecuteJoinerActivationError> {
        let started = Instant::now();
        let result = self.inner.execute(admission_id, preparation).await;
        if self.policy.should_record() {
            record_joiner_activation(JoinerActivationOperation::Execute, started, result.is_ok());
        }
        result
    }
}

fn record_joiner_activation(operation: JoinerActivationOperation, started: Instant, success: bool) {
    tracing::info!(
        target: "admission.performance",
        operation = operation.as_str(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        outcome = if success { "ok" } else { "error" },
        "joiner activation operation completed"
    );
}

#[derive(Clone, Copy)]
enum SpaceSessionTransitionOperation {
    Preflight,
    Prepare,
    Advance,
    Discard,
}

impl SpaceSessionTransitionOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "space_session_transition_preflight",
            Self::Prepare => "space_session_transition_prepare",
            Self::Advance => "space_session_transition_advance",
            Self::Discard => "space_session_transition_discard",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SpaceSessionTransitionObservationPolicy;

impl SpaceSessionTransitionObservationPolicy {
    pub(crate) const fn record_all() -> Self {
        Self
    }

    const fn should_record(self) -> bool {
        true
    }
}

pub(crate) struct ObservedSpaceSessionTransition {
    inner: Arc<dyn AdmissionSpaceTransitionPort>,
    policy: SpaceSessionTransitionObservationPolicy,
}

impl ObservedSpaceSessionTransition {
    pub(crate) fn new(
        inner: Arc<dyn AdmissionSpaceTransitionPort>,
        policy: SpaceSessionTransitionObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record(&self, operation: SpaceSessionTransitionOperation, started: Instant, success: bool) {
        if self.policy.should_record() {
            tracing::info!(
                target: "admission.performance",
                operation = operation.as_str(),
                elapsed_ms = started.elapsed().as_millis() as u64,
                outcome = if success { "ok" } else { "error" },
                "space session transition operation completed"
            );
        }
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for ObservedSpaceSessionTransition {
    async fn preflight_source_history(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let started = Instant::now();
        let result = self
            .inner
            .preflight_source_history(preserve_unreadable_history)
            .await;
        self.record(
            SpaceSessionTransitionOperation::Preflight,
            started,
            result.is_ok(),
        );
        result
    }

    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        let started = Instant::now();
        let result = self.inner.prepare_if_needed(input).await;
        self.record(
            SpaceSessionTransitionOperation::Prepare,
            started,
            result.is_ok(),
        );
        result
    }

    async fn advance(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let started = Instant::now();
        let result = self.inner.advance(transition).await;
        self.record(
            SpaceSessionTransitionOperation::Advance,
            started,
            result.is_ok(),
        );
        result
    }

    async fn discard_pre_activation(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let started = Instant::now();
        let result = self.inner.discard_pre_activation(transition).await;
        self.record(
            SpaceSessionTransitionOperation::Discard,
            started,
            result.is_ok(),
        );
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionRecoveryObservationPolicy, JoinerActivationObservationPolicy,
        JoinerCandidateObservationPolicy, SpaceAdmissionTransportObservationPolicy,
        SpaceSessionTransitionObservationPolicy, SponsorAdmissionStateObservationPolicy,
    };

    #[test]
    fn recovery_policy_suppresses_only_successful_empty_loads() {
        let policy = AdmissionRecoveryObservationPolicy::suppress_successful_empty_loads();

        assert!(!policy.should_record_load(true, Some(0)));
        assert!(policy.should_record_load(true, Some(1)));
        assert!(policy.should_record_load(false, None));
    }

    #[test]
    fn activation_policy_suppresses_only_successful_empty_state_loads() {
        let policy = JoinerActivationObservationPolicy::suppress_successful_empty_loads();

        assert!(!policy.should_record_state_load(true, false));
        assert!(policy.should_record_state_load(true, true));
        assert!(policy.should_record_state_load(false, false));
    }

    #[test]
    fn record_all_policies_enable_their_operations() {
        assert!(
            SpaceAdmissionTransportObservationPolicy::record_safe_message_kind()
                .should_record_exchange()
        );
        assert!(SponsorAdmissionStateObservationPolicy::record_all().should_record());
        assert!(JoinerCandidateObservationPolicy::record_all().should_record());
        assert!(SpaceSessionTransitionObservationPolicy::record_all().should_record());
    }
}
