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
    PrepareJoinerCandidatePort, PrepareSponsorSettledError, PrepareSponsorSettledPort,
    PreparedJoinerActivation, PreparedJoinerCandidateMaterial, PreparedSponsorSettled,
    RePairingStateError, RePairingStateStorePort, SpaceAdmissionAdapters,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort, SponsorAdmissionCommitToken,
    SponsorAdmissionMutation, SponsorAdmissionStateError, SponsorAdmissionStatePort,
};
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    JoinerActivationPreparation, JoinerAdmissionTransition, JoinerCandidatePreparation,
    JoinerCompletePreparation, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute,
    SponsorSettlementPreparation,
};

pub(crate) fn observe_admission(adapters: SpaceAdmissionAdapters) -> SpaceAdmissionAdapters {
    let activation_policy = JoinerActivationObservationPolicy::suppress_successful_empty_loads();
    SpaceAdmissionAdapters {
        re_pairing_state_store: Arc::new(ObservedRePairingStateStore::new(
            adapters.re_pairing_state_store,
        )),
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
        prepare_sponsor_settled: Arc::new(ObservedSponsorSettlementPreparation::new(
            adapters.prepare_sponsor_settled,
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

struct ObservedRePairingStateStore {
    inner: Arc<dyn RePairingStateStorePort>,
}

impl ObservedRePairingStateStore {
    fn new(inner: Arc<dyn RePairingStateStorePort>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl RePairingStateStorePort for ObservedRePairingStateStore {
    async fn is_required(&self) -> Result<bool, RePairingStateError> {
        let started = Instant::now();
        let result = self.inner.is_required().await;
        record_re_pairing_state("re_pairing_state_load", started, result.as_ref().copied());
        result
    }

    async fn set_required(&self, required: bool) -> Result<(), RePairingStateError> {
        let started = Instant::now();
        let result = self.inner.set_required(required).await;
        record_re_pairing_state(
            "re_pairing_state_set",
            started,
            result.as_ref().map(|()| required),
        );
        result
    }
}

fn record_re_pairing_state(
    operation: &'static str,
    started: Instant,
    result: Result<bool, &RePairingStateError>,
) {
    match result {
        Ok(required) => tracing::info!(
            target: "admission.performance",
            operation,
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "ok",
            state = if required { "required" } else { "resolved" },
            "re-pairing state operation completed"
        ),
        Err(error) => tracing::info!(
            target: "admission.performance",
            operation,
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "error",
            error_kind = match error {
                RePairingStateError::Unavailable => "unavailable",
                RePairingStateError::Inconsistent => "inconsistent",
            },
            "re-pairing state operation completed"
        ),
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

    fn record_load(started: Instant, trigger: &'static str, loaded_count: usize) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "ok",
            trigger,
            loaded_count,
            "admission recovery state load completed"
        );
    }

    fn record_load_error(
        started: Instant,
        trigger: &'static str,
        error: &PendingAdmissionRecoveryStateError,
    ) {
        tracing::info!(
            target: "admission.performance",
            operation = AdmissionRecoveryStateOperation::Load.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "error",
            trigger,
            error_kind = admission_recovery_error_kind(error),
            "admission recovery state load failed"
        );
    }

    fn record_commit(started: Instant, error: Option<&PendingAdmissionRecoveryStateError>) {
        match error {
            Some(error) => tracing::info!(
                target: "admission.performance",
                operation = AdmissionRecoveryStateOperation::Commit.as_str(),
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "error",
                error_kind = admission_recovery_error_kind(error),
                "admission recovery state commit completed"
            ),
            None => tracing::info!(
                target: "admission.performance",
                operation = AdmissionRecoveryStateOperation::Commit.as_str(),
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "ok",
                "admission recovery state commit completed"
            ),
        }
    }
}

#[async_trait]
impl PendingAdmissionRecoveryStatePort for ObservedAdmissionRecoveryState {
    async fn load(
        &self,
        trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        let started = Instant::now();
        let trigger_kind = admission_recovery_trigger_kind(trigger);
        let result = self.inner.load(trigger).await;
        match &result {
            Ok(loaded) if self.policy.should_record_load(true, Some(loaded.len())) => {
                Self::record_load(started, trigger_kind, loaded.len());
            }
            Err(error) => Self::record_load_error(started, trigger_kind, error),
            Ok(_) => {}
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
        Self::record_commit(started, result.as_ref().err());
        result
    }
}

fn admission_recovery_trigger_kind(trigger: AdmissionRecoveryTrigger) -> &'static str {
    match trigger {
        AdmissionRecoveryTrigger::Startup => "startup",
        AdmissionRecoveryTrigger::Resume => "resume",
        AdmissionRecoveryTrigger::Periodic => "periodic",
        AdmissionRecoveryTrigger::StateChanged => "state_changed",
        AdmissionRecoveryTrigger::PeerOnline(_) => "peer_online",
    }
}

fn admission_recovery_error_kind(error: &PendingAdmissionRecoveryStateError) -> &'static str {
    match error {
        PendingAdmissionRecoveryStateError::Locked => "locked",
        PendingAdmissionRecoveryStateError::Unavailable => "unavailable",
        PendingAdmissionRecoveryStateError::StateChanged => "state_changed",
        PendingAdmissionRecoveryStateError::RecoveryRequired => "recovery_required",
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

    fn record_establish(
        started: Instant,
        channel: &'static str,
        error: Option<&SpaceAdmissionTransportError>,
    ) {
        match error {
            Some(error) => tracing::info!(
                target: "admission.performance",
                operation = SpaceAdmissionTransportOperation::Establish.as_str(),
                channel,
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "error",
                error_kind = admission_transport_error_kind(error),
                "admission channel establishment completed"
            ),
            None => tracing::info!(
                target: "admission.performance",
                operation = SpaceAdmissionTransportOperation::Establish.as_str(),
                channel,
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "ok",
                "admission channel establishment completed"
            ),
        }
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
            match &result {
                Ok(_) => tracing::info!(
                    target: "admission.performance",
                    operation = SpaceAdmissionTransportOperation::Exchange.as_str(),
                    message_kind = ?request.kind(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "ok",
                    "admission message exchange completed"
                ),
                Err(error) => tracing::info!(
                    target: "admission.performance",
                    operation = SpaceAdmissionTransportOperation::Exchange.as_str(),
                    message_kind = ?request.kind(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "error",
                    error_kind = admission_transport_error_kind(error),
                    "admission message exchange completed"
                ),
            }
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
        Self::record_establish(started, "initial", result.as_ref().err());
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
        Self::record_establish(started, "continuation", result.as_ref().err());
        result.map(|inner| {
            Box::new(ObservedAuthenticatedAdmissionExchange {
                inner,
                policy: self.policy,
            }) as _
        })
    }
}

fn admission_transport_error_kind(error: &SpaceAdmissionTransportError) -> &'static str {
    match error {
        SpaceAdmissionTransportError::Deferred => "deferred",
        SpaceAdmissionTransportError::InvitationUnavailable => "invitation_unavailable",
        SpaceAdmissionTransportError::AuthenticationRejected => "authentication_rejected",
        SpaceAdmissionTransportError::PeerUpgradeRequired => "peer_upgrade_required",
        SpaceAdmissionTransportError::ProtocolRejected => "protocol_rejected",
        SpaceAdmissionTransportError::Unavailable => "unavailable",
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

    fn record(
        &self,
        operation: SponsorAdmissionStateOperation,
        started: Instant,
        error: Option<&SponsorAdmissionStateError>,
    ) {
        if self.policy.should_record() {
            match error {
                Some(error) => tracing::info!(
                    target: "admission.performance",
                    operation = operation.as_str(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "error",
                    error_kind = sponsor_admission_state_error_kind(error),
                    "sponsor admission state operation completed"
                ),
                None => tracing::info!(
                    target: "admission.performance",
                    operation = operation.as_str(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "ok",
                    "sponsor admission state operation completed"
                ),
            }
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
            result.as_ref().err(),
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
            result.as_ref().err(),
        );
        result
    }
}

fn sponsor_admission_state_error_kind(error: &SponsorAdmissionStateError) -> &'static str {
    match error {
        SponsorAdmissionStateError::Locked { .. } => "locked",
        SponsorAdmissionStateError::StateChanged { .. } => "state_changed",
        SponsorAdmissionStateError::RecoveryRequired { .. } => "recovery_required",
        SponsorAdmissionStateError::Unavailable { .. } => "unavailable",
    }
}

#[derive(Clone, Copy)]
enum SponsorSettlementOperation {
    Prepare,
}

impl SponsorSettlementOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "sponsor_settlement_prepare",
        }
    }
}

struct ObservedSponsorSettlementPreparation {
    inner: Arc<dyn PrepareSponsorSettledPort>,
}

impl ObservedSponsorSettlementPreparation {
    fn new(inner: Arc<dyn PrepareSponsorSettledPort>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl PrepareSponsorSettledPort for ObservedSponsorSettlementPreparation {
    async fn prepare(
        &self,
        admission_id: SpaceAdmissionId,
        preparation: SponsorSettlementPreparation<'_>,
        complete_ack: &SpaceAdmissionEnvelopeV1,
    ) -> Result<PreparedSponsorSettled, PrepareSponsorSettledError> {
        let started = Instant::now();
        let result = self
            .inner
            .prepare(admission_id, preparation, complete_ack)
            .await;
        match &result {
            Ok(_) => tracing::info!(
                target: "admission.performance",
                operation = SponsorSettlementOperation::Prepare.as_str(),
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "ok",
                "sponsor settlement preparation completed"
            ),
            Err(error) => tracing::info!(
                target: "admission.performance",
                operation = SponsorSettlementOperation::Prepare.as_str(),
                elapsed_ms = duration_ms(started.elapsed()),
                outcome = "error",
                error_kind = prepare_sponsor_settled_error_kind(error),
                "sponsor settlement preparation completed"
            ),
        }
        result
    }
}

fn prepare_sponsor_settled_error_kind(error: &PrepareSponsorSettledError) -> &'static str {
    match error {
        PrepareSponsorSettledError::Invalid { .. } => "invalid",
        PrepareSponsorSettledError::Unavailable { .. } => "unavailable",
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
            match &result {
                Ok(_) => tracing::info!(
                    target: "admission.performance",
                    operation = JoinerCandidateOperation::Prepare.as_str(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "ok",
                    "joiner candidate preparation completed"
                ),
                Err(error) => tracing::info!(
                    target: "admission.performance",
                    operation = JoinerCandidateOperation::Prepare.as_str(),
                    elapsed_ms = duration_ms(started.elapsed()),
                    outcome = "error",
                    error_kind = prepare_joiner_candidate_error_kind(error),
                    "joiner candidate preparation completed"
                ),
            }
        }
        result
    }
}

fn prepare_joiner_candidate_error_kind(error: &PrepareJoinerCandidateError) -> &'static str {
    match error {
        PrepareJoinerCandidateError::Invalid
        | PrepareJoinerCandidateError::InvalidSource { .. } => "invalid",
        PrepareJoinerCandidateError::Unavailable { .. } => "unavailable",
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
            record_joiner_activation(
                JoinerActivationOperation::Prepare,
                started,
                result
                    .as_ref()
                    .err()
                    .map(prepare_joiner_activation_error_kind),
            );
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
                result
                    .as_ref()
                    .err()
                    .map(joiner_activation_state_error_kind),
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
                result
                    .as_ref()
                    .err()
                    .map(joiner_activation_state_error_kind),
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
            record_joiner_activation(
                JoinerActivationOperation::Execute,
                started,
                result
                    .as_ref()
                    .err()
                    .map(execute_joiner_activation_error_kind),
            );
        }
        result
    }
}

fn record_joiner_activation(
    operation: JoinerActivationOperation,
    started: Instant,
    error_kind: Option<&'static str>,
) {
    match error_kind {
        Some(error_kind) => tracing::info!(
            target: "admission.performance",
            operation = operation.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "error",
            error_kind,
            "joiner activation operation completed"
        ),
        None => tracing::info!(
            target: "admission.performance",
            operation = operation.as_str(),
            elapsed_ms = duration_ms(started.elapsed()),
            outcome = "ok",
            "joiner activation operation completed"
        ),
    }
}

fn prepare_joiner_activation_error_kind(error: &PrepareJoinerActivationError) -> &'static str {
    match error {
        PrepareJoinerActivationError::Invalid { .. } => "invalid",
        PrepareJoinerActivationError::Unavailable { .. } => "unavailable",
    }
}

fn joiner_activation_state_error_kind(error: &JoinerActivationStateError) -> &'static str {
    match error {
        JoinerActivationStateError::Locked { .. } => "locked",
        JoinerActivationStateError::StateChanged { .. } => "state_changed",
        JoinerActivationStateError::RecoveryRequired { .. } => "recovery_required",
        JoinerActivationStateError::Unavailable { .. } => "unavailable",
    }
}

fn execute_joiner_activation_error_kind(error: &ExecuteJoinerActivationError) -> &'static str {
    match error {
        ExecuteJoinerActivationError::Invalid { .. } => "invalid",
        ExecuteJoinerActivationError::Unavailable { .. } => "unavailable",
    }
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
        AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
        ExecuteJoinerActivationError, JoinerActivationStateError,
        PendingAdmissionRecoveryStateError, PrepareJoinerActivationError,
        PrepareJoinerCandidateError, PrepareSponsorSettledError, RePairingStateError,
        RePairingStateStorePort, SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
        SponsorAdmissionStateError,
    };
    use uc_core::membership::{
        AdmissionChannelPeerId, AdmissionContinuationCredential,
        AdmissionEncryptedPasswordEquivalent, AdmissionMessageId, AdmissionPeerBinding,
        AdmissionRole, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
        SpaceAdmissionRoute,
    };

    use super::{
        admission_recovery_error_kind, admission_recovery_trigger_kind,
        admission_transport_error_kind, execute_joiner_activation_error_kind,
        joiner_activation_state_error_kind, prepare_joiner_activation_error_kind,
        prepare_joiner_candidate_error_kind, prepare_sponsor_settled_error_kind,
        sponsor_admission_state_error_kind, AdmissionRecoveryObservationPolicy,
        JoinerActivationObservationPolicy, JoinerCandidateObservationPolicy,
        ObservedRePairingStateStore, ObservedSpaceAdmissionTransport,
        SpaceAdmissionTransportObservationPolicy, SponsorAdmissionStateObservationPolicy,
        SponsorSettlementOperation,
    };

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for CapturedWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("captured writer lock")
                .extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CapturedWriter {
        type Writer = CapturedWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            self.clone()
        }
    }

    impl CapturedWriter {
        fn output(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured writer lock").clone())
                .expect("captured events should be UTF-8")
        }
    }

    struct TestRePairingStateStore {
        required: bool,
        fail_write: bool,
    }

    #[async_trait]
    impl RePairingStateStorePort for TestRePairingStateStore {
        async fn is_required(&self) -> Result<bool, RePairingStateError> {
            Ok(self.required)
        }

        async fn set_required(&self, _required: bool) -> Result<(), RePairingStateError> {
            if self.fail_write {
                Err(RePairingStateError::Inconsistent)
            } else {
                Ok(())
            }
        }
    }

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

    #[test]
    fn admission_errors_map_to_stable_safe_kinds() {
        assert_eq!(
            admission_recovery_error_kind(&PendingAdmissionRecoveryStateError::StateChanged),
            "state_changed"
        );
        assert_eq!(
            admission_transport_error_kind(&SpaceAdmissionTransportError::Deferred),
            "deferred"
        );
        assert_eq!(
            sponsor_admission_state_error_kind(&SponsorAdmissionStateError::recovery_required(
                anyhow::anyhow!("SECRET_SPONSOR_ERROR")
            )),
            "recovery_required"
        );
        assert_eq!(
            prepare_joiner_candidate_error_kind(&PrepareJoinerCandidateError::invalid(
                anyhow::anyhow!("SECRET_CANDIDATE_ERROR")
            )),
            "invalid"
        );
        assert_eq!(
            prepare_joiner_activation_error_kind(&PrepareJoinerActivationError::unavailable(
                anyhow::anyhow!("SECRET_PREPARE_ERROR")
            )),
            "unavailable"
        );
        assert_eq!(
            joiner_activation_state_error_kind(&JoinerActivationStateError::locked(
                anyhow::anyhow!("SECRET_STATE_ERROR")
            )),
            "locked"
        );
        assert_eq!(
            execute_joiner_activation_error_kind(&ExecuteJoinerActivationError::invalid(
                anyhow::anyhow!("SECRET_EXECUTE_ERROR")
            )),
            "invalid"
        );
        assert_eq!(
            prepare_sponsor_settled_error_kind(&PrepareSponsorSettledError::invalid(
                anyhow::anyhow!("SECRET_SETTLED_ERROR")
            )),
            "invalid"
        );
    }

    #[test]
    fn admission_recovery_triggers_map_without_peer_identity() {
        assert_eq!(
            admission_recovery_trigger_kind(AdmissionRecoveryTrigger::Startup),
            "startup"
        );
        assert_eq!(
            admission_recovery_trigger_kind(AdmissionRecoveryTrigger::StateChanged),
            "state_changed"
        );
        assert_eq!(
            admission_recovery_trigger_kind(AdmissionRecoveryTrigger::Periodic),
            "periodic"
        );
        assert_eq!(
            admission_recovery_trigger_kind(AdmissionRecoveryTrigger::PeerOnline(
                uc_core::ids::DeviceId::new("SECRET_DEVICE")
            )),
            "peer_online"
        );
    }

    #[test]
    fn sponsor_settlement_operation_name_is_stable() {
        assert_eq!(
            SponsorSettlementOperation::Prepare.as_str(),
            "sponsor_settlement_prepare"
        );
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

    #[test]
    fn re_pairing_state_observation_records_safe_results_and_error_kinds() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime");

        tracing::dispatcher::with_default(&dispatch, || {
            runtime.block_on(async {
                let successful =
                    ObservedRePairingStateStore::new(Arc::new(TestRePairingStateStore {
                        required: true,
                        fail_write: false,
                    }));
                assert!(successful.is_required().await.expect("state load"));
                successful
                    .set_required(false)
                    .await
                    .expect("state resolution");

                let failing = ObservedRePairingStateStore::new(Arc::new(TestRePairingStateStore {
                    required: false,
                    fail_write: true,
                }));
                assert_eq!(
                    failing.set_required(false).await,
                    Err(RePairingStateError::Inconsistent)
                );
            });
        });

        let output = writer.output();
        assert!(output.contains("operation=\"re_pairing_state_load\""));
        assert!(output.contains("state=\"required\""));
        assert!(output.contains("operation=\"re_pairing_state_set\""));
        assert!(output.contains("state=\"resolved\""));
        assert!(output.contains("outcome=\"ok\""));
        assert!(output.contains("outcome=\"error\""));
        assert!(output.contains("error_kind=\"inconsistent\""));
    }
}
