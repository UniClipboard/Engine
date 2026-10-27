//! 显式独占进程，将真实文件业务导出到严格Collector；不替换网络、SQLite或业务结果。
#[path = "observation/progress.rs"]
mod progress;
use super::*;
use crate::{flush_test_tracing, init_test_tracing_with_otlp};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要本机严格Collector与Jaeger，独占进程观测，精确运行"]
async fn real_engine_file_partial_retry_and_cancellation_export_actual_business_results() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let trace_endpoint =
        std::env::var("UC_TEST_OTLP_TRACE_ENDPOINT").expect("显式设置本机测试trace接收端");
    let log_endpoint =
        std::env::var("UC_TEST_OTLP_LOG_ENDPOINT").expect("显式设置本机测试log接收端");
    assert!(init_test_tracing_with_otlp(&trace_endpoint, &log_endpoint));

    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("原部分失败必须接受单项重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt_id);
    assert!(completed.items.iter().all(|item| item.state == "completed"));
    let saved = saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    assert_eq!(saved.len(), 3);
    for (bytes, path) in &case.saved {
        assert!(saved.get(bytes) == Some(path), "原成功文件与路径必须保留");
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    case.shutdown().await;

    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("第二次原部分失败必须接受单项重试")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(output.is_file());
    let cancellation = CancelEntryReceiveInput {
        entry_id: case.first.entry_id.clone(),
        attempt_id: attempt_id.clone(),
    };
    assert!(timeout(
        Duration::from_millis(50),
        case.receiver
            .execute(Operation::CancelEntryReceive(cancellation.clone()))
    )
    .await
    .is_err());
    let cancelling = progress(&case.receiver, &case.first.entry_id, "cancelling").await;
    assert_eq!(cancelling.attempt_id, attempt_id);
    assert_eq!(
        case.receiver
            .execute(Operation::CancelEntryReceive(cancellation))
            .await
            .unwrap(),
        OperationResult::EntryReceiveCancellation(
            EntryReceiveCancellationOutcome::CancellationIncomplete
        )
    );
    case.exports.release.add_permits(1);
    let cancelled = progress(&case.receiver, &case.first.entry_id, "cancelled").await;
    assert_eq!(cancelled.attempt_id, attempt_id);
    assert_eq!(cancelled.items[0].state, "completed");
    assert_eq!(cancelled.items[1].state, "cancelled");
    assert_eq!(cancelled.items[2].state, "completed");
    assert!(!output.exists());
    assert!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await
            == case.saved,
        "取消仍须保留原成功文件与路径"
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    case.shutdown().await;
    flush_test_tracing();
}
