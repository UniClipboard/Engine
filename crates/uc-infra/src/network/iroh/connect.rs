use std::sync::Arc;
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionFailurePhase, ConnectionFailureReason, ConnectionObservation, ConnectionPurpose,
    DialFailure,
};

use iroh::endpoint::ConnectOptions;
use iroh::endpoint::Connection;
use iroh::endpoint::{ConnectWithOptsError, ConnectingError};
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use tokio::task::JoinError;
use tokio::task::JoinSet;
use tokio::time::error::Elapsed;
use tracing::instrument::WithSubscriber;
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;

use super::addr_filter::filter_endpoint_addr;
use super::runtime_consts::DialPolicy;

/// Per-attempt connect timeout.
///
/// 3s sits comfortably above the observed LAN/direct `iroh connect`
/// success latency (~1s for an Online peer's first attempt) and the
/// relay-fallback case (~1-2s when pkarr discovery completes before
/// the direct path), while keeping the worst-case staggered-retry
/// budget below [`crate::network::iroh::FAN_OUT_DEADLINE_HINT`]'s 5s
/// dispatch-side hard cap.
///
/// Concurrent content operations share one staggered dial batch per target.
/// A failed batch reports a communication failure; the peer connection owner
/// independently checks existing connections and schedules subsequent recovery.
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
/// `report_communication_failure` at 1.
const STAGGERED_DELAYS: [Duration; 3] = [
    Duration::from_millis(0),
    Duration::from_millis(500),
    Duration::from_millis(1500),
];

/// 出站拨号前按活动节点的拨号策略整理对端地址。
///
/// 1. 直连地址使用与地址发现相同的可信判定（`filter_endpoint_addr`）：Clash fake-ip 与
///    IPv4 链路本地永远丢弃，CGNAT/Tailscale 段只保留可信网段内的地址。已保存的地址与
///    邀请路由直接交给 `connect`，不经过 endpoint 的 `AddrFilter`，所以必须在这里执行。
/// 2. LAN-only 下剥掉 `TransportAddr::Relay`：本端 `RelayMode::Disabled` **不阻止** iroh
///    用对端 `EndpointAddr` 中的 relay url 发起出站连接。
///
/// 直连地址被全部丢弃时不提前失败：iroh 仍会经 mDNS 解析对端当前地址。
pub(super) fn prepare_dial_addr(addr: EndpointAddr) -> EndpointAddr {
    apply_dial_policy(addr, &super::runtime_consts::dial_policy())
}

fn apply_dial_policy(addr: EndpointAddr, policy: &DialPolicy) -> EndpointAddr {
    let addr = filter_endpoint_addr(addr, &policy.trusted_networks);
    if !policy.lan_only {
        return addr;
    }
    let EndpointAddr { id, addrs } = addr;
    let kept = addrs
        .into_iter()
        .filter(|a| !matches!(a, TransportAddr::Relay(_)));
    EndpointAddr::from_parts(id, kept)
}

/// 一次交错拨号中每个尝试都失败。只保存最能说明原因的一次尝试：
/// 优先取非超时的失败，全部超时时取最后一次超时。
#[derive(Debug, thiserror::Error)]
#[error("all staggered connection attempts failed")]
pub(crate) struct StaggeredDialError {
    failure: DialFailure,
    #[source]
    attempt: DialAttemptError,
}

impl StaggeredDialError {
    pub(crate) fn failure(&self) -> DialFailure {
        self.failure
    }
}

