//! `PerPeerDispatcher` — the per-target dispatch body fanned out by the
//! use case. Owns the four ports the JoinSet task touches 1:1 per peer
//! (wire dispatch + presence preflight + the two telemetry funnels) so the
//! "send to one peer and emit its per-peer analytics" concern has a single
//! home and an independent test surface.
//!
//! ## Load-bearing invariants (Slice 8c funnel)
//!
//! - `sync_attempted` fires BEFORE any dial — on every path, including the
//!   known-offline deferral — so `attempted = succeeded + failed +
//!   deferred` holds and the dashboard can derive user-perceived attempts
//!   as `attempted - deferred`.
//! - A presence preflight of `Offline` short-circuits to
//!   `SyncDeferred(PeerKnownOffline)` + `Err(Offline)` WITHOUT dialing —
//!   redialing a peer the dispatch adapter already concluded unreachable
//!   would only burn the fan-out deadline (see the use-case module doc).
//! - `sync_latency_ms` is recorded only on success; a failure has no
//!   end-to-end timing.
//! - `mark_*` errors are warn-only and never abort the dial.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::warn;
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, DispatchReport, DispatchTiming,
    FirstSyncStatePort, PeerReachabilityPort, ReachabilityState, SyncPayload,
};
use uc_observability_contract::analytics::{
    AnalyticsPort, Direction, Event, PayloadSizeBucket, PayloadType, SyncDeferReason,
    SyncDeferredProps, SyncEventProps, TransportType,
};
use uc_observability_contract::otlp::{
    log_clipboard_sync_stage, ClipboardSyncStage as OtlpClipboardSyncStage, ClipboardSyncTiming,
};
use uc_observability_contract::FlowId;

use super::{
    dispatch_failure_stage, map_dispatch_error_to_failure_reason, transport_type_from_channel,
    PeerDispatchResult,
};

pub(crate) struct PerPeerDispatcher {
    clipboard_dispatch: Arc<dyn ClipboardDispatchPort>,
    presence: Arc<dyn PeerReachabilityPort>,
    analytics: Arc<dyn AnalyticsPort>,
    first_sync_state: Arc<dyn FirstSyncStatePort>,
}

impl PerPeerDispatcher {
    pub(crate) fn new(
        clipboard_dispatch: Arc<dyn ClipboardDispatchPort>,
        presence: Arc<dyn PeerReachabilityPort>,
        analytics: Arc<dyn AnalyticsPort>,
        first_sync_state: Arc<dyn FirstSyncStatePort>,
    ) -> Self {
        Self {
            clipboard_dispatch,
            presence,
            analytics,
            first_sync_state,
        }
    }

    /// Fire `sync_attempted` (+ first-attempt funnel). ALWAYS called before
    /// any dial so funnel parity holds on every path; the deferred path
    /// calls it too. `mark_*` failure is warn-only.
    async fn capture_attempted(
        &self,
        payload_type: PayloadType,
        payload_size_bucket: PayloadSizeBucket,
    ) {
        self.analytics.capture(Event::SyncAttempted(SyncEventProps {
            direction: Direction::Outbound,
            payload_type,
            payload_size_bucket,
            // `sync_attempted` fires BEFORE any dial, so no connection path
            // exists yet — report `Unknown` rather than guessing a route.
            transport_type: TransportType::Unknown,
            peer_os: None,
            sync_latency_ms: None,
            failure_reason: None,
            failure_stage: None,
        }));
        match self.first_sync_state.mark_first_sync_attempted().await {
            Ok(true) => self.analytics.capture(Event::FirstClipboardSyncAttempted {
                direction: Direction::Outbound,
            }),
            Ok(false) => {}
            Err(err) => warn!(
                error = %err,
                "first_sync_state.mark_first_sync_attempted failed; skipping fire",
            ),
        }
    }

