use super::super::entry_file_set_repo::DieselEntryFileSetRepository;
use super::*;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::models::snapshot_representation::NewSnapshotRepresentationRow;
use crate::db::models::{NewClipboardEntryRow, NewClipboardEventRow};
use crate::db::pool::init_db_pool;
use crate::db::schema::{
    clipboard_event, clipboard_snapshot_representation::dsl as representation_dsl,
    entry_file_set::dsl as file_set_dsl,
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use uc_core::clipboard::{
    ContentHash, EntryFileSet, EntryFileSetLine, EntryFileSetLineKind, FileSetMemberKind,
    FileSetMemberLocation,
};
use uc_core::crypto::domain::Plaintext;
use uc_core::ids::EntryId as CoreEntryId;
use uc_core::ids::ProfileId;
use uc_core::ports::clipboard::EntryFileSetRepositoryPort;
use uc_core::ports::security::current_profile::{CurrentProfileError, CurrentProfilePort};
use uc_core::ports::security::BlobCipherError;
use uc_core::ports::space::{DeriveSpaceSubkeyPort, SpaceAccessError};
use uc_infra_security::{BlobCipherAdapter, InMemorySession, MasterKey};

struct TestKeys;
#[async_trait::async_trait]
impl DeriveSpaceSubkeyPort for TestKeys {
    async fn derive_subkey(&self, _: &[u8], _: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        Ok([7; 32])
    }
}
#[async_trait::async_trait]
impl CurrentProfilePort for TestKeys {
    async fn current_profile(&self) -> Result<ProfileId, CurrentProfileError> {
        Ok(ProfileId::from("default"))
    }
}

#[tokio::test]
async fn missing_placeholder_makes_file_list_unavailable() {
    let body = b"file:///tmp/a.txt\r\nuniclip-missing:///b.iso?size=10&reason=cancelled";
    assert!(!file_list_is_fully_present(body, None).await);
}

#[tokio::test]
async fn nonexistent_file_uri_is_unavailable() {
    let body = b"file:///definitely/not/here/zzz-uniclip-test.bin";
    assert!(!file_list_is_fully_present(body, None).await);
}

#[tokio::test]
async fn comments_and_blank_lines_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("present.txt");
    std::fs::write(&path, b"hi").unwrap();
    let uri = url::Url::from_file_path(&path).unwrap();
    let body = format!("# a comment\r\n\r\n{uri}\r\n");
    assert!(file_list_is_fully_present(body.as_bytes(), None).await);
}

#[tokio::test]
async fn existing_regular_file_is_available() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.bin");
    std::fs::write(&path, b"bytes").unwrap();
    let uri = url::Url::from_file_path(&path).unwrap();
    assert!(file_list_is_fully_present(uri.as_str().as_bytes(), None).await);
}

#[tokio::test]
async fn directory_target_is_not_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let uri = url::Url::from_file_path(dir.path()).unwrap();
    assert!(!file_list_is_fully_present(uri.as_str().as_bytes(), None).await);
}

#[tokio::test]
async fn failed_payload_state_is_unavailable() {
    let reps = vec![(false, None, "Failed".to_string())];
    assert!(!reps_indicate_available(&reps, None).await);
}

#[tokio::test]
async fn inline_text_entry_is_available() {
    let reps = vec![(false, Some(b"hello".to_vec()), "Inline".to_string())];
    assert!(reps_indicate_available(&reps, None).await);
}

#[tokio::test]
async fn empty_rep_set_is_unavailable() {
    assert!(!reps_indicate_available(&[], None).await);
}

// 使用真实SQLite、加密表示和实际文件系统贯通条目→事件→表示。

fn make_repo() -> (
    DieselEntryAvailabilityRepository<Arc<DieselSqliteExecutor>>,
    Arc<DieselSqliteExecutor>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("availability.sqlite");
    let pool = init_db_pool(path.to_str().unwrap()).unwrap();
    let executor = Arc::new(DieselSqliteExecutor::new(pool));
    let session = Arc::new(InMemorySession::new());
    session.set_master_key_for_space(
        "availability".into(),
        MasterKey::from_bytes(&[8; 32]).unwrap(),
    );
    let repo = DieselEntryAvailabilityRepository::new(
        Arc::clone(&executor),
        Arc::new(BlobCipherAdapter::new(session)),
        Arc::new(DieselEntryFileSetRepository::new(
            Arc::clone(&executor),
            Arc::new(TestKeys),
            Arc::new(TestKeys),
        )),
    );
    (repo, executor, dir)
}