/// 单次拨号尝试的失败来源。
#[derive(Debug, thiserror::Error)]
pub(crate) enum DialAttemptError {
    #[error("connection setup failed")]
    Setup(#[source] ConnectWithOptsError),
    #[error("connection handshake failed")]
    Handshake(#[source] ConnectingError),
    #[error("connection attempt timed out")]
    TimedOut(#[source] Elapsed),
    #[error("connection attempt task failed")]
    Task(#[source] JoinError),
    /// 交错延迟表非空时不会出现；保留为明确分类而不是 panic。
    #[error("no connection attempt ran")]
    NotStarted,
}

impl DialAttemptError {
    fn timed_out(&self) -> bool {
        matches!(self, Self::TimedOut(_))
    }
}

pub(crate) async fn connect_with_staggered_retry(
    endpoint: Arc<Endpoint>,
    addr: EndpointAddr,
    alpn: &'static [u8],
    purpose: &'static str,
    source: AddressInputSource,
) -> Result<Connection, StaggeredDialError> {
    connect_with_staggered_retry_classified(endpoint, addr, alpn, Vec::new(), purpose, source).await
}

pub(super) async fn connect_with_staggered_retry_classified(
    endpoint: Arc<Endpoint>,
    addr: EndpointAddr,
    alpn: &'static [u8],
    additional_alpns: Vec<Vec<u8>>,
    purpose: &'static str,
    source: AddressInputSource,
) -> Result<Connection, StaggeredDialError> {
    let addr = prepare_dial_addr(addr);
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
                            let outcome =
                                super::connection_diagnostics::preparation_failure(&error);
                            (DialAttemptError::Setup(error), outcome)
                        })?;
                    phase = ConnectionFailurePhase::Handshake;
                    connecting.with_subscriber(driver).await.map_err(|error| {
                        let outcome = super::connection_diagnostics::handshake_failure(&error);
                        (DialAttemptError::Handshake(error), outcome)
                    })
                };
                match tokio::time::timeout(ATTEMPT_TIMEOUT, connect).await {
                    Ok(Ok(connection)) => {
                        attempt.connected(connection.stable_id() as u64);
                        Ok((attempt_no, connection))
                    }
                    Ok(Err((err, outcome))) => {
                        attempt.finish(outcome);
                        Err((err, outcome))
                    }
                    Err(elapsed) => {
                        let outcome = super::connection_diagnostics::failed(
                            phase,
                            ConnectionFailureReason::TimedOut,
                        );
                        attempt.finish(outcome);
                        Err((DialAttemptError::TimedOut(elapsed), outcome))
                    }
                }
            }
            .with_current_subscriber(),
        );
    }

    let mut reported: Option<DialAttemptError> = None;
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
            Ok(Err((err, outcome))) => {
                all_timed_out &= err.timed_out();
                last_failure = outcome;
                reported = Some(prefer_informative(reported, err));
            }
            Err(err) => {
                all_timed_out = false;
                last_failure = super::connection_diagnostics::failed(
                    ConnectionFailurePhase::Unknown,
                    ConnectionFailureReason::Internal,
                );
                reported = Some(prefer_informative(reported, DialAttemptError::Task(err)));
            }
        }
    }

    observation.finish(last_failure);
    let failure = if all_timed_out {
        DialFailure::TimedOut
    } else {
        DialFailure::TransportFailed
    };
    Err(StaggeredDialError {
        failure,
        attempt: reported.unwrap_or(DialAttemptError::NotStarted),
    })
}

fn prefer_informative(
    current: Option<DialAttemptError>,
    next: DialAttemptError,
) -> DialAttemptError {
    match current {
        Some(current) if next.timed_out() && !current.timed_out() => current,
        _ => next,
    }
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
    use uc_core::network::TrustedNetworks;

    fn mixed_peer_addr() -> EndpointAddr {
        let relay: iroh::RelayUrl = "https://relay.example.com".parse().unwrap();
        EndpointAddr::from_parts(
            iroh::SecretKey::generate().public(),
            [
                TransportAddr::Ip("10.8.0.2:42000".parse().unwrap()),
                TransportAddr::Ip("100.64.1.9:42000".parse().unwrap()),
                TransportAddr::Ip("198.18.0.1:42000".parse().unwrap()),
                TransportAddr::Relay(relay),
            ],
        )
    }

    fn dial_ips(addr: &EndpointAddr) -> Vec<String> {
        addr.ip_addrs()
            .map(|socket| socket.ip().to_string())
            .collect()
    }

    #[test]
    fn lan_only_dial_keeps_trusted_and_plain_private_addrs_without_relay() {
        let policy = DialPolicy {
            lan_only: true,
            trusted_networks: TrustedNetworks::parse(&["100.64.1.0/24"]).unwrap(),
        };
        let addr = apply_dial_policy(mixed_peer_addr(), &policy);

        assert_eq!(dial_ips(&addr), vec!["10.8.0.2", "100.64.1.9"]);
        assert!(!addr
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Relay(_))));
    }

    #[test]
    fn dial_drops_untrusted_overlay_but_keeps_relay_outside_lan_only() {
        let addr = apply_dial_policy(mixed_peer_addr(), &DialPolicy::default());

        assert_eq!(dial_ips(&addr), vec!["10.8.0.2"]);
        assert!(addr
            .addrs
            .iter()
            .any(|a| matches!(a, TransportAddr::Relay(_))));
    }

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

    async fn elapsed() -> Elapsed {
        tokio::time::timeout(Duration::ZERO, std::future::pending::<()>())
            .await
            .unwrap_err()
    }

    #[tokio::test]
    async fn dial_error_keeps_a_non_timeout_attempt_over_later_timeouts() {
        let kept = prefer_informative(
            Some(DialAttemptError::NotStarted),
            DialAttemptError::TimedOut(elapsed().await),
        );
        assert!(matches!(kept, DialAttemptError::NotStarted));

        let replaced = prefer_informative(
            Some(DialAttemptError::TimedOut(elapsed().await)),
            DialAttemptError::NotStarted,
        );
        assert!(matches!(replaced, DialAttemptError::NotStarted));

        let error = StaggeredDialError {
            failure: DialFailure::TimedOut,
            attempt: DialAttemptError::TimedOut(elapsed().await),
        };
        let source = std::error::Error::source(&error).expect("attempt source");
        assert!(source.downcast_ref::<DialAttemptError>().is_some());
        assert!(matches!(error.failure(), DialFailure::TimedOut));
    }
}
