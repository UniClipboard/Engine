//! 实际协商能力决定头字节；旧端不接收新布局，拒绝不能触发旧协议重发。
use super::{
    bind_endpoint, dispatch_fixture, peer_reachability_mock, seed_addr, wait_for_direct_addrs,
    EpochOnlyAdmission, MemRepo,
};
use crate::network::iroh::admitted_connections::AdmittedConnections;
use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use std::sync::Arc;
use tokio::sync::mpsc;
use uc_application::deps::ConnectivityPolicy;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, DispatchAck, SyncPayload,
};
use uc_sync_protocol::clipboard::{
    self, AckCode, ATTRIBUTED_CLIPBOARD_ALPN, CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN,
};

#[derive(Debug)]
struct Record {
    frames: mpsc::Sender<(Vec<u8>, Vec<u8>, ClipboardHeader, Vec<u8>)>,
    ack: u8,
}
impl ProtocolHandler for Record {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(AcceptError::from_err)?;
        let mut magic = [0];
        recv.read_exact(&mut magic)
            .await
            .map_err(AcceptError::from_err)?;
        assert_eq!(magic, [clipboard::CLIPBOARD_MAGIC]);
        let mut length = [0; 4];
        recv.read_exact(&mut length)
            .await
            .map_err(AcceptError::from_err)?;
        let mut raw = vec![0; u32::from_be_bytes(length) as usize];
        recv.read_exact(&mut raw)
            .await
            .map_err(AcceptError::from_err)?;
        let header = clipboard::decode_header(&raw).map_err(AcceptError::from_err)?;
        recv.read_exact(&mut length)
            .await
            .map_err(AcceptError::from_err)?;
        let mut payload = vec![0; u32::from_be_bytes(length) as usize];
        recv.read_exact(&mut payload)
            .await
            .map_err(AcceptError::from_err)?;
        assert!(recv
            .read_to_end(1)
            .await
            .map_err(AcceptError::from_err)?
            .is_empty());
        self.frames
            .send((connection.alpn().to_vec(), raw, header, payload))
            .await
            .unwrap();
        send.write_all(&[self.ack])
            .await
            .map_err(AcceptError::from_err)?;
        send.finish().map_err(AcceptError::from_err)?;
        connection.closed().await;
        Ok(())
    }
}

#[tokio::test]
async fn clipboard_source_header_matches_actual_negotiation_and_never_replays_after_rejection() {
    for (alpn, rejected) in [
        (ATTRIBUTED_CLIPBOARD_ALPN, false),
        (MULTIPLEXED_CLIPBOARD_ALPN, false),
        (CLIPBOARD_ALPN, false),
        (ATTRIBUTED_CLIPBOARD_ALPN, true),
    ] {
        let (frames, mut received) = mpsc::channel(4);
        let peer = bind_endpoint().await;
        wait_for_direct_addrs(&peer).await;
        let ack = if rejected {
            AckCode::Rejected
        } else {
            AckCode::Accepted
        };
        let mut router = Router::builder((*peer).clone()).accept(
            alpn,
            Record {
                frames: frames.clone(),
                ack: ack.as_byte(),
            },
        );
        if rejected {
            router = router.accept(
                CLIPBOARD_ALPN,
                Record {
                    frames,
                    ack: AckCode::Accepted.as_byte(),
                },
            );
        }
        let router = router.spawn();
        let sender = bind_endpoint().await;
        wait_for_direct_addrs(&sender).await;
        let target = DeviceId::new("target");
        let repo = Arc::new(MemRepo::default());
        seed_addr(&repo, &target, &peer.addr()).await;
        let adapter = dispatch_fixture(
            sender.clone(),
            repo,
            peer_reachability_mock(),
            Arc::new(EpochOnlyAdmission(AdmittedConnections::default())),
            ConnectivityPolicy::default(),
        );
        let source = EntryId::from("42424242-4242-4242-4242-424242424242");
        let header = ClipboardHeader {
            source_entry: Some(source.clone()),
            version: 2,
            snapshot_hash: "h".into(),
            captured_at_ms: 1,
            origin_device_id: "d".into(),
            origin_device_name: "n".into(),
            payload_version: 3,
        };
        let outcome = adapter
            .dispatch(
                &target,
                &header,
                SyncPayload {
                    ciphertext: Bytes::from_static(b"original-ciphertext"),
                },
            )
            .await
            .outcome;
        if rejected {
            assert!(matches!(
                outcome,
                Err(ClipboardDispatchError::PeerRejected(_))
            ));
        } else {
            assert_eq!(outcome.unwrap(), DispatchAck::Accepted);
        }
        let (actual, raw, decoded, payload) = received.recv().await.unwrap();
        assert_eq!(actual, alpn);
        assert_eq!(payload, b"original-ciphertext");
        if alpn == ATTRIBUTED_CLIPBOARD_ALPN {
            assert_eq!(decoded.source_entry, Some(source));
            assert_eq!(raw, b"UCT2\x02\x01h\x02\x01d\x01n\x03\x00BBBBBBBBBBBBBBBB");
        } else {
            assert_eq!(decoded.source_entry, None);
            assert_eq!(raw, b"UCT1\x02\x01h\x02\x01d\x01n\x03\x00");
        }
        assert!(received.try_recv().is_err());
        drop(adapter);
        sender.close().await;
        router.shutdown().await.unwrap();
    }
}
