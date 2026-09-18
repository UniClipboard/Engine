use std::sync::atomic::Ordering;

use super::{Fixture, LifecycleTarget, LifecycleTaskFailure};

#[tokio::test]
async fn panicked_stop_drains_other_work_and_retains_both_failure_sources() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture.session.panic_suspend.store(true, Ordering::SeqCst);
    fixture.local.panic_suspend.store(true, Ordering::SeqCst);
    let error = fixture.coordinator.stop(None).await.unwrap_err();
    assert_eq!(fixture.names(), ["session", "local"]);
    assert_eq!(
        error.primary.downcast_ref::<LifecycleTaskFailure>(),
        Some(&LifecycleTaskFailure::Panicked)
    );
    assert_eq!(error.additional.len(), 1);
    assert_eq!(
        error.additional[0].downcast_ref::<LifecycleTaskFailure>(),
        Some(&LifecycleTaskFailure::Panicked)
    );
    assert!(!format!("{:#}", error.primary).contains("sensitive"));
    assert!(!format!("{:#}", error.additional[0]).contains("sensitive"));
    assert!(!format!("{error:?}").contains("sensitive"));
    assert!(!format!("{error}").contains("sensitive"));
    assert!(fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .unwrap_err()
        .is_stopped());

    fixture.session.panic_suspend.store(false, Ordering::SeqCst);
    fixture.local.panic_suspend.store(false, Ordering::SeqCst);
    fixture.calls.lock().unwrap().clear();
    fixture.coordinator.stop(None).await.unwrap();
    assert_eq!(fixture.names(), ["session", "local", "resources"]);
}

#[tokio::test]
async fn panicked_resume_cleans_partial_work_before_a_new_attempt() {
    for index in 0..3 {
        let fixture = Fixture::new();
        let participant = [&fixture.resources, &fixture.local, &fixture.session][index];
        participant.panic_resume.store(true, Ordering::SeqCst);
        let error = fixture
            .coordinator
            .transition(LifecycleTarget::Active, None)
            .await
            .unwrap_err();
        assert_eq!(
            error.primary.downcast_ref::<LifecycleTaskFailure>(),
            Some(&LifecycleTaskFailure::Panicked)
        );
        assert!(error.additional.is_empty());
        let mut expected = ["resources", "local", "session"][..=index].to_vec();
        expected.extend(["session", "local", "resources"]);
        assert_eq!(fixture.names(), expected);

        participant.panic_resume.store(false, Ordering::SeqCst);
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
async fn panicked_resource_release_must_finish_before_resources_are_reopened() {
    let fixture = Fixture::new();
    fixture.activate().await;
    fixture
        .resources
        .panic_suspend
        .store(true, Ordering::SeqCst);
    let error = fixture
        .coordinator
        .transition(LifecycleTarget::Suspended, None)
        .await
        .unwrap_err();
    assert_eq!(
        error.primary.downcast_ref::<LifecycleTaskFailure>(),
        Some(&LifecycleTaskFailure::Panicked)
    );
    fixture.calls.lock().unwrap().clear();
    assert!(fixture
        .coordinator
        .transition(LifecycleTarget::Active, None)
        .await
        .is_err());
    assert_eq!(fixture.names(), ["session", "local", "resources"]);

    fixture
        .resources
        .panic_suspend
        .store(false, Ordering::SeqCst);
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
