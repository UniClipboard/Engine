use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
#[cfg(feature = "dev-tools")]
use std::sync::atomic::Ordering;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub(crate) mod event_stream;
mod in_flight;
mod lifecycle;
#[cfg(test)]
mod lifecycle_tests;
mod log_activity;
mod operation;
#[cfg(test)]
mod operation_tests;
mod owned_executor;
#[cfg(feature = "test-binding-worker-panic")]
mod probe_startup_host;
mod shutdown;
#[cfg(test)]
mod shutdown_tests;
pub(crate) mod startup;
mod startup_control;
mod startup_outcome;
pub(crate) mod startup_owner;

use crate::runtime::{RecoverableRuntime, ReusableHost};
#[cfg(feature = "dev-tools")]
use crate::{DevOperation, DevOperationResult};
use crate::{
    EngineConfig, EngineError, EngineErrorCategory, EngineState, HostCapabilities, Operation,
    OperationResult, OperationTerminal,
};
pub use event_stream::EventStream;
use event_stream::{event_channel, EventSender};
use in_flight::InFlightOperations;
use lifecycle::TransitionQueue;
use log_activity::LogActivity;
use owned_executor::OwnedExecutor;
#[cfg(feature = "test-binding-worker-panic")]
pub use probe_startup_host::{
    probe_release_startup_retry, probe_startup_retry_calls, probe_startup_retry_entered,
    probe_wrap_startup_host,
};
use shutdown::{OrphanRetention, ShutdownProgress};
pub use startup::{StartupProgress, StartupProgressInput};
#[cfg(feature = "test-binding-worker-panic")]
pub use startup_control::StartupProbeOutcome;
pub use startup_control::{StartupLifecycle, StartupLifecycleInput, StartupReleaseOutcome};
pub(crate) use startup_outcome::StartupOutcomeSnapshot;
use startup_owner::UnpolledStartupCleanup;

const INVALID_STATE_CODE: u32 = 1001;
const OPERATION_CANCELLED_CODE: u32 = 1002;

#[cfg(feature = "dev-tools")]
#[doc(hidden)]
pub fn probe_fail_next_orphan_retry_executor() {
    shutdown::PROBE_ORPHAN_RETRY_EXECUTOR_FAILURE.store(true, Ordering::SeqCst);
}

#[async_trait]
pub(crate) trait EngineRuntime: Send + Sync {
    async fn execute(
        &self,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError>;

    #[cfg(feature = "dev-tools")]
    async fn execute_dev(
        &self,
        _operation: DevOperation,
        _cancellation: CancellationToken,
    ) -> Result<DevOperationResult, EngineError> {
        Err(EngineError::new(
            1901,
            EngineErrorCategory::Unavailable,
            false,
        ))
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError>;
    async fn resume(&self, cancellation: CancellationToken) -> Result<(), EngineError>;
    async fn shutdown(&self, deadline: Option<Instant>) -> Result<(), EngineError>;

    /// 等待已接受的工作结算；本动作不重新执行关闭，也不确认资源释放。
    async fn wait_shutdown_obligations(&self) {}
    /// 进程工作线程的同步等待，不创建新的异步执行器。
    fn wait_shutdown_obligations_blocking(&self) {}

    async fn supports_shutdown_retry(&self) -> bool {
        false
    }
}

#[cfg(feature = "dev-tools")]
pub(crate) fn probe_fail_owned_start(stage: u8) {
    owned_executor::PROBE_START_FAILURE.store(stage, Ordering::Release);
}

#[cfg(feature = "dev-tools")]
pub(crate) fn probe_fail_log_begin() {
    log_activity::PROBE_BEGIN_FAILURE.store(true, Ordering::Release);
}

#[cfg(feature = "dev-tools")]
static PROBE_HOLD_LIFECYCLE: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "dev-tools")]
static PROBE_LIFECYCLE_HELD: AtomicBool = AtomicBool::new(false);

