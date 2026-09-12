use std::sync::Arc;
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionPurpose,
    DialFailure,
};

use iroh::endpoint::ConnectOptions;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use tokio::task::JoinSet;
use tracing::instrument::WithSubscriber;
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;

/// Per-attempt connect timeout.
///
/// 3s sits comfortably above the observed LAN/direct `iroh connect`
/// success latency (~1s for an Online peer's first attempt) and the
/// relay-fallback case (~1-2s when pkarr discovery completes before
/// the direct path), while keeping the worst-case staggered-retry
/// budget below [`crate::network::iroh::FAN_OUT_DEADLINE_HINT`]'s 5s
/// dispatch-side hard cap.
///
/// Pre-#886 phase 4 this was 10s, picked when staggered retry was the
/// only thing guarding the dispatch path. Now that the dispatch
/// adapter has single-flight (one staggered-retry batch per peer per
/// concurrent storm) and presence has a 30s sticky window after
/// `mark_offline`, repeated copies against a dead peer no longer
/// accumulate 15s tails — the leader's first batch alone defines the
/// per-storm dial cost, so trimming the constant is no longer
/// trading off ergonomics against repeated-storm cost.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(3);

/// Stagger offsets for the three concurrent attempts inside one
/// `connect_with_staggered_retry` call. 500ms + 1500ms after the
/// initial attempt gives slow paths (pkarr discovery, relay
/// handshake) a chance to overtake a dead direct-path race, without
/// dragging the worst-case lifetime out past the dispatch deadline.
///
/// Worst-case lifetime = `STAGGERED_DELAYS[2]` (1.5s) +
/// `ATTEMPT_TIMEOUT` (3s) = 4.5s. Storm metric per #886: a 5-copy
/// burst against an offline peer at 1s intervals lands at ~4s spawn
/// + 4.5s leader = 8.5s aggregate wall (down from the 19s phase-0
/// baseline), with `iroh connect` attempts capped at 3 and
/// `mark_offline` at 1.
const STAGGERED_DELAYS: [Duration; 3] = [
    Duration::from_millis(0),
    Duration::from_millis(500),
    Duration::from_millis(1500),
];

/// LAN-only Mode 反向防御：本端 `RelayMode::Disabled` **不阻止** iroh 用对端
/// `EndpointAddr` 中的 `relay_url` 发起出站连接。已配对 peer 的 addr blob 在
/// `to_persistable_addr` 阶段被收敛为"NodeId + Relay 提示"，所以 LAN-only
/// 用户复制内容触发 dispatch 时仍会看到日志：
///   `iroh::endpoint: connecting relay_url=Some(...) ip_addresses=[]`
///
/// 当 [`super::runtime_consts::lan_only`] 为 `true` 时剥掉 `TransportAddr::Relay`
/// 项，强制 iroh 只能走 mDNS 重新解析的直连地址。如果对端不在同一子网
/// （mDNS 不可达），connect 自然失败 —— 这正是 LAN-only 的设计意图。
///
/// 非 LAN-only 路径下零开销直接返回原 addr（一次 `Vec::iter().any` 短路）。
pub(super) fn strip_relay_if_lan_only(addr: EndpointAddr) -> EndpointAddr {
    if !super::runtime_consts::lan_only() {
        return addr;
    }
    let EndpointAddr { id, addrs } = addr;
    let kept = addrs
        .into_iter()
        .filter(|a| !matches!(a, TransportAddr::Relay(_)));
    EndpointAddr::from_parts(id, kept)
}

pub(crate) async fn connect_with_staggered_retry(
    endpoint: Arc<Endpoint>,
    addr: EndpointAddr,
    alpn: &'static [u8],
    purpose: &'static str,
    source: AddressInputSource,
) -> Result<Connection, String> {
    connect_with_staggered_retry_classified(endpoint, addr, alpn, Vec::new(), purpose, source)
        .await
        .map_err(|(message, _)| message)
}

