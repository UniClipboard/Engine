//! 实际原发送动作在首个端口调用前保留完整意图，且每个对端独立。
use super::{
    build_uc, dispatch_report, input, make_device_identity, make_local_identity_stub,
    make_member_repo_all_enabled, make_settings_stub, record, MockCipher, MockDispatch,
    MockPeerAddrRepo,
};
use crate::clipboard::sync::payload_codec::{
    encode_snapshot_with_blob_refs_to_v3_bytes, V3BlobRef,
};
use crate::transfer::progress::{ProgressProjectionKey, ProgressProjectionRegistry};
use std::sync::{Arc, OnceLock};
use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemKind};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::DispatchAck;
use uc_core::{ObservedClipboardRepresentation, SystemClipboardSnapshot};
use uuid::Uuid;

#[tokio::test]
async fn original_intent_is_complete_before_first_dispatch_and_does_not_wait_for_progress() {
    let source = ProgressIdentity::new([1; 16]);
    let other_local_entry = ProgressIdentity::new([2; 16]);
    let plan = vec![
        ProgressItem {
            kind: ProgressItemKind::Representation,
            index: 1,
        },
        ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        },
        ProgressItem {
            kind: ProgressItemKind::Representation,
            index: 0,
        },
        ProgressItem {
            kind: ProgressItemKind::File,
            index: 1,
        },
    ];
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: (0..2)
            .map(|index| {
                ObservedClipboardRepresentation::new(
                    RepresentationId::new(),
                    FormatId::from(format!("binary-{index}")),
                    None,
                    Vec::new(),
                )
            })
            .collect(),
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let refs = [Some(1), None, Some(0), None]
        .into_iter()
        .map(|representation_index| V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![7]),
            entry_id: EntryId::from(Uuid::from_bytes(source.bytes()).to_string()),
            filename: None,
            mime: None,
            size_bytes: 10,
            representation_index,
        })
        .collect::<Vec<_>>();
    let (plaintext, snapshot_hash) =
        encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &refs).unwrap();
    let observed = Arc::new(OnceLock::<ProgressProjectionRegistry>::new());
    let read = observed.clone();
    let expected = plan.clone();
    let mut dispatch = MockDispatch::new();
    dispatch
        .expect_dispatch()
        .times(2)
        .returning(move |peer, _, _| {
            let view = read
                .get()
                .unwrap()
                .snapshot(ProgressProjectionKey {
                    peer: *peer,
                    source,
                })
                .expect("original intent precedes the first message");
            assert_eq!(
                view.items,
                expected
                    .iter()
                    .map(|item| (*item, None))
                    .collect::<Vec<_>>()
            );
            assert!(view.task.is_none());
            assert!(view.bound.is_none());
            dispatch_report(Ok(DispatchAck::Accepted))
        });
    let mut repo = MockPeerAddrRepo::new();
    repo.expect_list()
        .returning(|| Ok(vec![record("peer-a"), record("peer-b"), record("peer-c")]));
    let mut cipher = MockCipher::new();
    cipher
        .expect_encrypt()
        .returning(|bytes| Ok(bytes.to_vec()));
    let uc = build_uc(
        repo,
        make_member_repo_all_enabled(),
        cipher,
        dispatch,
        make_device_identity("self-device"),
        make_local_identity_stub(),
        make_settings_stub(),
    );
    assert!(observed.set(uc.progress.clone()).is_ok());
    let mut request = input();
    request.plaintext = plaintext;
    request.snapshot_hash = snapshot_hash;
    request.entry_id = Some(EntryId::from(
        Uuid::from_bytes(other_local_entry.bytes()).to_string(),
    ));
    request.target_filter = Some(vec![DeviceId::new("peer-a"), DeviceId::new("peer-b")]);
    let result = uc.execute(request).await.unwrap();
    assert_eq!(result.total_accepted, 2);
    for peer in ["peer-a", "peer-b"] {
        let view = uc
            .progress
            .snapshot(ProgressProjectionKey {
                peer: DeviceId::new(peer),
                source,
            })
            .unwrap();
        assert_eq!(view.items.len(), 4);
        assert!(view.items.iter().all(|(_, item)| item.is_none()));
        assert!(uc
            .progress
            .snapshot(ProgressProjectionKey {
                peer: DeviceId::new(peer),
                source: other_local_entry
            })
            .is_none());
    }
    assert!(uc
        .progress
        .snapshot(ProgressProjectionKey {
            peer: DeviceId::new("peer-c"),
            source
        })
        .is_none());
}