    /// Dispatch one encrypted payload to one peer, emitting that peer's
    /// per-peer telemetry, and return the device + wire outcome for the
    /// fan-out to fold.
    pub(crate) async fn dispatch_one(
        &self,
        device_id: DeviceId,
        header: Arc<ClipboardHeader>,
        payload: SyncPayload,
        payload_type: PayloadType,
        payload_size_bucket: PayloadSizeBucket,
        source_started_at: Option<Instant>,
    ) -> PeerDispatchResult {
        // Preflight presence, then fire attempted (ordering: attempted must
        // precede the dial / deferral so funnel parity holds).
        let preflight_state = self.presence.current_state(&device_id).await;
        let known_offline = matches!(preflight_state, ReachabilityState::Offline);
        self.capture_attempted(payload_type, payload_size_bucket)
            .await;

        // Skip the dial entirely when presence already reports Offline. The
        // dispatch adapter writes presence Offline on its own dial failures
        // and enforces a TTL re-dial, so by the time `known_offline` is true
        // we have first-hand evidence the peer is unreachable. Telemetry
        // fires `sync_deferred` (not `sync_failed`) to preserve
        // attempted+deferred parity.
        if known_offline {
            self.analytics
                .capture(Event::SyncDeferred(SyncDeferredProps {
                    direction: Direction::Outbound,
                    payload_type,
                    payload_size_bucket,
                    peer_os: None,
                    defer_reason: SyncDeferReason::PeerKnownOffline,
                }));
            return (device_id, Err(ClipboardDispatchError::Offline));
        }

        let started_at = Instant::now();
        let source_to_dispatch_ms = source_started_at.map(|source_started_at| {
            duration_ms(started_at.saturating_duration_since(source_started_at))
        });
        let DispatchReport {
            transport,
            timing,
            outcome,
        } = self
            .clipboard_dispatch
            .dispatch(&device_id, &header, payload)
            .await;
        let dispatch_to_remote_commit_ms = duration_ms(started_at.elapsed());
        let copy_to_remote_commit_ms =
            source_started_at.map(|source_started_at| duration_ms(source_started_at.elapsed()));
        // Translate the path the adapter actually used (probed post-settle)
        // into the analytics bucket. `Unknown` when no active path resolved —
        // e.g. a dial failure or a mid-handshake snapshot.
        let transport_type = transport_type_from_channel(transport);

        let event = match &outcome {
            Ok(_) => Event::SyncSucceeded(SyncEventProps {
                direction: Direction::Outbound,
                payload_type,
                payload_size_bucket,
                transport_type,
                peer_os: None,
                sync_latency_ms: Some(dispatch_to_remote_commit_ms),
                failure_reason: None,
                failure_stage: None,
            }),
            Err(err) => Event::SyncFailed(SyncEventProps {
                direction: Direction::Outbound,
                payload_type,
                payload_size_bucket,
                transport_type,
                peer_os: None,
                sync_latency_ms: None,
                failure_reason: Some(map_dispatch_error_to_failure_reason(err)),
                failure_stage: Some(dispatch_failure_stage(err)),
            }),
        };
        let is_ok = outcome.is_ok();
        self.analytics.capture(event);

        if is_ok {
            let (flow_id, flow_synthetic) = timing_flow_id(&header);
            self.capture_completed_stages(
                &flow_id,
                flow_synthetic,
                payload_type,
                payload_size_bucket,
                transport_type,
                source_to_dispatch_ms,
                copy_to_remote_commit_ms,
                dispatch_to_remote_commit_ms,
                timing,
            );
        }

        // First-success funnel: fires the generic clipboard event and (if
        // File) the file-specific event. Both flags dedup independently.
        if is_ok {
            match self.first_sync_state.mark_first_sync_succeeded().await {
                Ok(true) => self.analytics.capture(Event::FirstClipboardSyncSucceeded {
                    direction: Direction::Outbound,
                    peer_os: None,
                    transport_type,
                    duration_ms: dispatch_to_remote_commit_ms,
                }),
                Ok(false) => {}
                Err(err) => warn!(
                    error = %err,
                    "first_sync_state.mark_first_sync_succeeded failed; skipping fire",
                ),
            }
            if matches!(payload_type, PayloadType::File) {
                match self.first_sync_state.mark_first_file_sync_succeeded().await {
                    Ok(true) => self.analytics.capture(Event::FirstFileSyncSucceeded {
                        peer_os: None,
                        transport_type,
                        payload_size_bucket,
                    }),
                    Ok(false) => {}
                    Err(err) => warn!(
                        error = %err,
                        "first_sync_state.mark_first_file_sync_succeeded failed; skipping fire",
                    ),
                }
            }
        }

        (device_id, outcome)
    }

