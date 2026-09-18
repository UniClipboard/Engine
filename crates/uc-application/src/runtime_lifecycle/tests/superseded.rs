use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::{Fixture, LifecycleTarget};

#[tokio::test]
async fn superseded_resume_finishes_the_current_capability_then_cleans_without_starting_the_next() {
    for index in 0..3 {
        let fixture = Fixture::new();
        let participant = [&fixture.resources, &fixture.local, &fixture.session][index];
        participant.block_resume.store(true, Ordering::SeqCst);
        let cancellation = CancellationToken::new();
        let deadline = Some(Instant::now() + Duration::from_secs(1));
        let mut caller = tokio::spawn({
            let coordinator = fixture.coordinator.clone();
            let cancellation = cancellation.clone();
            async move {
                coordinator
                    .transition_with_cancellation(LifecycleTarget::Active, deadline, cancellation)
                    .await
            }
        });
        participant.starting.notified().await;
        cancellation.cancel();
        assert!(timeout(Duration::from_millis(20), &mut caller)
            .await
            .is_err());
        participant.resume_release.notify_one();
        let error = caller.await.unwrap().unwrap_err();
        assert!(error.is_superseded());
        assert!(error.additional.is_empty());
        let mut expected = ["resources", "local", "session"][..=index].to_vec();
        expected.extend(["session", "local", "resources"]);
        assert_eq!(fixture.names(), expected);
        assert!(fixture
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call.1 == 1 && call.2 == deadline));

        participant.block_resume.store(false, Ordering::SeqCst);
        fixture.calls.lock().unwrap().clear();
        fixture
            .coordinator
            .transition(LifecycleTarget::Active, None)
            .await
            .unwrap();
        assert_eq!(fixture.names(), ["resources", "local", "session"]);
    }
}

#[tokio::test]
async fn a_request_superseded_before_start_does_not_open_resources_or_stop_future_resumes() {
    let fixture = Fixture::new();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(fixture
        .coordinator
        .transition_with_cancellation(LifecycleTarget::Active, None, cancellation)
        .await
        .unwrap_err()
        .is_superseded());
    assert!(fixture.names().is_empty());
    fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["resources", "local", "session"]);
}

#[tokio::test]
async fn superseded_resume_retains_cleanup_failure_and_retries_it_before_starting_again() {
    let fixture = Fixture::new();
    fixture.local.block_resume.store(true, Ordering::SeqCst);
    fixture.local.fail_suspend.store(true, Ordering::SeqCst);
    let cancellation = CancellationToken::new();
    let caller = tokio::spawn({
        let coordinator = fixture.coordinator.clone();
        let cancellation = cancellation.clone();
        async move {
            coordinator
                .transition_with_cancellation(LifecycleTarget::Active, None, cancellation)
                .await
        }
    });
    fixture.local.starting.notified().await;
    cancellation.cancel();
    fixture.local.resume_release.notify_one();
    let error = caller.await.unwrap().unwrap_err();
    assert!(error.is_superseded());
    assert_eq!(error.additional.len(), 1);
    assert!(error.additional[0]
        .downcast_ref::<std::io::Error>()
        .is_some());
    assert_eq!(fixture.names(), ["resources", "local", "session", "local"]);
    fixture.local.fail_suspend.store(false, Ordering::SeqCst);
    fixture.local.block_resume.store(false, Ordering::SeqCst);
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
async fn request_cancellation_never_discards_an_accepted_pause() {
    let fixture = Fixture::new();
    fixture.activate().await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    fixture
        .coordinator
        .transition_with_cancellation(LifecycleTarget::Suspended, None, cancellation)
        .await
        .unwrap();
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
}
