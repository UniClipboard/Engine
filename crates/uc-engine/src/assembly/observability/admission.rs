use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use uc_application::deps::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort,
    AuthenticatedAdmissionReply, AuthenticatedSpaceAdmissionMessage, CommittedSponsorAdmission,
    CompletedJoinerActivation, ExecuteJoinerActivationError, ExecuteJoinerActivationPort,
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationStateError,
    JoinerActivationStatePort, LoadedJoinerActivation, LoadedPendingAdmission,
    LoadedSponsorAdmission, PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
    PrepareJoinerActivationError, PrepareJoinerActivationPort, PrepareJoinerCandidateError,
    PrepareJoinerCandidatePort, PreparedJoinerActivation, PreparedJoinerCandidateMaterial,
    SpaceAdmissionAdapters, SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
    SponsorAdmissionCommitToken, SponsorAdmissionMutation, SponsorAdmissionStateError,
    SponsorAdmissionStatePort,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    JoinerActivationPreparation, JoinerAdmissionTransition, JoinerCandidatePreparation,
    JoinerCompletePreparation, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute,
};

pub(crate) fn observe_admission(adapters: SpaceAdmissionAdapters) -> SpaceAdmissionAdapters {
    let activation_policy = JoinerActivationObservationPolicy::suppress_successful_empty_loads();
    SpaceAdmissionAdapters {
        pending_admission_recovery_state: Arc::new(ObservedAdmissionRecoveryState::new(
            adapters.pending_admission_recovery_state,
            AdmissionRecoveryObservationPolicy::suppress_successful_empty_loads(),
        )),
        space_admission_transport: Arc::new(ObservedSpaceAdmissionTransport::new(
            adapters.space_admission_transport,
            SpaceAdmissionTransportObservationPolicy::record_safe_message_kind(),
        )),
        sponsor_admission_state: Arc::new(ObservedSponsorAdmissionState::new(
            adapters.sponsor_admission_state,
            SponsorAdmissionStateObservationPolicy::record_all(),
        )),
        prepare_joiner_candidate: Arc::new(ObservedJoinerCandidatePreparation::new(
            adapters.prepare_joiner_candidate,
            JoinerCandidateObservationPolicy::record_all(),
        )),
        prepare_joiner_activation: Arc::new(ObservedJoinerActivationPreparation::new(
            adapters.prepare_joiner_activation,
            activation_policy,
        )),
        joiner_activation_state: Arc::new(ObservedJoinerActivationState::new(
            adapters.joiner_activation_state,
            activation_policy,
        )),
        execute_joiner_activation: Arc::new(ObservedJoinerActivationExecutor::new(
            adapters.execute_joiner_activation,
            activation_policy,
        )),
        ..adapters
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
struct AdmissionRecoveryObservationPolicy {
    suppress_successful_empty_loads: bool,
}

impl AdmissionRecoveryObservationPolicy {
    const fn suppress_successful_empty_loads() -> Self {
        Self {
            suppress_successful_empty_loads: true,
        }
    }

    const fn should_record_load(self, successful: bool, loaded_count: Option<usize>) -> bool {
        !successful || !self.suppress_successful_empty_loads || !matches!(loaded_count, Some(0))
    }
}

struct ObservedAdmissionRecoveryState {
    inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
    policy: AdmissionRecoveryObservationPolicy,
}

impl ObservedAdmissionRecoveryState {
    fn new(
        inner: Arc<dyn PendingAdmissionRecoveryStatePort>,
        policy: AdmissionRecoveryObservationPolicy,
    ) -> Self {
        Self { inner, policy }
    }

    fn record_load(started: Instant, loaded_count: usize) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "ok",
            loaded_count,
            "admission recovery state load completed"
        );
    }

    fn record_load_error(started: Instant) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "error",
            "admission recovery state load failed"
        );
    }

    fn record_commit(started: Instant, success: bool) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Commit.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
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
struct ObservedSpaceAdmissionTransport {
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
    fn new(
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
            elapsed_ms = duration_ms(started.elapsed()),
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
struct SpaceAdmissionTransportObservationPolicy {
    record_exchanges: bool,
}

impl SpaceAdmissionTransportObservationPolicy {
    const fn record_safe_message_kind() -> Self {
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
                elapsed_ms = duration_ms(started.elapsed()),
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
struct SponsorAdmissionStateObservationPolicy;

impl SponsorAdmissionStateObservationPolicy {
    const fn record_all() -> Self {
        Self
    }

    const fn should_record(self) -> bool {
        true
    }
}

struct ObservedSponsorAdmissionState {
    inner: Arc<dyn SponsorAdmissionStatePort>,
    policy: SponsorAdmissionStateObservationPolicy,
}

impl ObservedSponsorAdmissionState {
    fn new(
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
                elapsed_ms = duration_ms(started.elapsed()),
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
struct JoinerCandidateObservationPolicy;

impl JoinerCandidateObservationPolicy {
    const fn record_all() -> Self {
        Self
    }

    const fn should_record(self) -> bool {
        true
    }
}

struct ObservedJoinerCandidatePreparation {
    inner: Arc<dyn PrepareJoinerCandidatePort>,
    policy: JoinerCandidateObservationPolicy,
}

impl ObservedJoinerCandidatePreparation {
    fn new(
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
                elapsed_ms = duration_ms(started.elapsed()),
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
struct JoinerActivationObservationPolicy {
    suppress_successful_empty_loads: bool,
}

impl JoinerActivationObservationPolicy {
    const fn suppress_successful_empty_loads() -> Self {
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

struct ObservedJoinerActivationPreparation {
    inner: Arc<dyn PrepareJoinerActivationPort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationPreparation {
    fn new(
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

struct ObservedJoinerActivationState {
    inner: Arc<dyn JoinerActivationStatePort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationState {
    fn new(
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

struct ObservedJoinerActivationExecutor {
    inner: Arc<dyn ExecuteJoinerActivationPort>,
    policy: JoinerActivationObservationPolicy,
}

impl ObservedJoinerActivationExecutor {
    fn new(
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
        elapsed_ms = duration_ms(started.elapsed()),
        outcome = if success { "ok" } else { "error" },
        "joiner activation operation completed"
    );
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use uc_application::deps::{
        AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
        SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
    };
    use uc_core::membership::{
        AdmissionChannelPeerId, AdmissionContinuationCredential,
        AdmissionEncryptedPasswordEquivalent, AdmissionMessageId, AdmissionPeerBinding,
        AdmissionRole, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
        SpaceAdmissionRoute,
    };

    use super::{
        AdmissionRecoveryObservationPolicy, JoinerActivationObservationPolicy,
        JoinerCandidateObservationPolicy, ObservedSpaceAdmissionTransport,
        SpaceAdmissionTransportObservationPolicy, SponsorAdmissionStateObservationPolicy,
    };

    struct CountingAdmissionTransport {
        establish_calls: Arc<AtomicUsize>,
        exchange_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl SpaceAdmissionTransportPort for CountingAdmissionTransport {
        async fn establish_initial(
            &self,
            _admission_id: SpaceAdmissionId,
            _route: &SpaceAdmissionRoute,
            _encrypted_password_equivalent: &AdmissionEncryptedPasswordEquivalent,
        ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError>
        {
            self.establish_calls.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(CountingAuthenticatedExchange {
                exchange_calls: Arc::clone(&self.exchange_calls),
            }))
        }

        async fn resume(
            &self,
            _admission_id: SpaceAdmissionId,
            _route: &SpaceAdmissionRoute,
            _peer_binding: AdmissionPeerBinding,
            _continuation_credential: &AdmissionContinuationCredential,
        ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError>
        {
            Err(SpaceAdmissionTransportError::Deferred)
        }
    }

    struct CountingAuthenticatedExchange {
        exchange_calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AuthenticatedAdmissionExchangePort for CountingAuthenticatedExchange {
        fn peer_binding(&self) -> AdmissionPeerBinding {
            AdmissionPeerBinding::new(
                AdmissionChannelPeerId::from_bytes([1; 32]).expect("valid local peer"),
                AdmissionChannelPeerId::from_bytes([2; 32]).expect("valid remote peer"),
            )
            .expect("distinct peers")
        }

        fn take_newly_established_continuation(
            &mut self,
        ) -> Option<AdmissionContinuationCredential> {
            None
        }

        async fn exchange(
            self: Box<Self>,
            _request: &SpaceAdmissionEnvelopeV1,
        ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
            self.exchange_calls.fetch_add(1, Ordering::SeqCst);
            Err(SpaceAdmissionTransportError::ProtocolRejected)
        }
    }

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
    }

    #[tokio::test]
    async fn authenticated_exchange_remains_wrapped_and_transparent() {
        let establish_calls = Arc::new(AtomicUsize::new(0));
        let exchange_calls = Arc::new(AtomicUsize::new(0));
        let observed = ObservedSpaceAdmissionTransport::new(
            Arc::new(CountingAdmissionTransport {
                establish_calls: Arc::clone(&establish_calls),
                exchange_calls: Arc::clone(&exchange_calls),
            }),
            SpaceAdmissionTransportObservationPolicy::record_safe_message_kind(),
        );
        let admission_id =
            SpaceAdmissionId::from_bytes([3; 32]).expect("valid admission identifier");
        let route = SpaceAdmissionRoute::from_bytes(vec![4; 32]).expect("valid admission route");
        let password = AdmissionEncryptedPasswordEquivalent::from_bytes(vec![5; 64])
            .expect("valid password equivalent");

        let mut exchange = observed
            .establish_initial(admission_id, &route, &password)
            .await
            .expect("test transport establishes");
        assert_eq!(establish_calls.load(Ordering::SeqCst), 1);
        assert_eq!(exchange.peer_binding().local_peer_id().as_bytes(), &[1; 32]);
        assert!(exchange.take_newly_established_continuation().is_none());

        let request = SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            AdmissionMessageId::from_bytes([6; 32]).expect("valid message identifier"),
            Some(AdmissionMessageId::from_bytes([7; 32]).expect("valid predecessor")),
            SpaceAdmissionBodyV1::CancelRequested,
        )
        .expect("valid cancellation request");
        let result = exchange.exchange(&request).await;

        assert!(matches!(
            result,
            Err(SpaceAdmissionTransportError::ProtocolRejected)
        ));
        assert_eq!(exchange_calls.load(Ordering::SeqCst), 1);
    }
}