    fn capture_completed_stages(
        &self,
        flow_id: &FlowId,
        flow_synthetic: bool,
        payload_type: PayloadType,
        payload_size_bucket: PayloadSizeBucket,
        transport_type: TransportType,
        source_to_dispatch_ms: Option<u32>,
        copy_to_remote_commit_ms: Option<u32>,
        dispatch_to_remote_commit_ms: u32,
        timing: DispatchTiming,
    ) {
        let log = |stage, duration_ms| {
            log_clipboard_sync_stage(ClipboardSyncTiming {
                flow_id,
                flow_synthetic,
                direction: Direction::Outbound,
                payload_type,
                payload_size_bucket,
                transport_type,
                stage,
                duration_ms,
            });
        };

        if let Some(duration_ms) = source_to_dispatch_ms {
            log(OtlpClipboardSyncStage::SourceToDispatch, duration_ms);
        }
        log(
            OtlpClipboardSyncStage::AddressResolution,
            timing.address_resolution_ms,
        );
        if let Some(duration_ms) = timing.connection_ms {
            log(OtlpClipboardSyncStage::Connection, duration_ms);
        }
        if let Some(duration_ms) = timing.stream_open_ms {
            log(OtlpClipboardSyncStage::StreamOpen, duration_ms);
        }
        if let Some(duration_ms) = timing.frame_write_ms {
            log(OtlpClipboardSyncStage::FrameWrite, duration_ms);
        }
        if let Some(duration_ms) = timing.receiver_apply_wait_ms {
            log(OtlpClipboardSyncStage::ReceiverApplyWait, duration_ms);
        }
        log(
            OtlpClipboardSyncStage::DispatchToRemoteCommit,
            dispatch_to_remote_commit_ms,
        );
        if let Some(duration_ms) = copy_to_remote_commit_ms {
            log(OtlpClipboardSyncStage::CopyToRemoteCommit, duration_ms);
        }
    }
}

fn timing_flow_id(header: &ClipboardHeader) -> (FlowId, bool) {
    match header.flow_id.as_deref().map(FlowId::parse_str) {
        Some(Ok(flow_id)) => (flow_id, false),
        Some(Err(_)) | None => (FlowId::generate(), true),
    }
}