pub struct Engine {
    state: Arc<Mutex<EngineState>>,
    lifecycle_gate: Arc<Mutex<()>>,
    lifecycle_requests: Arc<TransitionQueue>,
    shutdown_gate: Arc<Mutex<()>>,
    shutdown_progress: Arc<Mutex<ShutdownProgress>>,
    orphan_retention: Arc<OrphanRetention>,
    retain_on_drop: bool,
    stop_requested: Arc<AtomicBool>,
    runtime: Arc<dyn EngineRuntime>,
    owned_executor: Option<Arc<OwnedExecutor>>,
    events: EventSender,
    operations: Arc<InFlightOperations>,
    log_activity: LogActivity,
    /// 暂停结果核对打开文件时检查的宿主目录。
    file_roots: Arc<[PathBuf]>,
}

impl Engine {
    pub fn start(
        config: EngineConfig,
        host: HostCapabilities,
    ) -> impl Future<Output = Result<(Self, EventStream), EngineError>> + Send {
        let (input, _) = StartupProgress::channel();
        Self::start_with_progress(config, host, input)
    }

    /// 启动前交入只读进度通道；关闭观察者不影响本次启动。
    pub fn start_with_progress(
        config: EngineConfig,
        host: HostCapabilities,
        progress: StartupProgressInput,
    ) -> impl Future<Output = Result<(Self, EventStream), EngineError>> + Send {
        let (lifecycle, _) = StartupLifecycle::channel();
        Self::start_with_lifecycle(config, host, progress, lifecycle)
    }

    /// 创建 future 时接管宿主能力；构造完成后先处理已接收的通知再交出实例。
    pub fn start_with_lifecycle(
        config: EngineConfig,
        host: HostCapabilities,
        mut progress: StartupProgressInput,
        lifecycle: StartupLifecycleInput,
    ) -> impl Future<Output = Result<(Self, EventStream), EngineError>> + Send {
        let host = Arc::new(ReusableHost::new(host));
        lifecycle.cleanup.accept_host(Arc::clone(&host));
        lifecycle
            .cleanup
            .register_requests(Arc::clone(&lifecycle.requests));
        lifecycle
            .cleanup
            .register_progress(Arc::clone(&progress.store));
        progress.track_cleanup(&lifecycle.cleanup);
        let unpolled =
            UnpolledStartupCleanup::new(Arc::clone(&lifecycle.cleanup), Arc::clone(&host));
        Self::start_owned(config, host, progress, lifecycle, unpolled)
    }

    async fn start_runtime(
        config: EngineConfig,
        host: Arc<ReusableHost>,
        progress: &StartupProgressInput,
        requests: Arc<TransitionQueue>,
        cleanup: Arc<startup_owner::StartupCleanupOwner>,
        owned_executor: Arc<OwnedExecutor>,
    ) -> Result<(Self, EventStream), EngineError> {
        const EVENT_CAPACITY: usize = 256;

        let (events, stream) = event_channel(EVENT_CAPACITY);
        let directories = host.directories();
        let file_roots: Arc<[PathBuf]> = [
            directories.private_data(),
            directories.cache(),
            directories.temporary(),
            directories.logs(),
        ]
        .into_iter()
        .map(Path::to_path_buf)
        .collect();
        let runtime = Arc::new(
            RecoverableRuntime::start(
                config,
                host,
                events.clone(),
                Arc::clone(&progress.store),
                Arc::clone(&cleanup),
            )
            .await?,
        );
        let log_activity = LogActivity::default();
        cleanup.accept_runtime(Arc::clone(&runtime), log_activity.clone());
        if let Err(error) = log_activity.begin().await {
            cleanup.record_primary(error.clone());
            cleanup.start_runtime_cleanup(Arc::clone(&runtime), log_activity);
            return Err(error);
        }
        cleanup.finish_runtime_construction();
        let engine = Self {
            state: Arc::new(Mutex::new(EngineState::Running)),
            lifecycle_gate: Arc::new(Mutex::new(())),
            lifecycle_requests: requests,
            shutdown_gate: Arc::new(Mutex::new(())),
            shutdown_progress: Arc::new(Mutex::new(ShutdownProgress::default())),
            orphan_retention: Arc::new(OrphanRetention::default()),
            retain_on_drop: true,
            stop_requested: Arc::new(AtomicBool::new(false)),
            runtime,
            owned_executor: Some(owned_executor),
            events,
            operations: Arc::new(InFlightOperations::new()),
            log_activity,
            file_roots,
        };
        let startup_handoff = engine.bind_lifecycle(true);
        startup_handoff
            .await
            // discarded-source[channel]: `tokio::sync::oneshot::error::RecvError`: the error only means the peer is gone or carries the unsent payload, which must not outlive it
            .map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?;
        Ok((engine, stream))
    }