pub(super) async fn connect_with_staggered_retry_classified(
    endpoint: Arc<Endpoint>,
    addr: EndpointAddr,
    alpn: &'static [u8],
    additional_alpns: Vec<Vec<u8>>,
    purpose: &'static str,
    source: AddressInputSource,
) -> Result<Connection, (String, DialFailure)> {
    let addr = strip_relay_if_lan_only(addr);
    let observation =
        ConnectionObservation::begin(connection_purpose(purpose), *addr.id.as_bytes());
    let (summary, fingerprint) = super::connection_diagnostics::candidate_summary(&addr);
    observation.input_candidates(fingerprint, source, summary);
    let mut attempts = JoinSet::new();

    for (idx, delay) in STAGGERED_DELAYS.iter().copied().enumerate() {
        let endpoint = Arc::clone(&endpoint);
        let addr = addr.clone();
        let observations = observation.attempts();
        let additional_alpns = additional_alpns.clone();
        attempts.spawn(
            async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }

                let attempt_no = idx + 1;
                let attempt = observations.begin(attempt_no as u32, ATTEMPT_TIMEOUT);
                let driver = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
                let options = ConnectOptions::new().with_additional_alpns(additional_alpns);
                let mut phase = ConnectionFailurePhase::Establish;
                let connect = async {
                    let connecting = endpoint
                        .connect_with_opts(addr, alpn, options)
                        .with_subscriber(driver.clone())
                        .await
                        .map_err(|error| {
                            (
                                error.to_string(),
                                super::connection_diagnostics::preparation_failure(&error),
                            )
                        })?;
                    phase = ConnectionFailurePhase::Handshake;
                    connecting.with_subscriber(driver).await.map_err(|error| {
                        (
                            error.to_string(),
                            super::connection_diagnostics::handshake_failure(&error),
                        )
                    })
                };
                match tokio::time::timeout(ATTEMPT_TIMEOUT, connect).await {
                    Ok(Ok(connection)) => {
                        attempt.connected(connection.stable_id() as u64);
                        Ok((attempt_no, connection))
                    }
                    Ok(Err((err, outcome))) => {
                        attempt.finish(outcome);
                        Err((attempt_no, err, false, outcome))
                    }
                    Err(_) => {
                        let outcome = super::connection_diagnostics::failed(
                            phase,
                            ConnectionFailureReason::TimedOut,
                        );
                        attempt.finish(outcome);
                        Err((
                            attempt_no,
                            format!("timed out after {}ms", ATTEMPT_TIMEOUT.as_millis()),
                            true,
                            outcome,
                        ))
                    }
                }
            }
            .with_current_subscriber(),
        );
    }

    let mut failures = Vec::new();
    let mut all_timed_out = true;
    let mut last_failure = super::connection_diagnostics::failed(
        ConnectionFailurePhase::Unknown,
        ConnectionFailureReason::Internal,
    );
    while let Some(joined) = attempts.join_next().await {
        match joined {
            Ok(Ok((_attempt, connection))) => {
                observation.connected(connection.stable_id() as u64);
                attempts.abort_all();
                return Ok(connection);
            }
            Ok(Err((attempt, err, timed_out, outcome))) => {
                all_timed_out &= timed_out;
                last_failure = outcome;
                failures.push(format!("attempt {attempt}: {err}"));
            }
            Err(err) => {
                all_timed_out = false;
                last_failure = super::connection_diagnostics::failed(
                    ConnectionFailurePhase::Unknown,
                    ConnectionFailureReason::Internal,
                );
                failures.push(format!("task failed: {err}"));
            }
        }
    }

    observation.finish(last_failure);
    Err((
        failures.join("; "),
        if all_timed_out {
            DialFailure::TimedOut
        } else {
            DialFailure::TransportFailed
        },
    ))
}