fn duration_ms(duration: Duration) -> u32 {
    duration.as_millis().min(u32::MAX as u128) as u32
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use super::super::test_support::*;
    use super::*;

    use tracing_subscriber::fmt::MakeWriter;
    use uc_core::ports::{ConnectionChannel, DispatchAck, DispatchTiming};
    use uc_observability_contract::analytics::{FailureReason, SyncFailureStage};
    use uc_observability_contract::FlowId;

    #[derive(Clone, Default)]
    struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

    struct Writer(CapturedWriter);

    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                 .0
                .lock()
                .expect("captured log writer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for CapturedWriter {
        type Writer = Writer;

        fn make_writer(&'a self) -> Self::Writer {
            Writer(self.clone())
        }
    }

    impl CapturedWriter {
        fn output(&self) -> String {
            String::from_utf8(self.0.lock().expect("captured log writer lock").clone())
                .expect("UTF-8 log output")
        }
    }

    fn header() -> Arc<ClipboardHeader> {
        Arc::new(test_header())
    }

    fn header_with_flow() -> (Arc<ClipboardHeader>, FlowId) {
        let flow_id = FlowId::generate();
        let mut header = test_header();
        header.flow_id = Some(flow_id.to_string());
        (Arc::new(header), flow_id)
    }

    fn bucket() -> PayloadSizeBucket {
        PayloadSizeBucket::from_bytes(11)
    }

    /// Build a `DispatchReport` with an explicit connection path so the test
    /// drives the channel→`TransportType` mapping under assertion (the whole
    /// point of this funnel — see `transport_type_from_channel`).
    fn report(
        transport: ConnectionChannel,
        outcome: Result<DispatchAck, ClipboardDispatchError>,
    ) -> DispatchReport {
        DispatchReport {
            transport,
            outcome,
            timing: DispatchTiming {
                address_resolution_ms: 11,
                connection_ms: Some(22),
                stream_open_ms: Some(33),
                frame_write_ms: Some(44),
                receiver_apply_wait_ms: Some(55),
            },
        }
    }

    /// Online (Unknown) peer + accepting dispatch: `sync_attempted` fires
    /// before the dial, then `sync_succeeded` carries latency. Result is the
    /// peer's `Ok(ack)`.
    #[tokio::test]
    async fn dispatch_one_success_fires_attempted_then_succeeded_with_latency() {
        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch()
            .times(1)
            .returning(|_, _, _| report(ConnectionChannel::Direct, Ok(DispatchAck::Accepted)));

        let analytics = Arc::new(CapturingAnalyticsSink::default());
        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Unknown)),
            analytics.clone(),
            Arc::new(AllMarkedFirstSyncState),
        );

        let (device, outcome) = dispatcher
            .dispatch_one(
                dev("peer-a"),
                header(),
                sync_payload(),
                PayloadType::Text,
                bucket(),
                None,
            )
            .await;

        assert_eq!(device.as_str(), "peer-a");
        assert!(matches!(outcome, Ok(DispatchAck::Accepted)));

        let events = analytics.events();
        assert_eq!(events.len(), 2, "got {events:?}");
        match &events[0] {
            // attempted fires before the dial → no path resolved yet.
            Event::SyncAttempted(p) => assert_eq!(p.transport_type, TransportType::Unknown),
            other => panic!("expected SyncAttempted, got {other:?}"),
        }
        match &events[1] {
            Event::SyncSucceeded(p) => {
                assert_eq!(p.direction, Direction::Outbound);
                // Direct channel maps to the P2pDirect bucket.
                assert_eq!(p.transport_type, TransportType::P2pDirect);
                assert!(p.sync_latency_ms.is_some());
                assert!(p.failure_reason.is_none());
                assert!(p.failure_stage.is_none());
            }
            other => panic!("expected SyncSucceeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_one_writes_relay_stage_timings_to_the_otlp_log() {
        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch()
            .times(1)
            .returning(|_, _, _| report(ConnectionChannel::Relay, Ok(DispatchAck::Accepted)));

        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Unknown)),
            Arc::new(CapturingAnalyticsSink::default()),
            Arc::new(AllMarkedFirstSyncState),
        );
        let (header, flow_id) = header_with_flow();
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let tracing_dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&tracing_dispatch);

        let (_, outcome) = dispatcher
            .dispatch_one(
                dev("peer-relay"),
                header,
                sync_payload(),
                PayloadType::Text,
                bucket(),
                Some(std::time::Instant::now()),
            )
            .await;

        assert!(matches!(outcome, Ok(DispatchAck::Accepted)));

        let output = writer.output();
        assert!(output.contains(&format!("flow_id={flow_id}")));
        assert!(output.contains("sync_transport=\"relay\""));
        assert!(output.contains("sync_stage=\"address_resolution\""));
        assert!(output.contains("sync_stage=\"receiver_apply_wait\""));
        assert!(output.contains("sync_stage=\"copy_to_remote_commit\""));
    }

    #[tokio::test]
    async fn dispatch_one_writes_direct_stage_timings_to_the_otlp_log() {
        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch()
            .times(1)
            .returning(|_, _, _| report(ConnectionChannel::Direct, Ok(DispatchAck::Accepted)));

        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Unknown)),
            Arc::new(CapturingAnalyticsSink::default()),
            Arc::new(AllMarkedFirstSyncState),
        );
        let (header, flow_id) = header_with_flow();
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let tracing_dispatch = tracing::Dispatch::new(subscriber);
        let _guard = tracing::dispatcher::set_default(&tracing_dispatch);

        let (_, outcome) = dispatcher
            .dispatch_one(
                dev("peer-direct"),
                header,
                sync_payload(),
                PayloadType::Text,
                bucket(),
                None,
            )
            .await;

        assert!(matches!(outcome, Ok(DispatchAck::Accepted)));

        let output = writer.output();
        assert!(output.contains(&format!("flow_id={flow_id}")));
        assert!(output.contains("sync_transport=\"p2p_direct\""));
        assert!(output.contains("sync_stage=\"address_resolution\""));
        assert!(output.contains("sync_stage=\"dispatch_to_remote_commit\""));
        assert!(!output.contains("peer-direct"));
        assert!(!output.contains("ciphertext"));
    }

    /// Presence `Offline` short-circuits: the dispatch port is NEVER touched,
    /// `sync_attempted` still fires (funnel parity), and the deferral is
    /// reported via `sync_deferred(PeerKnownOffline)` + `Err(Offline)`.
    #[tokio::test]
    async fn dispatch_one_known_offline_skips_dial_and_fires_deferred() {
        let mut dispatch = MockDispatch::new();
        dispatch.expect_dispatch().times(0);

        let analytics = Arc::new(CapturingAnalyticsSink::default());
        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Offline)),
            analytics.clone(),
            Arc::new(AllMarkedFirstSyncState),
        );

        let (device, outcome) = dispatcher
            .dispatch_one(
                dev("peer-off"),
                header(),
                sync_payload(),
                PayloadType::Text,
                bucket(),
                None,
            )
            .await;

        assert_eq!(device.as_str(), "peer-off");
        assert!(matches!(outcome, Err(ClipboardDispatchError::Offline)));

        let events = analytics.events();
        assert_eq!(events.len(), 2, "got {events:?}");
        assert!(matches!(events[0], Event::SyncAttempted(_)));
        match &events[1] {
            Event::SyncDeferred(p) => {
                assert_eq!(p.defer_reason, SyncDeferReason::PeerKnownOffline);
                assert_eq!(p.direction, Direction::Outbound);
            }
            other => panic!("expected SyncDeferred, got {other:?}"),
        }
    }

    /// A failing dispatch fires `sync_failed` with a mapped reason/stage and
    /// NO latency (a failure has no end-to-end timing).
    #[tokio::test]
    async fn dispatch_one_failure_fires_failed_without_latency() {
        let mut dispatch = MockDispatch::new();
        dispatch.expect_dispatch().times(1).returning(|_, _, _| {
            report(
                ConnectionChannel::Relay,
                Err(ClipboardDispatchError::PeerRejected("nope".to_string())),
            )
        });

        let analytics = Arc::new(CapturingAnalyticsSink::default());
        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Unknown)),
            analytics.clone(),
            Arc::new(AllMarkedFirstSyncState),
        );

        let (device, outcome) = dispatcher
            .dispatch_one(
                dev("peer-rej"),
                header(),
                sync_payload(),
                PayloadType::Text,
                bucket(),
                None,
            )
            .await;

        assert_eq!(device.as_str(), "peer-rej");
        assert!(matches!(
            outcome,
            Err(ClipboardDispatchError::PeerRejected(_))
        ));

        let events = analytics.events();
        assert_eq!(events.len(), 2, "got {events:?}");
        assert!(matches!(events[0], Event::SyncAttempted(_)));
        match &events[1] {
            Event::SyncFailed(p) => {
                assert!(p.sync_latency_ms.is_none());
                // A failure still rode a path — attribute it (Relay here).
                assert_eq!(p.transport_type, TransportType::Relay);
                assert_eq!(p.failure_reason, Some(FailureReason::NetworkError));
                assert_eq!(p.failure_stage, Some(SyncFailureStage::ImmediateSend));
            }
            other => panic!("expected SyncFailed, got {other:?}"),
        }
    }

    /// First-success funnel: a fresh `first_sync_state` + a `File` payload
    /// fires each `first_*` event exactly once (generic clipboard + file).
    #[tokio::test]
    async fn dispatch_one_first_file_success_fires_each_first_event_once() {
        let mut dispatch = MockDispatch::new();
        dispatch
            .expect_dispatch()
            .times(1)
            .returning(|_, _, _| report(ConnectionChannel::Direct, Ok(DispatchAck::Accepted)));

        let analytics = Arc::new(CapturingAnalyticsSink::default());
        let dispatcher = PerPeerDispatcher::new(
            Arc::new(dispatch),
            Arc::new(StaticPresence(ReachabilityState::Unknown)),
            analytics.clone(),
            Arc::new(InMemoryFirstSyncState::default()),
        );

        let _ = dispatcher
            .dispatch_one(
                dev("peer-a"),
                header(),
                sync_payload(),
                PayloadType::File,
                bucket(),
                None,
            )
            .await;

        let events = analytics.events();
        let first_attempted = events
            .iter()
            .filter(|e| matches!(e, Event::FirstClipboardSyncAttempted { .. }))
            .count();
        let first_succeeded = events
            .iter()
            .filter(|e| matches!(e, Event::FirstClipboardSyncSucceeded { .. }))
            .count();
        let first_file = events
            .iter()
            .filter(|e| matches!(e, Event::FirstFileSyncSucceeded { .. }))
            .count();
        assert_eq!(
            (first_attempted, first_succeeded, first_file),
            (1, 1, 1),
            "got {events:?}"
        );
        // The first-success funnel also carries the resolved path.
        let first_file_transport = events.iter().find_map(|e| match e {
            Event::FirstFileSyncSucceeded { transport_type, .. } => Some(*transport_type),
            _ => None,
        });
        assert_eq!(first_file_transport, Some(TransportType::P2pDirect));
    }

    /// The channel→`TransportType` mapping reaches `sync_succeeded` for every
    /// non-direct path: `Relay` stays `Relay`, while both `Unknown` (mid-
    /// handshake) and `Offline` (no resolvable path) collapse to `Unknown` so
    /// the dashboard never miscounts an unresolved route as direct.
    #[tokio::test]
    async fn dispatch_one_maps_each_channel_to_its_transport_bucket() {
        let cases = [
            (ConnectionChannel::Relay, TransportType::Relay),
            (ConnectionChannel::Unknown, TransportType::Unknown),
            (ConnectionChannel::Offline, TransportType::Unknown),
        ];

        for (channel, expected) in cases {
            let mut dispatch = MockDispatch::new();
            dispatch
                .expect_dispatch()
                .times(1)
                .returning(move |_, _, _| report(channel, Ok(DispatchAck::Accepted)));

            let analytics = Arc::new(CapturingAnalyticsSink::default());
            let dispatcher = PerPeerDispatcher::new(
                Arc::new(dispatch),
                Arc::new(StaticPresence(ReachabilityState::Unknown)),
                analytics.clone(),
                Arc::new(AllMarkedFirstSyncState),
            );

            let _ = dispatcher
                .dispatch_one(
                    dev("peer-x"),
                    header(),
                    sync_payload(),
                    PayloadType::Text,
                    bucket(),
                    None,
                )
                .await;

            let events = analytics.events();
            match &events[1] {
                Event::SyncSucceeded(p) => assert_eq!(
                    p.transport_type, expected,
                    "channel {channel:?} should map to {expected:?}"
                ),
                other => panic!("expected SyncSucceeded, got {other:?}"),
            }
        }
    }
}
