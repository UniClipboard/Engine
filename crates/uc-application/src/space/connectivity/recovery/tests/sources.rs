use std::error::Error;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{NetworkRecoveryFacade, RebuildNetworkSessionError, RecordingRebuilder};

#[tokio::test(start_paused = true)]
async fn exhausted_retries_return_the_last_original_failure() {
    let failures = (0..6).map(|attempt| {
        Err(RebuildNetworkSessionError::new(
            io::Error::from_raw_os_error(100 + attempt),
            true,
        ))
    });
    let port = Arc::new(RecordingRebuilder::new(failures));
    let recovery = NetworkRecoveryFacade::new(port.clone());
    let failure = recovery.request_recovery().await.unwrap_err();
    let source = failure
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .unwrap();
    assert_eq!(source.raw_os_error(), Some(105));
    assert_eq!(port.calls.load(Ordering::SeqCst), 6);
    assert!(recovery.status().await.retryable);
    recovery.shutdown().await.unwrap();
}

#[tokio::test]
async fn permanent_failure_preserves_its_chain_and_does_not_schedule_retries() {
    let source = anyhow::Error::new(io::Error::other("sensitive source payload"))
        .context("rebuild action failed");
    let failure = RebuildNetworkSessionError::new(source, false);
    let port = Arc::new(RecordingRebuilder::new([Err(failure.clone())]));
    let recovery = NetworkRecoveryFacade::new(port.clone());
    let reported = recovery.request_recovery().await.unwrap_err();
    let source = reported.source().unwrap().source().unwrap();
    assert!(source.downcast_ref::<io::Error>().is_some());
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
    assert!(!recovery.status().await.retryable);
    for text in [
        format!("{failure:?}"),
        format!("{failure}"),
        format!("{reported:?}"),
        format!("{reported}"),
    ] {
        assert!(!text.contains("sensitive"));
    }
    recovery.shutdown().await.unwrap();
}
