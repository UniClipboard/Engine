use crate::error_codes::*;

use uc_application::facade::{AppFacade, PeerSnapshotView, RosterError};
use uc_core::ports::ConnectionChannel;

use crate::{
    EngineError, EngineErrorCategory, OperationResult, PeerConnectionChannelSummary,
    PeerConnectionRefreshSummary, PeerConnectionSummary,
};

pub(crate) async fn execute_query_peer_connections(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let peers = facade
        .list_peer_snapshots()
        .await
        .map_err(map_query_error)?;
    Ok(OperationResult::PeerConnections(
        peers
            .into_iter()
            .map(|peer| PeerConnectionSummary {
                peer_id: peer.peer_id,
                device_name: peer.device_name,
                addresses: peer.addresses,
                is_paired: peer.is_paired,
                connected: peer.connected,
                pairing_state: peer.pairing_state,
                channel: map_channel(peer.channel),
                connection_address: peer.connection_address,
            })
            .collect(),
    ))
}

pub(crate) async fn execute_refresh_peer_connections(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let report = facade.refresh_presence().await.map_err(|_| {
        EngineError::new(
            REFRESH_PEER_CONNECTIONS_FAILED_CODE,
            EngineErrorCategory::Unavailable,
            true,
        )
    })?;
    log_relay_connections(facade).await;
    Ok(OperationResult::PeerConnectionsRefreshed(
        PeerConnectionRefreshSummary {
            total: report.total as u32,
            online: report.online as u32,
            offline: report.offline as u32,
            errors: report.errors.len() as u32,
        },
    ))
}

/// 在每次拨号或恢复成功后，记录仍在线且经中继连接的对端本次最终选择的
/// 中继地址。快照查询失败只记录脱敏类别，不改变刷新结果。
async fn log_relay_connections(facade: &AppFacade) {
    match facade.list_peer_snapshots().await {
        Ok(peers) => {
            for peer in &peers {
                log_relay_connection(peer);
            }
        }
        Err(error) => {
            let category = map_query_error(error).category();
            tracing::warn!(
                error_category = ?category,
                "relay log unavailable after peer refresh"
            );
        }
    }
}

fn log_relay_connection(peer: &PeerSnapshotView) {
    if let Some((device_id, relay_url)) = relay_connected_peer(peer) {
        tracing::info!(device_id, relay_url, "peer connected via relay");
    }
}

/// 本次连接最终选择中继的对端：在线、活跃通道为中继且存在中继地址。
fn relay_connected_peer(peer: &PeerSnapshotView) -> Option<(&str, &str)> {
    if peer.connected && peer.channel == ConnectionChannel::Relay {
        peer.connection_address
            .as_deref()
            .map(|relay_url| (peer.peer_id.as_str(), relay_url))
    } else {
        None
    }
}

fn map_channel(channel: ConnectionChannel) -> PeerConnectionChannelSummary {
    match channel {
        ConnectionChannel::Direct => PeerConnectionChannelSummary::Direct,
        ConnectionChannel::Relay => PeerConnectionChannelSummary::Relay,
        ConnectionChannel::Offline => PeerConnectionChannelSummary::Offline,
        ConnectionChannel::Unknown => PeerConnectionChannelSummary::Unknown,
    }
}

fn map_query_error(error: RosterError) -> EngineError {
    let (category, retryable) = match error {
        RosterError::Unavailable => (EngineErrorCategory::Unavailable, true),
        _ => (EngineErrorCategory::Internal, false),
    };
    EngineError::new(QUERY_PEER_CONNECTIONS_FAILED_CODE, category, retryable)
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::fmt::MakeWriter;

    use super::*;

    fn relay_peer(device_id: &str, relay_url: Option<&str>) -> PeerSnapshotView {
        PeerSnapshotView {
            peer_id: device_id.to_owned(),
            device_name: None,
            addresses: Vec::new(),
            is_paired: true,
            connected: true,
            pairing_state: "Trusted".to_owned(),
            channel: ConnectionChannel::Relay,
            connection_address: relay_url.map(str::to_owned),
        }
    }

    #[test]
    fn connection_channels_have_a_total_stable_mapping() {
        assert_eq!(
            map_channel(ConnectionChannel::Direct),
            PeerConnectionChannelSummary::Direct
        );
        assert_eq!(
            map_channel(ConnectionChannel::Relay),
            PeerConnectionChannelSummary::Relay
        );
        assert_eq!(
            map_channel(ConnectionChannel::Offline),
            PeerConnectionChannelSummary::Offline
        );
        assert_eq!(
            map_channel(ConnectionChannel::Unknown),
            PeerConnectionChannelSummary::Unknown
        );
    }

    #[test]
    fn query_errors_do_not_expose_internal_details() {
        let error = map_query_error(RosterError::MemberRepository(
            "private database path".to_string(),
        ));

        assert_eq!(error.code(), QUERY_PEER_CONNECTIONS_FAILED_CODE);
        assert_eq!(error.category(), EngineErrorCategory::Internal);
        assert!(!format!("{error:?}").contains("private database path"));
    }

    #[test]
    fn relay_connected_peer_requires_connected_relay_with_address() {
        let peer = relay_peer("relay-peer", Some("https://relay.example.com/"));
        assert_eq!(
            relay_connected_peer(&peer),
            Some(("relay-peer", "https://relay.example.com/"))
        );

        let without_address = relay_peer("relay-peer", None);
        assert_eq!(relay_connected_peer(&without_address), None);

        let offline = PeerSnapshotView {
            connected: false,
            ..relay_peer("relay-peer", Some("https://relay.example.com/"))
        };
        assert_eq!(relay_connected_peer(&offline), None);

        let direct = PeerSnapshotView {
            channel: ConnectionChannel::Direct,
            ..relay_peer("direct-peer", Some("https://relay.example.com/"))
        };
        assert_eq!(relay_connected_peer(&direct), None);

        let unknown = PeerSnapshotView {
            channel: ConnectionChannel::Unknown,
            ..relay_peer("unknown-peer", Some("https://relay.example.com/"))
        };
        assert_eq!(relay_connected_peer(&unknown), None);
    }

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

    #[test]
    fn relay_log_records_only_connected_relay_peers_with_address() {
        let writer = CapturedWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(writer.clone())
            .finish();
        let dispatch = tracing::Dispatch::new(subscriber);

        let relayed = relay_peer("relay-peer", Some("https://relay.example.com/"));
        let relayed_without_address = relay_peer("relay-no-address", None);
        let direct = PeerSnapshotView {
            channel: ConnectionChannel::Direct,
            ..relay_peer("direct-peer", Some("https://relay.example.com/"))
        };
        let offline = PeerSnapshotView {
            connected: false,
            ..relay_peer("offline-relay-peer", Some("https://relay.example.com/"))
        };

        tracing::dispatcher::with_default(&dispatch, || {
            log_relay_connection(&relayed);
            log_relay_connection(&relayed_without_address);
            log_relay_connection(&direct);
            log_relay_connection(&offline);
        });

        let output = writer.output();
        assert!(output.contains("peer connected via relay"));
        assert!(output.contains("device_id=\"relay-peer\""));
        assert!(output.contains("relay_url=\"https://relay.example.com/\""));
        assert!(!output.contains("relay-no-address"));
        assert!(!output.contains("direct-peer"));
        assert!(!output.contains("offline-relay-peer"));
    }
}
