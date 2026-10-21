use super::{test_materializer, MockBlobFetcher};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::facade::blob_transfer::FetchBlobToPathResult;
use crate::transfer::receive::materialization::{
    InboundBlobMaterializer, ReceiveReuseRefusal, ReceiveSelection, ReceiveWorkPlan, ReceivedItem,
    ReceivedOutput,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::ports::{ReceiveArtifact, ReceiveArtifactOwnership};
use uc_core::{MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot};

#[tokio::test]
async fn selecting_one_original_item_keeps_saved_same_name_file_without_fetch_or_cleanup_ownership()
{
    let cache = tempfile::tempdir().unwrap();
    let parent = cache.path().join("iroh-blobs/source");
    std::fs::create_dir_all(&parent).unwrap();
    let saved = parent.join("same.bin");
    std::fs::write(&saved, b"success").unwrap();
    let proof = ReceivedItem {
        blob_index: 0,
        plaintext_hash: *blake3::hash(b"success").as_bytes(),
        bytes: 7,
        output: ReceivedOutput::File(ReceiveArtifact {
            item_id: "old:item:0".into(),
            staged_path: saved.clone(),
            final_path: saved.clone(),
            ownership: ReceiveArtifactOwnership::ManagedStaging,
        }),
    };
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(1)
        .withf(|command| {
            command.transfer_context.as_ref().unwrap().transfer_id
                == "receiver:attempt:new:member:2"
        })
        .returning(|command| {
            std::fs::write(&command.target_path, b"retried").unwrap();
            Ok(FetchBlobToPathResult {
                entry_id: command.entry_id,
                bytes_written: 7,
                plaintext_hash: PlaintextHash::from_bytes(*blake3::hash(b"retried").as_bytes()),
                digest: BlobDigest::from_bytes([3; 32]),
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
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: vec![ObservedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("files"),
            Some(MimeType("text/uri-list".into())),
            Vec::new(),
        )],
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let result = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .materialize_plan(
            ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from("receiver"),
                snapshot,
                refs,
                None,
                Some("new".into()),
                None,
            )
            .with_selection(ReceiveSelection {
                unselected: HashMap::new(),
                selected: HashSet::from([2]),
                landed: HashMap::from([(0, proof)]),
            }),
        )
        .await
        .unwrap();
    assert!(result.is_partial());
    assert_eq!(result.missing.len(), 1);
    assert_eq!(result.snapshot.file_content_digests.len(), 2);
    assert_eq!(result.receive_artifacts.len(), 1);
    assert_eq!(
        result
            .completed
            .iter()
            .map(|item| item.blob_index)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert_ne!(result.receive_artifacts[0].final_path, saved);
    assert_eq!(std::fs::read(saved).unwrap(), b"success");
}

fn snapshot() -> SystemClipboardSnapshot {
    SystemClipboardSnapshot {
        ts_ms: 1,
        representations: vec![ObservedClipboardRepresentation::new(
            RepresentationId::new(),
            FormatId::from("files"),
            Some(MimeType("text/uri-list".into())),
            Vec::new(),
        )],
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    }
}
fn files(count: usize) -> Vec<V3BlobRef> {
    (0..count)
        .map(|_| V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![1]),
            entry_id: EntryId::from("source"),
            filename: Some("same.bin".into()),
            mime: None,
            size_bytes: 7,
            representation_index: None,
        })
        .collect()
}
fn file_proof(path: std::path::PathBuf) -> ReceivedItem {
    ReceivedItem {
        blob_index: 0,
        plaintext_hash: *blake3::hash(b"success").as_bytes(),
        bytes: 7,
        output: ReceivedOutput::File(ReceiveArtifact {
            item_id: "old:item".into(),
            staged_path: path.clone(),
            final_path: path,
            ownership: ReceiveArtifactOwnership::ManagedStaging,
        }),
    }
}

#[tokio::test]
async fn changed_missing_or_unowned_saved_files_are_rejected_before_any_fetch() {
    for scenario in 0..3 {
        let cache = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = cache.path().join("iroh-blobs/source");
        std::fs::create_dir_all(&root).unwrap();
        let path = if scenario == 2 {
            outside.path().join("saved.bin")
        } else {
            root.join("saved.bin")
        };
        if scenario != 1 {
            std::fs::write(
                &path,
                if scenario == 0 {
                    b"changed"
                } else {
                    b"success"
                },
            )
            .unwrap();
        }
        let mut fetcher = MockBlobFetcher::new();
        fetcher.expect_fetch_blob_to_path().times(0);
        fetcher.expect_fetch_blob().times(0);
        let error = test_materializer(Arc::new(fetcher), cache.path().to_owned())
            .materialize_plan(
                ReceiveWorkPlan::new(
                    DeviceId::new("peer"),
                    EntryId::from("receiver"),
                    snapshot(),
                    files(2),
                    None,
                    Some("new".into()),
                    None,
                )
                .with_selection(ReceiveSelection {
                    unselected: HashMap::new(),
                    selected: HashSet::from([1]),
                    landed: HashMap::from([(0, file_proof(path))]),
                }),
            )
            .await
            .unwrap_err();
        match scenario {
            0 => assert!(matches!(
                error.downcast_ref::<ReceiveReuseRefusal>(),
                Some(ReceiveReuseRefusal::ContentChanged)
            )),
            1 => assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::NotFound
            ),
            2 => assert!(matches!(
                error.downcast_ref::<ReceiveReuseRefusal>(),
                Some(ReceiveReuseRefusal::OwnershipChanged)
            )),
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn representations_and_files_keep_their_original_positions_when_only_later_file_is_selected()
{
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path().join("iroh-blobs/source");
    std::fs::create_dir_all(&root).unwrap();
    let saved = root.join("same.bin");
    std::fs::write(&saved, b"success").unwrap();
    let mut snapshot = snapshot();
    let image_id = RepresentationId::new();
    snapshot.representations.insert(
        0,
        ObservedClipboardRepresentation::new(
            image_id.clone(),
            FormatId::from("image"),
            Some(MimeType("image/png".into())),
            b"img!".to_vec(),
        ),
    );
    let mut refs = files(2);
    refs.insert(
        0,
        V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![2]),
            entry_id: EntryId::from("image-source"),
            filename: None,
            mime: Some("image/png".into()),
            size_bytes: 4,
            representation_index: Some(0),
        },
    );
    let mut file = file_proof(saved.clone());
    file.blob_index = 1;
    let image = ReceivedItem {
        blob_index: 0,
        plaintext_hash: *blake3::hash(b"img!").as_bytes(),
        bytes: 4,
        output: ReceivedOutput::Representation(image_id),
    };
    let mut fetcher = MockBlobFetcher::new();
    fetcher.expect_fetch_blob().times(0);
    fetcher
        .expect_fetch_blob_to_path()
        .times(1)
        .withf(|command| {
            command
                .transfer_context
                .as_ref()
                .unwrap()
                .transfer_id
                .ends_with("member:1")
        })
        .returning(|command| {
            std::fs::write(&command.target_path, b"retried").unwrap();
            Ok(FetchBlobToPathResult {
                entry_id: command.entry_id,
                bytes_written: 7,
                plaintext_hash: PlaintextHash::from_bytes(*blake3::hash(b"retried").as_bytes()),
                digest: BlobDigest::from_bytes([4; 32]),
            })
        });
    let result = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .materialize_plan(
            ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from("receiver"),
                snapshot,
                refs,
                None,
                Some("new".into()),
                None,
            )
            .with_selection(ReceiveSelection {
                unselected: HashMap::new(),
                selected: HashSet::from([2]),
                landed: HashMap::from([(0, image), (1, file)]),
            }),
        )
        .await
        .unwrap();
    assert!(!result.is_partial());
    assert_eq!(
        result
            .completed
            .iter()
            .map(|item| item.blob_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(result.receive_artifacts.len(), 1);
    assert_eq!(std::fs::read(saved).unwrap(), b"success");
    assert_eq!(
        result.snapshot.representations[0].expect_inline_bytes(),
        b"img!"
    );
}

#[tokio::test]
async fn directory_selection_rejects_member_subset_before_publication_or_fetch() {
    let cache = tempfile::tempdir().unwrap();
    let (refs, manifest, snapshot) = super::two_root_payload();
    let mut fetcher = MockBlobFetcher::new();
    fetcher.expect_fetch_blob_to_path().times(0);
    let error = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .materialize_plan(
            ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from("receiver"),
                snapshot,
                refs,
                Some(manifest),
                Some("new".into()),
                None,
            )
            .with_selection(ReceiveSelection {
                unselected: HashMap::new(),
                selected: HashSet::from([0]),
                landed: HashMap::new(),
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ReceiveReuseRefusal>(),
        Some(ReceiveReuseRefusal::DirectoryUnitRequired)
    ));
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancelled_selected_file_leaves_previous_success_outside_new_cleanup() {
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path().join("iroh-blobs/source");
    std::fs::create_dir_all(&root).unwrap();
    let saved = root.join("same.bin");
    std::fs::write(&saved, b"success").unwrap();
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(1)
        .withf(|command| {
            command
                .transfer_context
                .as_ref()
                .unwrap()
                .transfer_id
                .ends_with("member:2")
        })
        .returning(|_| Err(crate::facade::blob_transfer::BlobTransferError::Cancelled.into()));
    let result = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .materialize_plan(
            ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from("receiver"),
                snapshot(),
                files(3),
                None,
                Some("new".into()),
                None,
            )
            .with_selection(ReceiveSelection {
                unselected: HashMap::new(),
                selected: HashSet::from([2]),
                landed: HashMap::from([(0, file_proof(saved.clone()))]),
            }),
        )
        .await
        .unwrap();
    assert!(result.is_partial());
    assert!(result.receive_artifacts.is_empty());
    assert_eq!(result.completed.len(), 1);
    assert_eq!(result.completed[0].blob_index, 0);
    assert_eq!(result.missing.len(), 2);
    assert_eq!(std::fs::read(saved).unwrap(), b"success");
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_symlink_does_not_reuse_another_files_ownership() {
    let cache = tempfile::tempdir().unwrap();
    let root = cache.path().join("iroh-blobs/source");
    std::fs::create_dir_all(&root).unwrap();
    let target = root.join("user-file.bin");
    let replaced = root.join("saved.bin");
    std::fs::write(&target, b"success").unwrap();
    std::os::unix::fs::symlink(&target, &replaced).unwrap();
    let mut fetcher = MockBlobFetcher::new();
    fetcher.expect_fetch_blob_to_path().times(0);
    let error = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .materialize_plan(
            ReceiveWorkPlan::new(
                DeviceId::new("peer"),
                EntryId::from("receiver"),
                snapshot(),
                files(2),
                None,
                Some("new".into()),
                None,
            )
            .with_selection(ReceiveSelection {
                unselected: HashMap::new(),
                selected: HashSet::from([1]),
                landed: HashMap::from([(0, file_proof(replaced))]),
            }),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ReceiveReuseRefusal>(),
        Some(ReceiveReuseRefusal::OwnershipChanged)
    ));
    assert_eq!(std::fs::read(target).unwrap(), b"success");
}
