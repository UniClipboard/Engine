//! 捕获既有的根空目录标记必须能物化，其他点路径和穿越仍拒绝。
use super::{test_materializer, MockBlobFetcher};
use crate::transfer::receive::materialization::{
    InboundBlobMaterializer, InboundFileSetManifest, InboundFileSetMember, ReceiveWorkPlan,
};
use std::sync::Arc;
use uc_core::clipboard::FileSetMemberKind;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::SystemClipboardSnapshot;

#[tokio::test]
async fn directory_empty_root_marker_materializes_selected_root_without_fetching() {
    let root = tempfile::tempdir().unwrap();
    let materializer =
        test_materializer(Arc::new(MockBlobFetcher::new()), root.path().to_path_buf());
    let plan = plan(FileSetMemberKind::EmptyDirectory, ".", false, None);
    let result = materializer.materialize_plan(plan).await.unwrap();
    let published = root.path().join("iroh-blobs/empty-root-receiver/folder");
    assert!(published.is_dir());
    assert_eq!(std::fs::read_dir(&published).unwrap().count(), 0);
    let file_set = result.directory_file_set.as_ref().unwrap();
    assert_eq!(file_set.lines.len(), 1);
    let location = file_set.lines[0].member_location.as_ref().unwrap();
    assert_eq!(location.kind, FileSetMemberKind::EmptyDirectory);
    assert_eq!(location.relative_path, ".");
}

#[tokio::test]
async fn directory_empty_root_marker_does_not_admit_file_dot_empty_nested_or_parent_paths() {
    for (kind, path, root_is_file, blob) in [
        (FileSetMemberKind::File, ".", false, None),
        (FileSetMemberKind::Executable, ".", false, None),
        (FileSetMemberKind::EmptyDirectory, "", false, None),
        (FileSetMemberKind::EmptyDirectory, "a/.", false, None),
        (FileSetMemberKind::EmptyDirectory, "../escape", false, None),
        (FileSetMemberKind::EmptyDirectory, ".", true, None),
        (FileSetMemberKind::EmptyDirectory, ".", false, Some(0)),
    ] {
        let root = tempfile::tempdir().unwrap();
        let materializer =
            test_materializer(Arc::new(MockBlobFetcher::new()), root.path().to_path_buf());
        assert!(materializer
            .materialize_plan(plan(kind, path, root_is_file, blob))
            .await
            .is_err());
        assert!(!root
            .path()
            .join("iroh-blobs/empty-root-receiver/folder")
            .exists());
        assert!(!root.path().join("escape").exists());
    }
}

#[tokio::test]
async fn directory_empty_root_marker_cannot_also_declare_nested_members() {
    let root = tempfile::tempdir().unwrap();
    let materializer =
        test_materializer(Arc::new(MockBlobFetcher::new()), root.path().to_path_buf());
    let mut work = plan(FileSetMemberKind::EmptyDirectory, ".", false, None);
    let manifest = work.file_set_manifest.as_mut().unwrap();
    let mut nested = manifest.members[0].clone();
    nested.relative_path = "child".into();
    manifest.members.push(nested);
    assert!(materializer.materialize_plan(work).await.is_err());
    assert!(!root
        .path()
        .join("iroh-blobs/empty-root-receiver/folder")
        .exists());
}

fn plan(
    kind: FileSetMemberKind,
    path: &str,
    root_is_file: bool,
    blob_ref_index: Option<u32>,
) -> ReceiveWorkPlan {
    ReceiveWorkPlan::new(
        DeviceId::new("source"),
        EntryId::from("empty-root-receiver"),
        SystemClipboardSnapshot {
            ts_ms: 1,
            representations: Vec::new(),
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        },
        Vec::new(),
        Some(InboundFileSetManifest {
            members: vec![InboundFileSetMember {
                root_index: 0,
                root_name: "folder".into(),
                root_is_file,
                relative_path: path.into(),
                kind,
                blob_ref_index,
            }],
        }),
        None,
        None,
    )
}