    #[cfg(test)]
    fn from_runtime<R>(runtime: Arc<R>, event_capacity: usize) -> (Self, EventStream)
    where
        R: EngineRuntime + 'static,
    {
        Self::from_runtime_with_queue(
            runtime,
            event_capacity,
            Arc::new(TransitionQueue::default()),
        )
    }

    #[cfg(test)]
    fn from_runtime_with_queue<R>(
        runtime: Arc<R>,
        event_capacity: usize,
        requests: Arc<TransitionQueue>,
    ) -> (Self, EventStream)
    where
        R: EngineRuntime + 'static,
    {
        let (events, stream) = event_channel(event_capacity);
        let engine = Self {
            state: Arc::new(Mutex::new(EngineState::Running)),
            lifecycle_gate: Arc::new(Mutex::new(())),
            lifecycle_requests: requests,
            shutdown_gate: Arc::new(Mutex::new(())),
            shutdown_progress: Arc::new(Mutex::new(ShutdownProgress::default())),
            orphan_retention: Arc::new(OrphanRetention::default()),
            retain_on_drop: false,
            stop_requested: Arc::new(AtomicBool::new(false)),
            runtime,
            owned_executor: None,
            events,
            operations: Arc::new(InFlightOperations::new()),
            log_activity: LogActivity::default(),
            file_roots: Arc::from([]),
        };
        let _ = engine.bind_lifecycle(false);
        (engine, stream)
    }

