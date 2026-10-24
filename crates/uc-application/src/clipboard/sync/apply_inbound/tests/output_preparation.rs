//! 已建目标的日志失败仍须保留实际产物与原始错误，不能误作普通获取部分失败。
use super::{test_materializer, AttemptGate, FakeUserDirReserver, MockBlobFetcher};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::blob::BlobOutputPreparationError;
use crate::transfer::receive::materialization::{
    InboundBlobMaterializer, OutputCleanupFailure, ReceiveWorkPlan,
};
use async_trait::async_trait;
use std::io::{Error, ErrorKind};
use std::sync::Arc;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::{
    AttemptState, ReceiveArtifactLogError, ReceiveArtifactRecord, RecordReceiveArtifactsPort,
};
use uc_core::SystemClipboardSnapshot;

struct FailedOutputJournal {
    reject_cleanup: bool,
}
#[async_trait]
impl RecordReceiveArtifactsPort for FailedOutputJournal {
    async fn record_receive_artifacts(
        &self,
        record: &ReceiveArtifactRecord,
    ) -> Result<(), ReceiveArtifactLogError> {
        let path = &record.artifacts.last().unwrap().staged_path;
        assert!(
            path.is_file(),
            "artifact journal must follow real reservation"
        );
        if self.reject_cleanup {
            std::fs::remove_file(path).unwrap();
            std::fs::create_dir(path).unwrap();
        }
        Err(ReceiveArtifactLogError::Backend(Box::new(Error::from(
            ErrorKind::StorageFull,
        ))))
    }
}

#[tokio::test]
async fn failed_output_journal_cleans_only_reserved_target_and_retains_storage_and_cleanup_sources()
{
    for reject_cleanup in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        let saved = user.path().join("saved");
        std::fs::write(&saved, b"old success").unwrap();
        let mut fetcher = MockBlobFetcher::new();
        fetcher.expect_fetch_blob_to_path().times(0);
        let attempts = AttemptGate::new(AttemptState::Receiving, None);
        let materializer = test_materializer(Arc::new(fetcher), cache.path().to_owned())
            .with_target_reserver(Arc::new(FakeUserDirReserver {
                dir: user.path().to_owned(),
            }))
            .with_directory_receive_attempt_ports(
                attempts.clone(),
                attempts.clone(),
                attempts.clone(),
                attempts,
            )
            .with_receive_artifact_log(Arc::new(FailedOutputJournal { reject_cleanup }));
        let error = materializer
            .materialize_plan(ReceiveWorkPlan::new(
                DeviceId::new("source"),
                EntryId::from("receiver"),
                SystemClipboardSnapshot {
                    ts_ms: 1,
                    representations: Vec::new(),
                    file_content_digests: Vec::new(),
                    file_set_v1_component: None,
                },
                vec![V3BlobRef {
                    ticket: BlobTicket::from_bytes(vec![1]),
                    entry_id: EntryId::from("source"),
                    filename: Some("reserved".into()),
                    mime: None,
                    size_bytes: 5,
                    representation_index: None,
                }],
                None,
                Some("attempt-1".into()),
                None,
            ))
            .await
            .unwrap_err();
        assert!(error
            .chain()
            .any(|source| source.is::<BlobOutputPreparationError>()));
        assert_eq!(
            error
                .chain()
                .find_map(|source| source.downcast_ref::<Error>())
                .unwrap()
                .kind(),
            ErrorKind::StorageFull
        );
        if reject_cleanup {
            let cleanup = error.downcast_ref::<OutputCleanupFailure>().unwrap();
            assert_eq!(cleanup.cleanup.kind(), ErrorKind::IsADirectory);
            assert!(user.path().join("reserved").is_dir());
        } else {
            assert!(!user.path().join("reserved").exists());
        }
        assert_eq!(std::fs::read(saved).unwrap(), b"old success");
    }
}
