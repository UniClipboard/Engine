use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};

use super::error::{LifecycleDeadlineElapsed, LifecycleTaskFailure};
use super::{
    LifecycleTarget, RuntimeLifecycle, RuntimeLifecycleParticipants, RuntimeLifecyclePort,
    TransitionContext,
};

mod panics;
mod superseded;

type Calls = Arc<Mutex<Vec<(&'static str, u64, Option<Instant>)>>>;

struct Participant {
    name: &'static str,
    calls: Calls,
    fail_suspend: AtomicBool,
    fail_resume: AtomicBool,
    panic_suspend: AtomicBool,
    panic_resume: AtomicBool,
    block_suspend: AtomicBool,
    block_resume: AtomicBool,
    starting: Notify,
    resume_release: Notify,
    stopping: Notify,
    release: Notify,
    suspend_completed: AtomicBool,
    resume_completed: AtomicBool,
}

impl Participant {
    fn new(name: &'static str, calls: &Calls) -> Arc<Self> {
        Arc::new(Self {
            name,
            calls: Arc::clone(calls),
            fail_suspend: AtomicBool::new(false),
            fail_resume: AtomicBool::new(false),
            panic_suspend: AtomicBool::new(false),
            panic_resume: AtomicBool::new(false),
            block_suspend: AtomicBool::new(false),
            block_resume: AtomicBool::new(false),
            starting: Notify::new(),
            resume_release: Notify::new(),
            stopping: Notify::new(),
            release: Notify::new(),
            suspend_completed: AtomicBool::new(false),
            resume_completed: AtomicBool::new(false),
        })
    }
}

#[async_trait]
impl RuntimeLifecyclePort for Participant {
    async fn suspend(&self, context: &TransitionContext) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name, context.generation(), context.deadline()));
        if self.block_suspend.load(Ordering::SeqCst) {
            self.stopping.notify_one();
            self.release.notified().await;
        }
        self.suspend_completed.store(true, Ordering::SeqCst);
        if self.fail_suspend.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("stop failure").into());
        }
        assert!(
            !self.panic_suspend.load(Ordering::SeqCst),
            "sensitive stop panic payload"
        );
        Ok(())
    }

    async fn resume(&self, context: &TransitionContext) -> anyhow::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((self.name, context.generation(), context.deadline()));
        if self.block_resume.load(Ordering::SeqCst) {
            self.starting.notify_one();
            self.resume_release.notified().await;
        }
        self.resume_completed.store(true, Ordering::SeqCst);
        if self.fail_resume.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("start failure").into());
        }
        assert!(
            !self.panic_resume.load(Ordering::SeqCst),
            "sensitive resume panic payload"
        );
        Ok(())
    }
}

struct Fixture {
    coordinator: Arc<RuntimeLifecycle>,
    session: Arc<Participant>,
    local: Arc<Participant>,
    resources: Arc<Participant>,
    calls: Calls,
}

impl Fixture {
    fn new() -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let session = Participant::new("session", &calls);
        let local = Participant::new("local", &calls);
        let resources = Participant::new("resources", &calls);
        Self {
            coordinator: Arc::new(RuntimeLifecycle::new(RuntimeLifecycleParticipants {
                session_work: session.clone(),
                local_work: local.clone(),
                local_resources: resources.clone(),
            })),
            session,
            local,
            resources,
            calls,
        }
    }

    async fn activate(&self) {
        self.coordinator
            .transition(LifecycleTarget::Active, None)
            .await
            .unwrap();
        self.calls.lock().unwrap().clear();
    }

    fn names(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.0)
            .collect()
    }
}

#[tokio::test]
async fn dependency_order_and_context_are_shared_across_the_transition() {
    let fixture = Fixture::new();
    let deadline = Some(Instant::now() + Duration::from_secs(1));
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, deadline)
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["resources", "local", "session"]);
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| call.1 == 1 && call.2 == deadline));
    fixture.calls.lock().unwrap().clear();
    fixture
        .coordinator
        .transition(LifecycleTarget::Suspended, deadline)
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
    assert!(fixture
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|call| call.1 == 2 && call.2 == deadline));
}