    #[cfg(feature = "dev-tools")]
    #[doc(hidden)]
    pub fn probe_spawn_owned_blocking(&self, work: impl FnOnce() + Send + 'static) -> bool {
        let Some(executor) = &self.owned_executor else {
            return false;
        };
        executor.handle().spawn_blocking(work);
        true
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_hold_lifecycle(&self) {
        PROBE_LIFECYCLE_HELD.store(false, Ordering::Release);
        PROBE_HOLD_LIFECYCLE.store(true, Ordering::Release);
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_lifecycle_held(&self) -> bool {
        PROBE_LIFECYCLE_HELD.load(Ordering::Acquire)
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_owned_executor_stopping(&self) -> bool {
        self.owned_executor.as_ref().is_some_and(|executor| {
            executor.wait_if_stopping(std::time::Duration::ZERO)
                == Some(owned_executor::ExecutorStop::Pending)
        })
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_unfenced_executor_panic(&self) {
        if let Some(executor) = &self.owned_executor {
            executor.probe_unfenced_panic();
        }
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_owned_executor_admission(&self) -> bool {
        self.owned_executor
            .as_ref()
            .is_some_and(|executor| executor.spawn(async {}).is_ok())
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_unfenced_executor_panic_caught(&self) -> bool {
        self.owned_executor
            .as_ref()
            .is_some_and(|executor| executor.probe_unfenced_caught())
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_fail_owned_executor(&self) {
        if let Some(executor) = &self.owned_executor {
            executor.probe_panic();
        }
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_owned_executor_failed(&self) -> bool {
        self.owned_executor.as_ref().is_some_and(|executor| {
            executor.wait_if_stopping(std::time::Duration::ZERO)
                == Some(owned_executor::ExecutorStop::Failed)
        })
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn probe_owned_executor_stopped(&self) -> bool {
        self.owned_executor.as_ref().is_some_and(|executor| {
            executor.wait_if_stopping(std::time::Duration::ZERO)
                == Some(owned_executor::ExecutorStop::Stopped)
        })
    }

    pub async fn lifecycle_state(&self) -> EngineState {
        *self.state.lock().await
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if self.retain_on_drop {
            self.lifecycle_requests.abandon_owner();
            self.orphan_retention.retain(self);
        }
    }
}

fn invalid_state_error() -> EngineError {
    EngineError::new(INVALID_STATE_CODE, EngineErrorCategory::InvalidState, false)
}

fn operation_cancelled_error() -> EngineError {
    EngineError::new(
        OPERATION_CANCELLED_CODE,
        EngineErrorCategory::DeadlineExceeded,
        true,
    )
}

fn terminal_for_result<T>(result: &Result<T, EngineError>) -> OperationTerminal {
    match result {
        Ok(_) => OperationTerminal::Succeeded,
        Err(error) => OperationTerminal::Failed(error.clone()),
    }
}

#[cfg(test)]
mod tests {
    mod resume_failure;

    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::Notify;
    use tokio::time::Instant;
    use tokio_util::sync::CancellationToken;

    use crate::{
        EngineError, EngineErrorCategory, EngineEvent, EngineState, Operation, OperationResult,
        OperationTerminal, SendTextInput,
    };

    use super::{Engine, EngineRuntime};

    #[derive(Default)]
    pub(super) struct FakeRuntime {
        execute_calls: AtomicUsize,
        block_operations: AtomicBool,
        operation_started: Notify,
        suspend_calls: AtomicUsize,
        resume_calls: AtomicUsize,
        fail_suspend: AtomicBool,
        fail_resume: AtomicBool,
        pub(super) shutdown_calls: AtomicUsize,
        pub(super) shutdown_deadline: StdMutex<Option<Duration>>,
        pub(super) fail_shutdown: AtomicBool,
        pub(super) block_shutdown: AtomicBool,
        pub(super) panic_shutdown: AtomicBool,
        pub(super) shutdown_started: Notify,
        pub(super) shutdown_release: Notify,
    }

    #[async_trait]
    impl EngineRuntime for FakeRuntime {
        async fn execute(
            &self,
            _operation: Operation,
            cancellation: CancellationToken,
        ) -> Result<OperationResult, EngineError> {
            self.execute_calls.fetch_add(1, Ordering::SeqCst);
            if self.block_operations.load(Ordering::SeqCst) {
                self.operation_started.notify_one();
                cancellation.cancelled().await;
                return Err(EngineError::new(
                    1002,
                    EngineErrorCategory::DeadlineExceeded,
                    true,
                ));
            }
            Ok(OperationResult::Devices(Vec::new()))
        }

        async fn suspend(&self, _deadline: Option<Instant>) -> Result<(), EngineError> {
            self.suspend_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_suspend.load(Ordering::SeqCst) {
                return Err(EngineError::new(
                    9002,
                    EngineErrorCategory::Unavailable,
                    true,
                ));
            }
            Ok(())
        }

        async fn resume(&self, _cancellation: CancellationToken) -> Result<(), EngineError> {
            self.resume_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_resume.load(Ordering::SeqCst) {
                return Err(EngineError::new(
                    9003,
                    EngineErrorCategory::Unavailable,
                    true,
                ));
            }
            Ok(())
        }

        async fn shutdown(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
            self.shutdown_calls.fetch_add(1, Ordering::SeqCst);
            *self.shutdown_deadline.lock().unwrap() =
                deadline.map(|end| end.saturating_duration_since(Instant::now()));
            if self.block_shutdown.load(Ordering::SeqCst) {
                self.shutdown_started.notify_one();
                self.shutdown_release.notified().await;
            }
            if self.panic_shutdown.load(Ordering::SeqCst) {
                panic!("private-shutdown-panic");
            }
            if self.fail_shutdown.load(Ordering::SeqCst) {
                return Err(EngineError::new(9001, EngineErrorCategory::Internal, false));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn execute_is_rejected_after_quiesce_without_reaching_runtime() {
        let runtime = Arc::new(FakeRuntime::default());
        let (engine, _events) = Engine::from_runtime(runtime.clone(), 8);

        engine.quiesce(Duration::from_millis(50)).await.unwrap();
        let error = engine.execute(Operation::ListDevices).await.unwrap_err();

        assert_eq!(error.category(), EngineErrorCategory::InvalidState);
        assert_eq!(runtime.execute_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn quiesce_deadline_cancels_in_flight_operation_with_terminal_event() {
        let runtime = Arc::new(FakeRuntime {
            block_operations: AtomicBool::new(true),
            ..FakeRuntime::default()
        });
        let (engine, mut events) = Engine::from_runtime(runtime.clone(), 8);
        let engine = Arc::new(engine);
        let operation_started = runtime.operation_started.notified();
        let execute_engine = Arc::clone(&engine);
        let execute = tokio::spawn(async move {
            execute_engine
                .execute(Operation::SendText(SendTextInput {
                    text: "private text".into(),
                    target_devices: Vec::new(),
                }))
                .await
        });
        operation_started.await;

        engine.quiesce(Duration::from_millis(10)).await.unwrap();
        let error = tokio::time::timeout(Duration::from_millis(100), execute)
            .await
            .expect("cancelled operation did not finish")
            .expect("execute task panicked")
            .unwrap_err();

        assert_eq!(error.category(), EngineErrorCategory::DeadlineExceeded);
        assert_eq!(
            events.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Quiescing,
            })
        );
        assert!(matches!(
            events.next().await,
            Some(EngineEvent::OperationFinished {
                terminal: OperationTerminal::Cancelled,
                ..
            })
        ));
        assert_eq!(
            events.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Quiesced,
            })
        );
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_waiter_cancels_cooperative_work_before_shutdown() {
        let runtime = Arc::new(FakeRuntime {
            block_operations: AtomicBool::new(true),
            ..FakeRuntime::default()
        });
        let (engine, _events) = Engine::from_runtime(runtime.clone(), 8);
        let engine = Arc::new(engine);
        let operation_started = runtime.operation_started.notified();
        let execute_engine = Arc::clone(&engine);
        let execute = tokio::spawn(async move {
            execute_engine
                .execute(Operation::SendText(SendTextInput {
                    text: "private text".into(),
                    target_devices: Vec::new(),
                }))
                .await
        });
        operation_started.await;

        execute.abort();
        execute.await.expect_err("operation task must be cancelled");

        let shutdown_engine = Arc::clone(&engine);
        let shutdown =
            tokio::spawn(async move { shutdown_engine.shutdown(Duration::from_secs(60)).await });
        tokio::task::yield_now().await;
        if !shutdown.is_finished() {
            shutdown.abort();
            let _ = shutdown.await;
            panic!("an abandoned operation future kept its in-flight registration");
        }
        shutdown
            .await
            .expect("shutdown task must finish")
            .expect("an abandoned operation future must release its in-flight registration");
        assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn suspend_then_resume_keeps_stream_without_retrying_cancelled_work() {
        let runtime = Arc::new(FakeRuntime {
            block_operations: AtomicBool::new(true),
            ..FakeRuntime::default()
        });
        let (engine, mut events) = Engine::from_runtime(runtime.clone(), 8);
        let engine = Arc::new(engine);
        let operation_started = runtime.operation_started.notified();
        let execute_engine = Arc::clone(&engine);
        let execute = tokio::spawn(async move {
            execute_engine
                .execute(Operation::SendText(SendTextInput {
                    text: "private text".into(),
                    target_devices: Vec::new(),
                }))
                .await
        });
        operation_started.await;

        engine.suspend().await.unwrap();
        assert_eq!(engine.lifecycle_state().await, EngineState::Suspended);
        let cancelled = tokio::time::timeout(Duration::from_millis(100), execute)
            .await
            .expect("suspended operation did not finish")
            .unwrap()
            .unwrap_err();
        assert_eq!(cancelled.category(), EngineErrorCategory::DeadlineExceeded);
        assert_eq!(runtime.suspend_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.execute_calls.load(Ordering::SeqCst), 1);

        engine.resume().await.unwrap();
        assert_eq!(engine.lifecycle_state().await, EngineState::Running);
        assert_eq!(runtime.resume_calls.load(Ordering::SeqCst), 1);
        assert_eq!(runtime.execute_calls.load(Ordering::SeqCst), 1);

        let mut states = Vec::new();
        while states.len() < 5 {
            if let Some(EngineEvent::StateChanged { state }) = events.next().await {
                states.push(state);
            }
        }
        assert_eq!(
            states,
            vec![
                EngineState::Quiescing,
                EngineState::Quiesced,
                EngineState::Suspended,
                EngineState::Quiesced,
                EngineState::Running,
            ]
        );
    }

    #[tokio::test]
    async fn suspend_failure_is_reported_on_the_engine_event_stream() {
        let runtime = Arc::new(FakeRuntime {
            fail_suspend: AtomicBool::new(true),
            ..FakeRuntime::default()
        });
        let (engine, mut events) = Engine::from_runtime(runtime, 8);

        let error = engine.suspend().await.unwrap_err();

        while let Some(event) = events.next().await {
            if let EngineEvent::LifecycleFailed {
                action,
                error: observed,
            } = event
            {
                assert_eq!(action, crate::LifecycleAction::Suspend);
                assert_eq!(observed, error);
                return;
            }
        }
        panic!("suspend failure was not reported before the event stream closed");
    }

    #[tokio::test]
    async fn resume_failure_is_reported_on_the_engine_event_stream() {
        let runtime = Arc::new(FakeRuntime::default());
        let (engine, mut events) = Engine::from_runtime(runtime.clone(), 8);
        engine.suspend().await.unwrap();
        runtime.fail_resume.store(true, Ordering::SeqCst);

        let error = engine.resume().await.unwrap_err();

        while let Some(event) = events.next().await {
            if let EngineEvent::LifecycleFailed {
                action,
                error: observed,
            } = event
            {
                assert_eq!(action, crate::LifecycleAction::Resume);
                assert_eq!(observed, error);
                return;
            }
        }
        panic!("resume failure was not reported before the event stream closed");
    }

    #[tokio::test]
    async fn shutdown_drains_runtime_and_closes_event_stream() {
        let runtime = Arc::new(FakeRuntime::default());
        let (engine, mut events) = Engine::from_runtime(runtime.clone(), 8);

        engine.shutdown(Duration::from_millis(50)).await.unwrap();
        assert_eq!(engine.lifecycle_state().await, EngineState::Stopped);

        assert_eq!(runtime.shutdown_calls.load(Ordering::SeqCst), 1);
        let states = tokio::time::timeout(Duration::from_millis(100), async {
            let mut states = Vec::new();
            while let Some(event) = events.next().await {
                if let EngineEvent::StateChanged { state } = event {
                    states.push(state);
                }
            }
            states
        })
        .await
        .expect("event stream remained open after shutdown");
        assert_eq!(
            states,
            vec![EngineState::ShuttingDown, EngineState::Stopped,]
        );
        let error = engine.execute(Operation::ListDevices).await.unwrap_err();
        assert_eq!(error.category(), EngineErrorCategory::InvalidState);
    }

    #[tokio::test]
    async fn shutdown_reserves_completion_time_outside_the_runtime_budget() {
        let runtime = Arc::new(FakeRuntime::default());
        let (engine, _events) = Engine::from_runtime(runtime.clone(), 8);

        engine.shutdown(Duration::from_secs(1)).await.unwrap();

        let runtime_deadline = runtime.shutdown_deadline.lock().unwrap().unwrap();
        assert!(runtime_deadline <= Duration::from_millis(900));
    }
}
