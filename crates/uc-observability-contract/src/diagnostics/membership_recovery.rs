//! 完整成员恢复的不可读取观测作用域；不接收业务身份、账本或内部步骤。

use super::*;

tokio::task_local! {
    static RECOVERY_TRIGGER: MembershipRecoveryTrigger;
    static RECOVERY_CONFLICT: Arc<AtomicBool>;
}

#[derive(Clone, Copy)]
pub enum MembershipRecoveryTrigger {
    Startup,
    Resume,
    PeerOnline,
    Retry,
    StateChanged,
    Requested,
}

pub async fn scope_membership_recovery_trigger<F: Future>(
    trigger: MembershipRecoveryTrigger,
    future: F,
) -> F::Output {
    RECOVERY_TRIGGER.scope(trigger, future).await
}

pub(super) fn recovery_name() -> &'static str {
    match RECOVERY_TRIGGER
        .try_with(|value| *value)
        .unwrap_or(MembershipRecoveryTrigger::Requested)
    {
        MembershipRecoveryTrigger::Startup => "membership.recover.startup",
        MembershipRecoveryTrigger::Resume => "membership.recover.resume",
        MembershipRecoveryTrigger::PeerOnline => "membership.recover.peer_online",
        MembershipRecoveryTrigger::Retry => "membership.recover.retry",
        MembershipRecoveryTrigger::StateChanged => "membership.recover.state_changed",
        MembershipRecoveryTrigger::Requested => "membership.recover.requested",
    }
}

pub fn describe_membership_conflict() {
    let _ = RECOVERY_CONFLICT.try_with(|flag| flag.store(true, Ordering::Relaxed));
}

#[derive(Clone, Copy)]
pub enum MembershipRecoveryOutcome {
    NoWork,
    Completed,
    Partial,
    Deferred,
    Failed,
    Corrupt,
    Cancelled,
}

pub struct MembershipRecoveryObservation {
    span: tracing::Span,
    started: Instant,
    finished: AtomicBool,
    conflict: Arc<AtomicBool>,
}

impl MembershipRecoveryObservation {
    pub fn begin() -> Self {
        Self {
            span: operation_span(OperationContext {
                domain: DiagnosticDomain::SpaceMembership,
                operation: DiagnosticOperation::MembershipRecovery,
                role: DiagnosticRole::Local,
                kind: DiagnosticSpanKind::Internal,
            }),
            started: Instant::now(),
            finished: AtomicBool::new(false),
            conflict: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn scope<F: Future>(&self, future: F) -> F::Output {
        RECOVERY_CONFLICT
            .scope(self.conflict.clone(), future.instrument(self.span.clone()))
            .await
    }

    pub fn finish(&self, outcome: MembershipRecoveryOutcome) {
        if self.finished.swap(true, Ordering::Relaxed) {
            return;
        }
        if matches!(outcome, MembershipRecoveryOutcome::NoWork) {
            // 空检查有意省略，也不产生引用该节点的完成日志。
            self.span.record("uc.outcome", "skipped");
            return;
        }
        let domain = DiagnosticDomain::SpaceMembership;
        let operation = DiagnosticOperation::MembershipRecovery;
        let role = DiagnosticRole::Local;
        let elapsed = self.started.elapsed();
        let completion = match outcome {
            MembershipRecoveryOutcome::Completed => {
                OperationCompletion::succeeded(domain, operation, role, elapsed)
            }
            MembershipRecoveryOutcome::Partial => {
                OperationCompletion::partial(domain, operation, role, elapsed)
            }
            MembershipRecoveryOutcome::Deferred => {
                OperationCompletion::deferred(domain, operation, role, elapsed)
            }
            MembershipRecoveryOutcome::Failed if self.conflict.load(Ordering::Relaxed) => {
                OperationCompletion::conflict(domain, operation, role, elapsed)
            }
            MembershipRecoveryOutcome::Failed => OperationCompletion::failed(
                domain,
                operation,
                role,
                DiagnosticErrorType::MembershipRecoveryFailed,
                elapsed,
            ),
            MembershipRecoveryOutcome::Corrupt => OperationCompletion::failed(
                domain,
                operation,
                role,
                DiagnosticErrorType::Corrupt,
                elapsed,
            ),
            MembershipRecoveryOutcome::Cancelled => {
                OperationCompletion::cancelled(domain, operation, role, elapsed)
            }
            MembershipRecoveryOutcome::NoWork => return,
        };
        self.span.in_scope(|| complete_operation(completion));
    }
}

impl Drop for MembershipRecoveryObservation {
    fn drop(&mut self) {
        self.finish(MembershipRecoveryOutcome::Cancelled);
    }
}
