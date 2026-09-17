use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::invocation::invoke;
use super::{LifecycleError, LifecycleTarget, RuntimeLifecycleParticipants, TransitionContext};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Active,
    Suspended,
    Incomplete,
    Stopped,
}

struct State {
    phase: Phase,
    generation: u64,
}

/// 固定三类完整能力的转换负责人；不读取参与者内部业务状态。
pub struct RuntimeLifecycle {
    participants: RuntimeLifecycleParticipants,
    state: Mutex<State>,
    stop_requested: AtomicBool,
}

impl RuntimeLifecycle {
    pub(crate) fn new(participants: RuntimeLifecycleParticipants) -> Self {
        Self {
            participants,
            state: Mutex::new(State {
                phase: Phase::Suspended,
                generation: 0,
            }),
            stop_requested: AtomicBool::new(false),
        }
    }

    /// 关闭意图不可撤回；请求失败或等待方离开后也不允许重新启动。
    pub async fn stop(self: &Arc<Self>, deadline: Option<Instant>) -> Result<(), LifecycleError> {
        self.stop_requested.store(true, Ordering::Release);
        self.transition(LifecycleTarget::Suspended, deadline).await
    }

    pub async fn suspend(
        self: &Arc<Self>,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        self.transition(LifecycleTarget::Suspended, deadline).await
    }

    pub async fn resume(
        self: &Arc<Self>,
        deadline: Option<Instant>,
        cancellation: CancellationToken,
    ) -> Result<(), LifecycleError> {
        self.transition_with_cancellation(LifecycleTarget::Active, deadline, cancellation)
            .await
    }

    pub(crate) async fn transition(
        self: &Arc<Self>,
        target: LifecycleTarget,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let owner = Arc::clone(self);
        let cancellation = CancellationToken::new();
        // 执行任务持有负责人；丢弃等待者不会取消已接受的转换或资源收尾。
        tokio::spawn(async move { owner.execute(target, deadline, cancellation).await })
            .await
            .map_err(LifecycleError::from_task_failure)?
    }

    /// 撤销恢复目标只阻止下一项能力；已经开始的动作及必要收尾仍完整等待。
    pub(crate) async fn transition_with_cancellation(
        self: &Arc<Self>,
        target: LifecycleTarget,
        deadline: Option<Instant>,
        cancellation: CancellationToken,
    ) -> Result<(), LifecycleError> {
        let owner = Arc::clone(self);
        // 执行任务持有负责人；丢弃等待者不会取消已接受的转换或资源收尾。
        tokio::spawn(async move { owner.execute(target, deadline, cancellation).await })
            .await
            .map_err(LifecycleError::from_task_failure)?
    }

    async fn execute(
        &self,
        target: LifecycleTarget,
        deadline: Option<Instant>,
        cancellation: CancellationToken,
    ) -> Result<(), LifecycleError> {
        let mut state = self.state.lock().await;
        if target == LifecycleTarget::Active && self.stop_requested.load(Ordering::Acquire) {
            return Err(LifecycleError::stopped());
        }
        if target == LifecycleTarget::Active && cancellation.is_cancelled() {
            return Err(LifecycleError::superseded());
        }
        if state.phase == Phase::Stopped {
            return Ok(());
        }
        if matches!(
            (state.phase, target),
            (Phase::Active, LifecycleTarget::Active)
                | (Phase::Suspended, LifecycleTarget::Suspended)
        ) {
            if state.phase == Phase::Suspended && self.stop_requested.load(Ordering::Acquire) {
                state.phase = Phase::Stopped;
            }
            return Ok(());
        }
        state.generation = state.generation.saturating_add(1);
        let context =
            TransitionContext::new(state.generation, deadline).with_cancellation(cancellation);
        let needs_cleanup = state.phase == Phase::Incomplete;
        state.phase = Phase::Incomplete;
        if needs_cleanup || target == LifecycleTarget::Suspended {
            LifecycleError::from_errors(self.suspend_participants(&context).await)?;
            state.phase = self.stopped_phase();
        }
        if target == LifecycleTarget::Suspended {
            return Ok(());
        }

        // 提前标记未完成，参与者意外 panic 后仍必须先清理才能再次恢复。
        state.phase = Phase::Incomplete;
        if let Err(primary) = self.resume_participants(&context).await {
            let additional = self.suspend_participants(&context).await;
            if additional.is_empty() {
                state.phase = self.stopped_phase();
            }
            return Err(LifecycleError {
                primary,
                additional,
            });
        }
        state.phase = Phase::Active;
        Ok(())
    }

    async fn suspend_participants(&self, context: &TransitionContext) -> Vec<anyhow::Error> {
        // 两类工作先同时收到停止通知；资源仍要等双方完整收尾后才能交接。
        let (session, local) = tokio::join!(
            invoke(
                &self.participants.session_work,
                LifecycleTarget::Suspended,
                context,
            ),
            invoke(
                &self.participants.local_work,
                LifecycleTarget::Suspended,
                context,
            ),
        );
        let mut errors = Vec::new();
        if let Err(error) = session {
            errors.push(error.context("stop session work"));
        }
        if let Err(error) = local {
            errors.push(error.context("stop local work"));
        }
        if errors.is_empty() {
            if let Err(error) = invoke(
                &self.participants.local_resources,
                LifecycleTarget::Suspended,
                context,
            )
            .await
            {
                errors.push(error.context("release local resources"));
            }
        }
        errors
    }

    async fn resume_participants(&self, context: &TransitionContext) -> anyhow::Result<()> {
        self.ensure_running(context)?;
        invoke(
            &self.participants.local_resources,
            LifecycleTarget::Active,
            context,
        )
        .await
        .map_err(|error| error.context("prepare local resources"))?;
        // 会话构造可能使用本地物化，因此先恢复其依赖。
        self.ensure_running(context)?;
        invoke(
            &self.participants.local_work,
            LifecycleTarget::Active,
            context,
        )
        .await
        .map_err(|error| error.context("prepare local work"))?;
        self.ensure_running(context)?;
        invoke(
            &self.participants.session_work,
            LifecycleTarget::Active,
            context,
        )
        .await
        .map_err(|error| error.context("prepare session work"))?;
        self.ensure_running(context)
    }

    fn stopped_phase(&self) -> Phase {
        if self.stop_requested.load(Ordering::Acquire) {
            Phase::Stopped
        } else {
            Phase::Suspended
        }
    }

    fn ensure_running(&self, context: &TransitionContext) -> anyhow::Result<()> {
        if self.stop_requested.load(Ordering::Acquire) {
            return Err(LifecycleError::stopped().primary);
        }
        if context.is_cancelled() {
            return Err(LifecycleError::superseded().primary);
        }
        Ok(())
    }
}