fn connection_purpose(purpose: &str) -> ConnectionPurpose {
    match purpose {
        "presence" => ConnectionPurpose::Presence,
        "network_recovery_confirmation" => ConnectionPurpose::NetworkRecovery,
        "clipboard" => ConnectionPurpose::Clipboard,
        "active-clipboard" => ConnectionPurpose::ActiveClipboard,
        "active-clipboard-pull" => ConnectionPurpose::ClipboardPull,
        "transfer-progress" => ConnectionPurpose::TransferProgress,
        "group-update" => ConnectionPurpose::GroupUpdate,
        "membership-attestation" => ConnectionPurpose::MembershipAttestation,
        "membership-gossip" => ConnectionPurpose::MembershipGossip,
        "membership-history" => ConnectionPurpose::MembershipHistory,
        "membership-branch-recovery" => ConnectionPurpose::MembershipRecovery,
        _ => ConnectionPurpose::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::logs::AnyValue;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Debug, Default)]
    struct BlockFirstAttempt(std::sync::atomic::AtomicBool);

    impl iroh::endpoint::EndpointHooks for BlockFirstAttempt {
        fn before_connect<'a>(
            &'a self,
            _: &'a EndpointAddr,
            _: &'a [u8],
        ) -> impl std::future::Future<Output = iroh::endpoint::BeforeConnectOutcome> + Send + 'a
        {
            async move {
                if !self.0.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    std::future::pending::<()>().await;
                }
                iroh::endpoint::BeforeConnectOutcome::Accept
            }
        }
    }

    fn records(exporter: &InMemoryLogExporter) -> Vec<serde_json::Value> {
        exporter
            .get_emitted_logs()
            .expect("logs")
            .iter()
            .filter_map(|entry| {
                let field = |name: &str| {
                    entry.record.attributes_iter().find_map(|(key, value)| {
                        if key.as_str() != name {
                            return None;
                        }
                        match value {
                            AnyValue::String(value) => Some(value.to_string()),
                            _ => None,
                        }
                    })
                };
                uc_observability_contract::diagnostics::connectivity::decode_local_record(
                    &field("event.name")?,
                    &field("payload")?,
                    entry.record.severity_text()?,
                )
                .map(serde_json::Value::Object)
            })
            .collect()
    }

    #[tokio::test]
    async fn staggered_failure_and_cancellation_account_only_for_started_attempts() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&provider));
        let dispatch = tracing::Dispatch::new(subscriber);
        let endpoint = Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_address_lookup()
                .bind()
                .await
                .expect("endpoint"),
        );
        endpoint.close().await;
        let addr = EndpointAddr::new(iroh::SecretKey::generate().public());
        let result = connect_with_staggered_retry(
            endpoint.clone(),
            addr.clone(),
            b"probe",
            "membership-history",
            AddressInputSource::Provided,
        )
        .with_subscriber(dispatch.clone())
        .await;
        assert!(result.is_err());
        let rows = records(&exporter);
        assert_eq!(
            rows.iter()
                .filter(|r| r["event.name"] == "connection.started")
                .count(),
            1
        );
        let attempts: Vec<_> = rows
            .iter()
            .filter(|r| r["event.name"] == "connection.attempt.finished")
            .collect();
        assert_eq!(attempts.len(), 3);
        assert!(attempts
            .iter()
            .all(|r| r["error.reason"] == "endpoint_closed"));
        let finished = rows
            .iter()
            .find(|r| r["event.name"] == "connection.finished")
            .expect("finish");
        assert_eq!(finished["attempt_count"], 3);
        assert_eq!(finished["outcome"], "failed");

        let result = tokio::time::timeout(
            Duration::from_millis(25),
            connect_with_staggered_retry(
                endpoint,
                addr,
                b"probe",
                "membership-history",
                AddressInputSource::Provided,
            )
            .with_subscriber(dispatch),
        )
        .await;
        assert!(result.is_err());
        let after = records(&exporter);
        let cancelled = after[rows.len()..]
            .iter()
            .find(|r| r["event.name"] == "connection.finished")
            .expect("cancelled");
        assert_eq!(cancelled["outcome"], "interrupted");
        assert_eq!(
            cancelled["attempt_count"], 1,
            "尚未醒来的错峰任务不算连接尝试"
        );
    }

    #[tokio::test]
    async fn winning_connection_cancels_a_started_loser_without_reporting_a_failure() {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber =
            tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&provider));
        let server = Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(iroh::RelayMode::Disabled)
            .clear_address_lookup()
            .alpns(vec![b"probe".to_vec()])
            .bind()
            .await
            .expect("server");
        let client = Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_address_lookup()
                .hooks(BlockFirstAttempt::default())
                .bind()
                .await
                .expect("client"),
        );
        for _ in 0..100 {
            if !server.addr().addrs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!server.addr().addrs.is_empty());
        let incoming_server = server.clone();
        let incoming = tokio::spawn(async move {
            incoming_server
                .accept()
                .await
                .expect("incoming")
                .await
                .expect("handshake")
        });
        let connected = tokio::time::timeout(
            Duration::from_secs(6),
            connect_with_staggered_retry(
                client.clone(),
                server.addr(),
                b"probe",
                "membership-history",
                AddressInputSource::Provided,
            )
            .with_subscriber(subscriber),
        )
        .await
        .expect("deadline")
        .expect("connection");
        let accepted = incoming.await.expect("accept task");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if records(&exporter)
                    .iter()
                    .any(|r| r["outcome"] == "cancelled_by_winner")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("取消终态");
        let rows = records(&exporter);
        assert_eq!(
            rows.iter()
                .filter(|r| r["event.name"] == "connection.finished")
                .count(),
            1
        );
        assert!(!rows.iter().any(|r| r["outcome"] == "failed"));
        assert_eq!(
            rows.iter()
                .find(|r| r["event.name"] == "connection.finished")
                .expect("finish")["attempt_count"],
            2
        );
        connected.close(0u32.into(), b"done");
        drop(accepted);
        client.close().await;
        server.close().await;
    }
}
