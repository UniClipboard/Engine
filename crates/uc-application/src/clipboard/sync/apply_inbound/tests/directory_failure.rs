use super::{directory_materializer, two_root_payload, FakeAtomicPublisher, MockBlobFetcher};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::facade::blob_transfer::BlobTransferError;
use crate::transfer::receive::materialization::{InboundBlobMaterializer, ReceiveWorkPlan};
use std::io;
use std::sync::Arc;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::BlobTicket;

#[derive(Debug, thiserror::Error)]
#[error("representation fetch boundary")]
struct RepresentationFetchBoundary;

#[tokio::test]
async fn directory_representation_failure_preserves_source_and_never_starts_members() {
    for cancelled in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let publisher = Arc::new(FakeAtomicPublisher::default());
        let (mut refs, manifest, snapshot) = two_root_payload();
        refs.push(V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![9]),
            entry_id: EntryId::from("source-representation"),
            filename: None,
            mime: None,
            size_bytes: 4,
            representation_index: Some(0),
        });
        let mut fetcher = MockBlobFetcher::new();
        fetcher.expect_fetch_blob().times(1).returning(move |_| {
            let failure = if cancelled {
                BlobTransferError::Cancelled
            } else {
                BlobTransferError::Fetch(io::Error::from(io::ErrorKind::ConnectionReset).into())
            };
            Err(anyhow::Error::new(failure).context(RepresentationFetchBoundary))
        });
        fetcher.expect_fetch_blob_to_path().times(0);
        fetcher.expect_record_unfetched_failure().times(0);
        let materializer =
            directory_materializer(fetcher, cache.path(), destination.path(), publisher.clone());
        let error = materializer
            .materialize_plan(ReceiveWorkPlan::new(
                DeviceId::new("source"),
                EntryId::from("receiver"),
                snapshot,
                refs,
                Some(manifest),
                Some("attempt".into()),
                None,
            ))
            .await
            .expect_err("directory representation must settle as a failure");

        assert!(error
            .downcast_ref::<RepresentationFetchBoundary>()
            .is_some());
        assert_eq!(
            error.chain().any(|source| matches!(
                source.downcast_ref::<BlobTransferError>(),
                Some(BlobTransferError::Cancelled)
            )),
            cancelled
        );
        if !cancelled {
            assert_eq!(
                error
                    .chain()
                    .find_map(|source| source.downcast_ref::<io::Error>())
                    .unwrap()
                    .kind(),
                io::ErrorKind::ConnectionReset
            );
        }
        assert_eq!(publisher.publish_count(), 0);
        assert_eq!(std::fs::read_dir(destination.path()).unwrap().count(), 0);
    }
}
