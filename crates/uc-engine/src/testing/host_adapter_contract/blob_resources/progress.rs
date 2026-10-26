//! 原大文件交换后反复核对并关闭原受管连接，观察实际新拨号和资源平台。
use super::{GeneratedFiles, Resources};
use crate::testing::receive_exports::ReceiveExports;
use crate::{
    Engine, Operation, OperationResult, OutboundProgressBatchState, OutboundProgressInput,
    OutboundProgressReconciliation, OutboundProgressSummary, ReconcileOutboundProgressInput,
};
use anyhow::{anyhow, Result};
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::{sleep, timeout, Instant};
use uc_core::file_transfer::ProgressIdentity;
use uc_infra_p2p::network::iroh::test_support::progress_native_handle_counts;
use uuid::Uuid;

#[derive(Serialize)]
pub(super) struct ProgressResourceRound {
    queries: usize,
    reconciliations: usize,
    reconnects: usize,
    updates: usize,
    exact_ack: bool,
    samples: Vec<Resources>,
    peaks: Vec<Resources>,
    settlement_ms: Vec<u128>,
    native_handles: Vec<(usize, usize)>,
}
async fn completed(
    source: &Engine,
    input: &OutboundProgressInput,
) -> Result<OutboundProgressSummary> {
    timeout(Duration::from_secs(20), async {
        loop {
            if let OperationResult::OutboundProgress(Some(view)) = source
                .execute(Operation::QueryOutboundProgress(input.clone()))
                .await?
            {
                if view.state == OutboundProgressBatchState::Completed {
                    return Ok(view);
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(anyhow::Error::new)?
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn verify_round(
    source: &Engine,
    peer: &str,
    entry: &str,
    attempt: &str,
    network: &ReceiveExports,
    receiver: &ReceiveExports,
    files: &GeneratedFiles,
    file_bytes: u64,
) -> Result<ProgressResourceRound> {
    let input = OutboundProgressInput {
        entry_id: entry.into(),
        target_device_id: peer.into(),
    };
    let expected = completed(source, &input).await?;
    assert_eq!(expected.attempt_id.as_deref(), Some(attempt));
    receiver
        .matching_acks(
            ProgressIdentity::new(*Uuid::parse_str(entry)?.as_bytes()),
            ProgressIdentity::new(*Uuid::parse_str(attempt)?.as_bytes()),
            1,
        )
        .await;
    let reads = files.reads.load(Ordering::SeqCst);
    let misses = receiver.cache_misses.lock().unwrap().len();
    let exports = receiver.exports.lock().unwrap().len();
    let mut result = ProgressResourceRound {
        queries: 0,
        reconciliations: 0,
        reconnects: 0,
        updates: receiver.progress_updates.load(Ordering::SeqCst),
        exact_ack: true,
        samples: Vec::new(),
        peaks: Vec::new(),
        settlement_ms: Vec::new(),
        native_handles: Vec::new(),
    };
    if file_bytes >= 1024 * 1024 * 1024 {
        assert!(
            result.updates > 0,
            "large files must actually send native live progress"
        );
    }
    for burst in 0..3 {
        for _ in 0..4 {
            let reconciled = source
                .execute(Operation::ReconcileOutboundProgress(
                    ReconcileOutboundProgressInput {
                        entry_id: entry.into(),
                        target_device_id: peer.into(),
                        expected_attempt_id: Some(attempt.into()),
                    },
                ))
                .await?;
            if !matches!(
                reconciled,
                OperationResult::OutboundProgressReconciliation(
                    OutboundProgressReconciliation::Settled
                )
            ) {
                return Err(anyhow!(
                    "original completed progress did not reconcile as settled"
                ));
            }
            result.reconciliations += 1;
            let current = completed(source, &input).await?;
            result.queries += 1;
            assert_eq!(current.attempt_id, expected.attempt_id);
            assert_eq!(current.items, expected.items);
        }
        // 完整回复结束后仍可能有旧连接关闭交接；沿用原20秒收尾窗口，资源上限不变。
        let started = Instant::now();
        let mut peak = Resources::read()?;
        let mut previous = None;
        let stable = timeout(Duration::from_secs(20), async {
            loop {
                let sample = Resources::read()?;
                peak.include(sample);
                let native = progress_native_handle_counts();
                let first: Option<&Resources> = result.samples.first();
                let within = first.is_none_or(|first| {
                    sample.runtime_tasks <= first.runtime_tasks + 4
                        && sample.fds <= first.fds + 4
                        && sample.threads <= first.threads + 2
                        && sample.rss_kib <= first.rss_kib + 64 * 1024
                }) && result
                    .native_handles
                    .first()
                    .is_none_or(|first: &(usize, usize)| native.1 <= first.1);
                let counts = (sample.runtime_tasks, sample.fds, sample.threads, native.1);
                if within && previous == Some(counts) {
                    return Ok::<_, anyhow::Error>((sample, native));
                }
                previous = Some(counts);
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        let (sample, native) = stable.unwrap_or_else(|_| panic!("progress resources failed to return to original budget; first_tasks={:?}, first_fds={:?}, peak_tasks={}, peak_fds={}, native={:?}", result.samples.first().map(|x| x.runtime_tasks), result.samples.first().map(|x| x.fds), peak.runtime_tasks, peak.fds, progress_native_handle_counts()))?;
        result.samples.push(sample);
        result.peaks.push(peak);
        result.settlement_ms.push(started.elapsed().as_millis());
        result.native_handles.push(native);
        if burst < 2 {
            let before = network.progress_connections.lock().unwrap().len();
            let connections = network
                .progress_connections
                .lock()
                .unwrap()
                .iter()
                .filter_map(|connection| connection.upgrade())
                .collect::<Vec<_>>();
            assert!(
                !connections.is_empty(),
                "complete queries must use the original managed backend"
            );
            for connection in connections {
                connection.close();
            }
            let reconnected = source
                .execute(Operation::ReconcileOutboundProgress(
                    ReconcileOutboundProgressInput {
                        entry_id: entry.into(),
                        target_device_id: peer.into(),
                        expected_attempt_id: Some(attempt.into()),
                    },
                ))
                .await?;
            assert!(matches!(
                reconnected,
                OperationResult::OutboundProgressReconciliation(
                    OutboundProgressReconciliation::Settled
                )
            ));
            result.reconciliations += 1;
            assert_eq!(
                network.progress_connections.lock().unwrap().len(),
                before + 1,
                "ordinary close must cause one actual new managed dial"
            );
            result.reconnects += 1;
        }
    }
    assert_eq!(files.reads.load(Ordering::SeqCst), reads);
    assert_eq!(receiver.cache_misses.lock().unwrap().len(), misses);
    assert_eq!(receiver.exports.lock().unwrap().len(), exports);
    Ok(result)
}