async fn seed_uri_list_entry(
    repo: &DieselEntryAvailabilityRepository<Arc<DieselSqliteExecutor>>,
    executor: &Arc<DieselSqliteExecutor>,
    uri_list_body: &str,
) {
    let body_size = uri_list_body.len() as i64;
    let context = aad::for_inline(&EventId::from("ev"), &RepresentationId::from("rep"));
    let body = repo
        .cipher
        .encrypt(
            &Plaintext::new(uri_list_body.as_bytes().to_vec()),
            &Aad::from(context.as_slice()),
        )
        .await
        .unwrap()
        .into_bytes();
    executor
        .run(move |conn| {
            diesel::insert_into(clipboard_event::table)
                .values(&NewClipboardEventRow {
                    event_id: "ev".into(),
                    captured_at_ms: 0,
                    source_device: "dev".into(),
                    snapshot_hash: "blake3v1:00".into(),
                })
                .execute(conn)?;
            diesel::insert_into(clipboard_entry::table)
                .values(&NewClipboardEntryRow {
                    entry_id: "e1".into(),
                    event_id: "ev".into(),
                    created_at_ms: 0,
                    active_time_ms: 0,
                    total_size: 0,
                    pinned: false,
                    delivery_tracked: true,
                    is_favorited: false,
                    content_category: "file".into(),
                })
                .execute(conn)?;
            diesel::insert_into(clipboard_snapshot_representation::table)
                .values(&NewSnapshotRepresentationRow {
                    id: "rep".into(),
                    event_id: "ev".into(),
                    format_id: "files".into(),
                    mime_type: Some("text/uri-list".into()),
                    size_bytes: body_size,
                    inline_data: Some(body),
                    blob_id: None,
                    payload_state: "Inline".into(),
                    last_error: None,
                })
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn file_entry_flips_to_unavailable_when_local_file_disappears() {
    let (repo, executor, dir) = make_repo();
    let file = dir.path().join("payload.bin");
    std::fs::write(&file, b"bytes").unwrap();
    let uri = url::Url::from_file_path(&file).unwrap();
    seed_uri_list_entry(&repo, &executor, uri.as_str()).await;

    assert!(
        repo.is_entry_available(&CoreEntryId::from("e1"))
            .await
            .unwrap(),
        "present local file ⇒ available"
    );

    std::fs::remove_file(&file).unwrap();
    assert!(
        !repo
            .is_entry_available(&CoreEntryId::from("e1"))
            .await
            .unwrap(),
        "removed local file ⇒ not available (must not converge stale content)"
    );
}

#[tokio::test]
async fn complete_directory_manifest_is_available() {
    let (repo, _, _dir, _root) = seed_directory_entry().await;
    assert!(repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
}

async fn seed_directory_entry() -> (
    DieselEntryAvailabilityRepository<Arc<DieselSqliteExecutor>>,
    Arc<DieselSqliteExecutor>,
    tempfile::TempDir,
    PathBuf,
) {
    let (repo, executor, dir) = make_repo();
    let root = dir.path().join("tree");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::write(root.join("one.txt"), b"one").unwrap();
    std::fs::write(root.join("nested/two.txt"), b"two").unwrap();
    let uri = url::Url::from_file_path(&root).unwrap().to_string();
    seed_uri_list_entry(&repo, &executor, &uri).await;
    let files =
        DieselEntryFileSetRepository::new(executor.clone(), Arc::new(TestKeys), Arc::new(TestKeys));
    let mut lines = Vec::new();
    for (index, (relative, content)) in [
        ("one.txt", Some(b"one".as_slice())),
        ("nested/two.txt", Some(b"two".as_slice())),
        ("empty", None),
    ]
    .into_iter()
    .enumerate()
    {
        lines.push(EntryFileSetLine {
            line_index: index as i64 + 1,
            original_text: uri.clone(),
            member_location: Some(FileSetMemberLocation {
                root_index: 0,
                root_name: "tree".into(),
                relative_path: relative.into(),
                kind: if content.is_some() {
                    FileSetMemberKind::File
                } else {
                    FileSetMemberKind::EmptyDirectory
                },
            }),
            kind: match content {
                Some(bytes) => EntryFileSetLineKind::File {
                    content_hash: ContentHash::from(blake3::hash(bytes).as_bytes()),
                    blob_id: None,
                    size_bytes: Some(bytes.len() as i64),
                },
                None => EntryFileSetLineKind::NonFile,
            },
        });
    }
    files
        .save(&EntryId::from("e1"), &EntryFileSet { lines })
        .await
        .unwrap();
    assert!(
        repo.is_entry_available(&EntryId::from("e1")).await.unwrap(),
        "完整目录必须通过真实SQLite和已加密清单的可用性检查"
    );
    (repo, executor, dir, root)
}

#[tokio::test]
async fn directory_availability_rechecks_members_structure_and_content() {
    for damage in [
        "missing_file",
        "changed_content",
        "missing_empty",
        "extra",
        "file_to_directory",
        "root_to_file",
    ] {
        let (repo, _executor, _dir, root) = seed_directory_entry().await;
        match damage {
            "missing_file" => std::fs::remove_file(root.join("nested/two.txt")).unwrap(),
            "changed_content" => std::fs::write(root.join("nested/two.txt"), b"bad").unwrap(),
            "missing_empty" => std::fs::remove_dir(root.join("empty")).unwrap(),
            "extra" => std::fs::write(root.join("extra.txt"), b"extra").unwrap(),
            "file_to_directory" => {
                std::fs::remove_file(root.join("one.txt")).unwrap();
                std::fs::create_dir(root.join("one.txt")).unwrap();
            }
            _ => {
                std::fs::remove_dir_all(&root).unwrap();
                std::fs::write(&root, b"one").unwrap();
            }
        }
        assert!(
            !repo.is_entry_available(&EntryId::from("e1")).await.unwrap(),
            "{damage}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn directory_availability_rejects_root_and_nested_symlinks() {
    for nested in [false, true] {
        let (repo, _executor, dir, root) = seed_directory_entry().await;
        if nested {
            let nested = root.join("nested");
            let target = dir.path().join("moved-nested");
            std::fs::rename(&nested, &target).unwrap();
            std::os::unix::fs::symlink(&target, &nested).unwrap();
        } else {
            let target = dir.path().join("moved-root");
            std::fs::rename(&root, &target).unwrap();
            std::os::unix::fs::symlink(&target, &root).unwrap();
        }
        assert!(!repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
    }
}

#[tokio::test]
async fn partial_placeholder_entry_is_unavailable() {
    let (repo, executor, _dir) = make_repo();
    seed_uri_list_entry(
        &repo,
        &executor,
        "uniclip-missing:///big.iso?size=950000000&reason=cancelled",
    )
    .await;
    assert!(
        !repo
            .is_entry_available(&CoreEntryId::from("e1"))
            .await
            .unwrap(),
        "uniclip-missing placeholder ⇒ partial ⇒ not available"
    );
}

#[tokio::test]
async fn absent_entry_is_unavailable() {
    let (repo, _executor, _dir) = make_repo();
    assert!(!repo
        .is_entry_available(&CoreEntryId::from("nope"))
        .await
        .unwrap());
}
#[tokio::test]
async fn file_availability_rejects_corrupt_ciphertext_plaintext_and_wrong_aad_with_source() {
    for damage in ["corrupt", "plaintext", "identity"] {
        let (repo, executor, dir) = make_repo();
        let file = dir.path().join("auth.bin");
        std::fs::write(&file, b"content").unwrap();
        let uri = url::Url::from_file_path(&file).unwrap();
        seed_uri_list_entry(&repo, &executor, uri.as_str()).await;
        assert!(repo
            .is_entry_available(&CoreEntryId::from("e1"))
            .await
            .unwrap());
        executor
            .run(|conn| {
                match damage {
                    "identity" => {
                        diesel::update(representation_dsl::clipboard_snapshot_representation)
                            .set(representation_dsl::id.eq("different-representation"))
                            .execute(conn)?
                    }
                    "plaintext" => {
                        diesel::update(representation_dsl::clipboard_snapshot_representation)
                            .set(
                                representation_dsl::inline_data
                                    .eq(Some(uri.as_str().as_bytes().to_vec())),
                            )
                            .execute(conn)?
                    }
                    _ => diesel::update(representation_dsl::clipboard_snapshot_representation)
                        .set(representation_dsl::inline_data.eq(Some(vec![0])))
                        .execute(conn)?,
                };
                Ok(())
            })
            .unwrap();
        let error = repo
            .is_entry_available(&CoreEntryId::from("e1"))
            .await
            .unwrap_err();
        let error = SourceError::new(error);
        assert!(error
            .chain()
            .any(|cause| cause.downcast_ref::<BlobCipherError>().is_some()));
        assert!(file.is_file());
    }
}

#[tokio::test]
async fn directory_availability_authenticates_manifest_and_rejects_unsafe_members() {
    for damage in ["ciphertext", "parent", "absolute"] {
        let (repo, executor, _dir, _root) = seed_directory_entry().await;
        if damage == "ciphertext" {
            executor
                .run(|conn| {
                    diesel::update(file_set_dsl::entry_file_set)
                        .set(file_set_dsl::original_text_ct.eq(Some(vec![0u8])))
                        .execute(conn)?;
                    Ok(())
                })
                .unwrap();
            let error = repo
                .is_entry_available(&EntryId::from("e1"))
                .await
                .unwrap_err();
            assert!(std::error::Error::source(&error).is_some());
        } else {
            let mut files = repo
                .file_sets
                .load(&EntryId::from("e1"))
                .await
                .unwrap()
                .unwrap();
            files.lines[0]
                .member_location
                .as_mut()
                .unwrap()
                .relative_path = if damage == "parent" {
                "../one.txt"
            } else {
                "/one.txt"
            }
            .into();
            repo.file_sets
                .save(&EntryId::from("e1"), &files)
                .await
                .unwrap();
            assert!(!repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
        }
    }
}

#[tokio::test]
async fn empty_directory_root_requires_its_manifest_and_stays_empty() {
    let (repo, executor, dir) = make_repo();
    let root = dir.path().join("empty-root");
    std::fs::create_dir(&root).unwrap();
    let uri = url::Url::from_file_path(&root).unwrap();
    seed_uri_list_entry(&repo, &executor, uri.as_str()).await;
    assert!(!repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
    repo.file_sets
        .save(
            &EntryId::from("e1"),
            &EntryFileSet {
                lines: vec![EntryFileSetLine {
                    line_index: 1,
                    original_text: root.to_str().unwrap().into(),
                    member_location: Some(FileSetMemberLocation {
                        root_index: 0,
                        root_name: "empty-root".into(),
                        relative_path: ".".into(),
                        kind: FileSetMemberKind::EmptyDirectory,
                    }),
                    kind: EntryFileSetLineKind::NonFile,
                }],
            },
        )
        .await
        .unwrap();
    assert!(repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
    std::fs::write(root.join("unexpected"), b"extra").unwrap();
    assert!(!repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
}

#[tokio::test]
async fn legacy_root_identity_collisions_are_not_safe_to_reuse() {
    for collision in ["kind", "grouping", "safe"] {
        let (repo, executor, dir) = make_repo();
        let mut lines = Vec::new();
        let mut uris = Vec::new();
        let roots: [(i64, &str, &str); 2] = match collision {
            "kind" => [(0, "same", "same"), (1, "other", "child")],
            "grouping" => [(0, "same", "a"), (1, "same", "b")],
            _ => [(0, "same", "same"), (0, "same", "child")],
        };
        for (index, (root_index, root_name, relative)) in roots.into_iter().enumerate() {
            let root = dir.path().join(root_index.to_string()).join(root_name);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join(relative), b"content").unwrap();
            let uri = url::Url::from_file_path(&root).unwrap().to_string();
            uris.push(uri.clone());
            lines.push(EntryFileSetLine {
                line_index: 2 + index as i64,
                original_text: uri,
                member_location: Some(FileSetMemberLocation {
                    root_index,
                    root_name: root_name.into(),
                    relative_path: relative.into(),
                    kind: FileSetMemberKind::File,
                }),
                kind: EntryFileSetLineKind::File {
                    content_hash: ContentHash::from(blake3::hash(b"content").as_bytes()),
                    blob_id: None,
                    size_bytes: Some(7),
                },
            });
        }
        let directories = EntryFileSet { lines };
        let mut different_tree = directories.clone();
        if collision == "kind" {
            different_tree.lines[0].line_index = 0;
            assert!(directories.lines[0].indicates_directory_root(2));
            assert!(!different_tree.lines[0].indicates_directory_root(2));
        } else if collision == "grouping" {
            different_tree.lines[1]
                .member_location
                .as_mut()
                .unwrap()
                .root_index = 0;
        }
        if collision != "safe" {
            assert_eq!(
                directories.file_set_v1_component(),
                different_tree.file_set_v1_component(),
                "旧v1身份无法区分根类型或同名根的分组，不能仅凭摘要复用不同树"
            );
        }
        uris.dedup();
        seed_uri_list_entry(&repo, &executor, &uris.join("\r\n")).await;
        repo.file_sets
            .save(&EntryId::from("e1"), &directories)
            .await
            .unwrap();
        assert_eq!(
            repo.is_entry_available(&EntryId::from("e1")).await.unwrap(),
            collision == "safe",
            "{collision}: 实际成员齐全，也必须能由既有身份区分根结构才可复用"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn directory_availability_rechecks_executable_identity() {
    let (repo, _executor, _dir, root) = seed_directory_entry().await;
    let entry = EntryId::from("e1");
    let mut files = repo.file_sets.load(&entry).await.unwrap().unwrap();
    files.lines[0].member_location.as_mut().unwrap().kind = FileSetMemberKind::Executable;
    repo.file_sets.save(&entry, &files).await.unwrap();
    assert!(
        !repo.is_entry_available(&entry).await.unwrap(),
        "期望可执行成员时不可复用丢失执行位的文件"
    );
    let file = root.join("one.txt");
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_mode(permissions.mode() | 0o100);
    std::fs::set_permissions(&file, permissions.clone()).unwrap();
    assert!(repo.is_entry_available(&entry).await.unwrap());
    files.lines[0].member_location.as_mut().unwrap().kind = FileSetMemberKind::File;
    repo.file_sets.save(&entry, &files).await.unwrap();
    assert!(
        !repo.is_entry_available(&entry).await.unwrap(),
        "执行位也是ADR-010的内容身份组成"
    );
    permissions.set_mode(permissions.mode() & !0o111);
    std::fs::set_permissions(&file, permissions).unwrap();
    assert!(repo.is_entry_available(&entry).await.unwrap());
}

#[tokio::test]
async fn directory_availability_matches_normalized_manifest_to_real_member_names() {
    let (repo, executor, dir) = make_repo();
    let root = dir.path().join("tree");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("cafe\u{301}.txt"), b"bytes").unwrap();
    let uri = url::Url::from_file_path(&root).unwrap().to_string();
    seed_uri_list_entry(&repo, &executor, &uri).await;
    repo.file_sets
        .save(
            &EntryId::from("e1"),
            &EntryFileSet {
                lines: vec![EntryFileSetLine {
                    line_index: 1,
                    original_text: uri,
                    member_location: Some(FileSetMemberLocation {
                        root_index: 0,
                        root_name: "tree".into(),
                        relative_path: "caf\u{e9}.txt".into(),
                        kind: FileSetMemberKind::File,
                    }),
                    kind: EntryFileSetLineKind::File {
                        content_hash: ContentHash::from(blake3::hash(b"bytes").as_bytes()),
                        blob_id: None,
                        size_bytes: Some(5),
                    },
                }],
            },
        )
        .await
        .unwrap();
    assert!(
        repo.is_entry_available(&EntryId::from("e1")).await.unwrap(),
        "清单按NFC记录，真实文件系统保留NFD时仍须准确读回成员"
    );
}

#[tokio::test]
async fn file_formats_without_mime_still_require_local_content() {
    for format in ["files", "public.file-url", "custom-uri-list"] {
        for directory in [false, true] {
            let (repo, executor, _dir, removed) = if directory {
                let (repo, executor, dir, root) = seed_directory_entry().await;
                (repo, executor, dir, root.join("nested/two.txt"))
            } else {
                let (repo, executor, dir) = make_repo();
                let file = dir.path().join("held.bin");
                std::fs::write(&file, b"bytes").unwrap();
                let uri = url::Url::from_file_path(&file).unwrap();
                seed_uri_list_entry(&repo, &executor, uri.as_str()).await;
                (repo, executor, dir, file)
            };
            executor
                .run(move |conn| {
                    diesel::update(representation_dsl::clipboard_snapshot_representation)
                        .set((
                            representation_dsl::format_id.eq(format),
                            representation_dsl::mime_type.eq(None::<String>),
                        ))
                        .execute(conn)?;
                    Ok(())
                })
                .unwrap();
            assert!(repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
            std::fs::remove_file(&removed).unwrap();
            assert!(
                !repo.is_entry_available(&EntryId::from("e1")).await.unwrap(),
                "{format}: 省略MIME不应跳过Core已认定的文件内容检查"
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn replaced_regular_file_symlink_is_not_available() {
    let (repo, executor, dir) = make_repo();
    let file = dir.path().join("held.bin");
    let other = dir.path().join("other.bin");
    std::fs::write(&file, b"bytes").unwrap();
    std::fs::write(&other, b"bytes").unwrap();
    let uri = url::Url::from_file_path(&file).unwrap();
    seed_uri_list_entry(&repo, &executor, uri.as_str()).await;
    assert!(repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&other, &file).unwrap();
    assert!(!repo.is_entry_available(&EntryId::from("e1")).await.unwrap());
}
