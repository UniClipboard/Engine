//! 阶段 0：失败展示快照之外，原始 V3 接收材料足以重建同进程重试来源。
use super::{test_materializer, MockBlobFetcher};
use crate::clipboard::sync::apply_inbound::materializer::{
    InboundBlobMaterializer, ReceiveWorkPlan,
};
use crate::clipboard::sync::payload_codec::{
    decode_v3_bytes_to_snapshot_and_blob_refs, encode_snapshot_with_blob_refs_to_v3_bytes,
    V3BlobRef,
};
use crate::facade::blob_transfer::FetchBlobToPathResult;
use bytes::Bytes;
use std::io;
use std::sync::{Arc, Mutex};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::{MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot};

struct OriginalReceive {
    source: DeviceId,
    entry: EntryId,
    wire: Bytes,
    expected_hash: String,
}
impl OriginalReceive {
    fn plan(&self, attempt: &str) -> ReceiveWorkPlan {
        let (snapshot, refs) = decode_v3_bytes_to_snapshot_and_blob_refs(&self.wire).unwrap();
        ReceiveWorkPlan::new(
            self.source,
            self.entry.clone(),
            snapshot,
            refs,
            None,
            Some(attempt.to_owned()),
            Some(self.expected_hash.clone()),
        )
    }
}

#[tokio::test]
async fn failed_file_retries_original_v3_material_instead_of_missing_placeholder() {
    let cache = tempfile::tempdir().unwrap();
    let ticket = BlobTicket::from_bytes(vec![7, 3, 9, 11]);
    let original = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: vec![ObservedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("files"),
            Some(MimeType("text/uri-list".into())),
            b"file:///synthetic-source/original.bin\r\n".to_vec(),
        )],
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let source_ref = V3BlobRef {
        ticket: ticket.clone(),
        entry_id: EntryId::from("original-source-entry"),
        filename: Some("original.bin".into()),
        mime: Some("application/octet-stream".into()),
        size_bytes: 26,
        representation_index: None,
    };
    let (wire, expected_hash) =
        encode_snapshot_with_blob_refs_to_v3_bytes(&original, &[source_ref.clone()]).unwrap();
    let recipe = OriginalReceive {
        source: DeviceId::new("original-source-device"),
        entry: EntryId::from("same-receiver-entry"),
        wire: wire.clone(),
        expected_hash: expected_hash.clone(),
    };
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(2)
        .returning(move |command| {
            let mut calls = recorded.lock().unwrap();
            assert_eq!(command.ticket, ticket);
            assert_eq!(command.entry_id, EntryId::from("original-source-entry"));
            let context = command.transfer_context.as_ref().unwrap();
            calls.push((
                context.attempt_id.clone(),
                context.peer_id.clone(),
                command.target_path.clone(),
            ));
            if calls.len() == 1 {
                return Err(io::Error::from(io::ErrorKind::ConnectionReset).into());
            }
            let bytes = b"original-source-file-bytes";
            std::fs::write(&command.target_path, bytes).unwrap();
            Ok(FetchBlobToPathResult {
                entry_id: command.entry_id,
                plaintext_hash: PlaintextHash::from_bytes([5; 32]),
                digest: BlobDigest::from_bytes([6; 32]),
                bytes_written: bytes.len() as u64,
            })
        });
    let materializer = test_materializer(Arc::new(fetcher), cache.path().to_path_buf());
    let failed = materializer
        .materialize_plan(recipe.plan("old-attempt"))
        .await
        .unwrap();
    assert!(failed.is_partial());
    let display = failed
        .snapshot
        .representations
        .iter()
        .find(|rep| rep.format_id.as_ref() == "files")
        .unwrap()
        .expect_inline_bytes();
    assert!(String::from_utf8_lossy(display).contains("uniclip-missing://"));
    assert_ne!(display, original.representations[0].expect_inline_bytes());

    let completed = materializer
        .materialize_plan(recipe.plan("new-attempt"))
        .await
        .unwrap();
    assert!(!completed.is_partial());
    assert_eq!(recipe.wire, wire);
    assert_eq!(recipe.expected_hash, expected_hash);
    let (_, restored) = decode_v3_bytes_to_snapshot_and_blob_refs(&recipe.wire).unwrap();
    assert_eq!(restored, vec![source_ref]);
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].0.as_deref(), Some("old-attempt"));
    assert_eq!(calls[1].0.as_deref(), Some("new-attempt"));
    assert_eq!(calls[0].1, recipe.source.as_str());
    assert_eq!(calls[1].1, recipe.source.as_str());
    assert_eq!(
        std::fs::read(&calls[1].2).unwrap(),
        b"original-source-file-bytes"
    );
}

#[test]
fn original_positions_keep_duplicate_names_and_content_as_distinct_retry_items() {
    let refs: Vec<_> = [vec![1, 2], vec![3, 4], vec![1, 2]]
        .into_iter()
        .map(|ticket| V3BlobRef {
            ticket: BlobTicket::from_bytes(ticket),
            entry_id: EntryId::from("source"),
            filename: Some("same.bin".into()),
            mime: None,
            size_bytes: 9,
            representation_index: None,
        })
        .collect();
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: vec![ObservedClipboardRepresentation::new(
            RepresentationId::new(), FormatId::from("files"), None,
            b"file:///synthetic/a/same.bin\r\nfile:///synthetic/b/same.bin\r\nfile:///synthetic/c/same.bin\r\n".to_vec(),
        )], file_content_digests: Vec::new(), file_set_v1_component: None,
    };
    let (wire, _) = encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &refs).unwrap();
    let (_, original) = decode_v3_bytes_to_snapshot_and_blob_refs(&wire).unwrap();
    let selected: Vec<_> = original
        .into_iter()
        .enumerate()
        .filter(|(index, _)| [1, 2].contains(index))
        .collect();
    assert_eq!(selected[0], (1, refs[1].clone()));
    assert_eq!(selected[1], (2, refs[2].clone()));
    assert_eq!(refs[0], refs[2]);
    assert_ne!(selected[0].0, selected[1].0);
}