#[tokio::test]
async fn slow_session_does_not_delay_local_stop_notification() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.block_suspend.store(true, Ordering::SeqCst);
    fixture.local.block_suspend.store(true, Ordering::SeqCst);
    let stopping = tokio::spawn({
        let owner = Arc::clone(&fixture.coordinator);
        async move { owner.transition(LifecycleTarget::Suspended, None).await }
    });
    timeout(Duration::from_secs(1), fixture.session.stopping.notified())
        .await
        .unwrap();
    timeout(Duration::from_secs(1), fixture.local.stopping.notified())
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["session", "local"]);
    fixture.session.release.notify_one();
    tokio::task::yield_now().await;
    assert_eq!(fixture.names(), ["session", "local"]);
    fixture.local.release.notify_one();
    timeout(Duration::from_secs(1), stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
}

#[tokio::test]
async fn suspend_deadline_cancels_the_participant_task_before_returning() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.block_suspend.store(true, Ordering::SeqCst);
    let error = timeout(
        Duration::from_secs(1),
        fixture
            .coordinator
            .suspend(Some(Instant::now() + Duration::from_millis(20))),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error
        .primary
        .downcast_ref::<LifecycleDeadlineElapsed>()
        .is_some());
    assert!(!fixture.session.suspend_completed.load(Ordering::SeqCst));
    fixture.session.release.notify_one();
    tokio::task::yield_now().await;
    assert!(!fixture.session.suspend_completed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn resume_deadline_cancels_the_participant_task_before_returning() {
    let fixture = Fixture::new();
    fixture.resources.block_resume.store(true, Ordering::SeqCst);
    let error = timeout(
        Duration::from_secs(1),
        fixture.coordinator.resume(
            Some(Instant::now() + Duration::from_millis(20)),
            tokio_util::sync::CancellationToken::new(),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(error
        .primary
        .downcast_ref::<LifecycleDeadlineElapsed>()
        .is_some());
    assert!(!fixture.resources.resume_completed.load(Ordering::SeqCst));
    fixture.resources.resume_release.notify_one();
    tokio::task::yield_now().await;
    assert!(!fixture.resources.resume_completed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn stop_failure_attempts_other_work_but_retains_resources_and_all_sources() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.fail_suspend.store(true, Ordering::SeqCst);
    fixture.local.fail_suspend.store(true, Ordering::SeqCst);
    let error = fixture
        .coordinator
        .transition(LifecycleTarget::Suspended, None)
        .await
        .unwrap_err();
    assert_eq!(fixture.names(), ["session", "local"]);
    assert!(error.primary.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(error.additional.len(), 1);
    assert!(error.additional[0]
        .downcast_ref::<std::io::Error>()
        .is_some());
    fixture.session.fail_suspend.store(false, Ordering::SeqCst);
    fixture.local.fail_suspend.store(false, Ordering::SeqCst);
    fixture.calls.lock().unwrap().clear();
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap();
    assert_eq!(
        fixture.names(),
        [
            "session",
            "local",
            "resources",
            "resources",
            "local",
            "session"
        ]
    );
}

#[tokio::test]
async fn failed_resume_drains_the_failed_participant_before_releasing_resources() {
    let fixture = Fixture::new();
    fixture.local.fail_resume.store(true, Ordering::SeqCst);
    assert!(fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .is_err());
    assert_eq!(
        fixture.names(),
        ["resources", "local", "session", "local", "resources"]
    );
    fixture.local.fail_resume.store(false, Ordering::SeqCst);
    fixture.calls.lock().unwrap().clear();
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["resources", "local", "session"]);
}

#[tokio::test]
async fn dropping_the_waiter_does_not_drop_cleanup_or_allow_overlapping_resume() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.block_suspend.store(true, Ordering::SeqCst);
    let caller = tokio::spawn({
        let owner = Arc::clone(&fixture.coordinator);
        async move { owner.transition(LifecycleTarget::Suspended, None).await }
    });
    timeout(Duration::from_secs(1), fixture.session.stopping.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut resume = tokio::spawn({
        let owner = Arc::clone(&fixture.coordinator);
        async move { owner.transition(LifecycleTarget::Active, None).await }
    });
    assert!(timeout(Duration::from_millis(20), &mut resume)
        .await
        .is_err());
    assert_eq!(fixture.names(), ["session", "local"]);
    fixture.session.release.notify_one();
    timeout(Duration::from_secs(1), resume)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture.names(),
        [
            "session",
            "local",
            "resources",
            "resources",
            "local",
            "session"
        ]
    );
}

#[tokio::test]
async fn repeated_targets_do_not_start_or_stop_participants_twice() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap();
    assert!(fixture.names().is_empty());
    fixture
        .coordinator
        .transition(LifecycleTarget::Suspended, None)
        .await
        .unwrap();
    fixture.calls.lock().unwrap().clear();
    fixture
        .coordinator
        .transition(LifecycleTarget::Suspended, None)
        .await
        .unwrap();
    assert!(fixture.names().is_empty());
}

#[tokio::test]
async fn failed_resume_and_cleanup_keep_both_causes_and_retry_cleanup_first() {
    use std::error::Error;

    let fixture = Fixture::new();
    fixture.local.fail_resume.store(true, Ordering::SeqCst);
    fixture.local.fail_suspend.store(true, Ordering::SeqCst);
    let error = fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap_err();
    assert!(error.source().is_some());
    assert!(error.primary.downcast_ref::<std::io::Error>().is_some());
    assert_eq!(error.additional.len(), 1);
    assert!(error.additional[0]
        .downcast_ref::<std::io::Error>()
        .is_some());
    assert_eq!(fixture.names(), ["resources", "local", "session", "local"]);

    fixture.local.fail_resume.store(false, Ordering::SeqCst);
    fixture.local.fail_suspend.store(false, Ordering::SeqCst);
    fixture.calls.lock().unwrap().clear();
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap();
    assert_eq!(
        fixture.names(),
        [
            "session",
            "local",
            "resources",
            "resources",
            "local",
            "session"
        ]
    );
}

#[tokio::test]
async fn failed_stop_can_retry_but_never_accepts_resume() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.fail_suspend.store(true, Ordering::SeqCst);
    assert!(fixture.coordinator.stop(None).await.is_err());
    let calls = fixture.names();
    assert!(fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap_err()
        .is_stopped());
    assert_eq!(fixture.names(), calls);
    fixture.session.fail_suspend.store(false, Ordering::SeqCst);
    fixture.calls.lock().unwrap().clear();
    fixture.coordinator.stop(None).await.unwrap();
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
    fixture.calls.lock().unwrap().clear();
    fixture.coordinator.stop(None).await.unwrap();
    assert!(fixture.names().is_empty());
}

#[tokio::test]
async fn stop_during_resume_cleans_partial_start_without_starting_the_next_participant() {
    let fixture = Fixture::new();
    fixture.local.block_resume.store(true, Ordering::SeqCst);
    let resume = tokio::spawn({
        let owner = Arc::clone(&fixture.coordinator);
        async move { owner.transition(LifecycleTarget::Active, None).await }
    });
    timeout(Duration::from_secs(1), fixture.local.starting.notified())
        .await
        .unwrap();
    let mut stop = Box::pin(fixture.coordinator.stop(None));
    assert!(timeout(Duration::from_millis(20), stop.as_mut())
        .await
        .is_err());
    fixture.local.resume_release.notify_one();
    assert!(resume.await.unwrap().unwrap_err().is_stopped());
    stop.await.unwrap();
    assert_eq!(
        fixture.names(),
        ["resources", "local", "session", "local", "resources"]
    );
    assert!(fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap_err()
        .is_stopped());
}

#[tokio::test]
async fn abandoned_stop_waiter_keeps_cleanup_and_permanent_stop_intent() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.block_suspend.store(true, Ordering::SeqCst);
    let caller = tokio::spawn({
        let owner = Arc::clone(&fixture.coordinator);
        async move { owner.stop(None).await }
    });
    timeout(Duration::from_secs(1), fixture.session.stopping.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    fixture.session.release.notify_one();
    assert!(timeout(
        Duration::from_secs(1),
        fixture
            .coordinator
            .transition(LifecycleTarget::Active, None)
    )
    .await
    .unwrap()
    .unwrap_err()
    .is_stopped());
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
}
