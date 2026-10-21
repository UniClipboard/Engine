use super::{test_materializer, MockBlobFetcher};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::facade::blob_transfer::{BlobTransferError, FetchBlobToPathResult};
use crate::transfer::receive::materialization::{
    InboundBlobMaterializer, MaterializeOutcome, ReceiveWorkPlan,
};
use std::io;
use std::sync::{Arc, Mutex};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::{
    FileDisplayMetadata, MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot,
    FILE_DISPLAY_METADATA_FORMAT,
};

#[tokio::test]
async fn failed_or_individually_cancelled_files_do_not_skip_later_successes() {
    for failure_first in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let mut fetcher = MockBlobFetcher::new();
        fetcher
            .expect_fetch_blob_to_path()
            .times(3)
            .returning(move |command| {
                let mut calls = recorded.lock().unwrap();
                let index = calls.len();
                calls.push(
                    command
                        .transfer_context
                        .as_ref()
                        .unwrap()
                        .transfer_id
                        .clone(),
                );
                assert!(!command.cancellation.is_cancelled());
                if index == 0 && failure_first {
                    return Err(io::Error::from(io::ErrorKind::PermissionDenied).into());
                }
                if index == 0 || (index == 1 && failure_first) {
                    return Err(BlobTransferError::Cancelled.into());
                }
                std::fs::write(&command.target_path, b"success").unwrap();
                Ok(FetchBlobToPathResult {
                    entry_id: command.entry_id,
                    bytes_written: 7,
                    plaintext_hash: PlaintextHash::from_bytes([3; 32]),
                    digest: BlobDigest::from_bytes([4; 32]),
                })
            });
        let refs = (0..3)
            .map(|_| V3BlobRef {
                ticket: BlobTicket::from_bytes(vec![1]),
                entry_id: EntryId::from("source"),
                filename: Some("same.bin".into()),
                mime: None,
                size_bytes: 7,
                representation_index: None,
            })
            .collect();
        let snapshot = SystemClipboardSnapshot { ts_ms: 1, representations: vec![ObservedClipboardRepresentation::new(RepresentationId::new(), FormatId::from("files"), Some(MimeType("text/uri-list".into())), b"file:///source/a/same.bin\r\nfile:///source/b/same.bin\r\nfile:///source/c/same.bin\r\n".to_vec())], file_content_digests: Vec::new(), file_set_v1_component: None };
        let result = test_materializer(Arc::new(fetcher), cache.path().to_owned())
            .materialize_plan(ReceiveWorkPlan::new(
                DeviceId::new("source"),
                EntryId::from("receiver"),
                snapshot,
                refs,
                None,
                Some("attempt".into()),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(
            result.outcome(),
            if failure_first {
                MaterializeOutcome::PartialFailed
            } else {
                MaterializeOutcome::PartialCancelled
            }
        );
        assert_eq!(result.missing.len(), if failure_first { 2 } else { 1 });
        assert_eq!(
            result.receive_artifacts.len(),
            if failure_first { 1 } else { 2 }
        );
        assert_eq!(
            result.snapshot.file_content_digests.len(),
            result.receive_artifacts.len()
        );
        for artifact in &result.receive_artifacts {
            assert_eq!(std::fs::read(&artifact.final_path).unwrap(), b"success");
        }
        let metadata = result
            .snapshot
            .representations
            .iter()
            .find(|rep| rep.format_id.as_str() == FILE_DISPLAY_METADATA_FORMAT)
            .unwrap();
        let metadata = FileDisplayMetadata::decode(metadata.expect_inline_bytes()).unwrap();
        assert_eq!(metadata.files.len(), result.receive_artifacts.len());
        for (metadata, artifact) in metadata.files.iter().zip(&result.receive_artifacts) {
            assert_eq!(
                metadata.storage_name,
                artifact.final_path.file_name().unwrap().to_str().unwrap()
            );
            assert_eq!(metadata.display_name, "same.bin");
        }
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        for (index, id) in calls.iter().enumerate() {
            assert_eq!(id, &format!("receiver:attempt:attempt:member:{index}"));
        }
    }
}
