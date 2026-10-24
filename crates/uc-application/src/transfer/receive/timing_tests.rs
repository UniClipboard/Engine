use super::{ReceiveTiming, TimingError, TimingRefusal};
use std::error::Error;
use std::time::Duration;
use tokio::time::Instant;

fn refusal<T>(result: Result<T, TimingError>, expected: TimingRefusal) {
    let Err(error) = result else {
        panic!("unexpected receive budget acceptance");
    };
    assert_eq!(error.0, expected);
    assert!(error.source().is_some());
}

#[tokio::test(start_paused = true)]
async fn first_data_covers_preparation_and_exact_expiration_also_rejects_empty_completion() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    assert!(timing.check(start + Duration::from_millis(59_999)).is_ok());
    assert!(!timing
        .validated_through(0, start)
        .expect("empty is not new data"));
    refusal(
        timing.check(start + Duration::from_secs(60)),
        TimingRefusal::FirstDataExpired,
    );
    refusal(
        timing.validated_through(1, start + Duration::from_secs(60)),
        TimingRefusal::FirstDataExpired,
    );
}

#[tokio::test(start_paused = true)]
async fn repeated_data_does_not_extend_stall_deadline_or_verified_progress() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    assert!(timing.validated_through(100, start).expect("new bytes"));
    assert!(!timing
        .validated_through(100, start + Duration::from_secs(299))
        .expect("duplicate"));
    assert!(!timing
        .validated_through(50, start + Duration::from_secs(299))
        .expect("replayed prefix"));
    assert_eq!(timing.verified_end, 100);
    refusal(
        timing.check(start + Duration::from_secs(300)),
        TimingRefusal::DataStalled,
    );
}

#[tokio::test(start_paused = true)]
async fn continuous_new_data_has_no_fixed_total_download_lifetime() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    for index in 0..100u64 {
        let now = start + Duration::from_secs(index * 299);
        assert!(timing
            .validated_through(index + 1, now)
            .expect("validated progress"));
    }
    assert!(timing.check(start + Duration::from_secs(100 * 299)).is_ok());
}

#[tokio::test(start_paused = true)]
async fn recovery_uses_the_earliest_deadline_instead_of_a_fresh_first_data_budget() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    let retry_at = timing
        .schedule_recovery(start + Duration::from_millis(59_900))
        .expect("recoverable source");
    assert_eq!(retry_at, start + Duration::from_millis(60_100));
    assert_eq!(timing.deadline().at, start + Duration::from_secs(60));
    refusal(
        timing.start_retry(start + Duration::from_secs(60)),
        TimingRefusal::FirstDataExpired,
    );
    assert_eq!(timing.attempts_started, 1);
}

#[tokio::test(start_paused = true)]
async fn recovery_accumulates_across_valid_data_and_new_network_rounds() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    timing.validated_through(100, start).expect("initial data");
    timing
        .schedule_recovery(start + Duration::from_secs(5))
        .expect("first failure");
    timing
        .start_retry(start + Duration::from_secs(6))
        .expect("retry 2");
    timing
        .validated_through(200, start + Duration::from_secs(20))
        .expect("new data");
    assert_eq!(timing.recovery_used, Duration::from_secs(15));
    timing
        .schedule_recovery(start + Duration::from_secs(30))
        .expect("second failure");
    timing
        .start_retry(start + Duration::from_secs(31))
        .expect("retry 3");
    assert_eq!(timing.deadline().at, start + Duration::from_secs(45));
    assert!(!timing
        .validated_through(200, start + Duration::from_secs(44))
        .expect("duplicate"));
    timing
        .validated_through(300, start + Duration::from_millis(44_999))
        .expect("new unique byte before deadline");
    assert_eq!(timing.recovery_used, Duration::from_millis(29_999));
    timing
        .schedule_recovery(start + Duration::from_secs(100))
        .expect("third failure");
    assert_eq!(timing.deadline().at, start + Duration::from_millis(100_001));
    refusal(
        timing.start_retry(start + Duration::from_millis(100_001)),
        TimingRefusal::RecoveryExpired,
    );
    assert_eq!(timing.attempts_started, 3);
}

#[tokio::test(start_paused = true)]
async fn retry_backoffs_and_attempt_count_never_reset_on_repeated_failure_notifications() {
    let start = Instant::now();
    let mut timing = ReceiveTiming::new(start);
    refusal(timing.start_retry(start), TimingRefusal::RetryNotScheduled);
    let mut failed_at = start;
    for (attempt, delay) in [(2, 200), (3, 800), (4, 2000)] {
        let retry_at = timing
            .schedule_recovery(failed_at)
            .expect("retry scheduled");
        assert_eq!(retry_at, failed_at + Duration::from_millis(delay));
        assert_eq!(
            timing
                .schedule_recovery(failed_at + Duration::from_millis(1))
                .expect("same retry"),
            retry_at
        );
        refusal(
            timing.start_retry(retry_at - Duration::from_nanos(1)),
            TimingRefusal::RetryNotReady,
        );
        timing.start_retry(retry_at).expect("retry now");
        assert_eq!(timing.attempts_started, attempt);
        failed_at = retry_at;
    }
    refusal(
        timing.schedule_recovery(failed_at),
        TimingRefusal::AttemptsExhausted,
    );
}
