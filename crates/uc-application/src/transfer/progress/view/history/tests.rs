//! 未持久化位置的表示和混合范围不能伪造成完整原始计划。
use super::original_items;
use crate::transfer::progress::{ProgressHistoryEvidence, ProgressHistoryRepresentation};
use uc_core::clipboard::{
    EntryFileSet, EntryFileSetLine, EntryFileSetLineKind, FileSetMemberKind, FileSetMemberLocation,
    MimeType,
};
use uc_core::file_transfer::{ProgressItem, ProgressItemKind};

fn image(size_bytes: i64, sole_paste: bool) -> ProgressHistoryEvidence {
    ProgressHistoryEvidence {
        file_set: None,
        representations: vec![ProgressHistoryRepresentation {
            mime_type: Some(MimeType("image/png".into())),
            size_bytes,
        }],
        sole_paste,
    }
}
#[test]
fn historical_image_requires_provable_single_original_position_and_large_payload() {
    assert_eq!(
        original_items(image(70_000, true)),
        Some(vec![ProgressItem {
            kind: ProgressItemKind::Representation,
            index: 0
        }])
    );
    assert!(original_items(image(70_000, false)).is_none());
    assert!(original_items(image(64 * 1024, true)).is_none());
    assert!(original_items(image(-1, true)).is_none());
    let mut ambiguous = image(70_000, true);
    ambiguous
        .representations
        .push(ProgressHistoryRepresentation {
            mime_type: Some(MimeType("text/plain".into())),
            size_bytes: 4,
        });
    assert!(original_items(ambiguous).is_none());
}
fn empty_directory() -> EntryFileSet {
    EntryFileSet {
        lines: vec![EntryFileSetLine {
            line_index: 0,
            original_text: "file:///root".into(),
            member_location: Some(FileSetMemberLocation {
                root_index: 0,
                root_name: "root".into(),
                relative_path: "".into(),
                kind: FileSetMemberKind::EmptyDirectory,
            }),
            kind: EntryFileSetLineKind::NonFile,
        }],
    }
}
#[test]
fn historical_empty_directory_is_one_complete_unit_and_mixed_image_scope_is_never_shrunk() {
    assert_eq!(
        original_items(ProgressHistoryEvidence {
            file_set: Some(empty_directory()),
            representations: Vec::new(),
            sole_paste: false
        }),
        Some(vec![ProgressItem {
            kind: ProgressItemKind::DirectoryUnit,
            index: 0
        }])
    );
    let mut mixed = image(70_000, false);
    mixed.file_set = Some(empty_directory());
    assert!(original_items(mixed).is_none());
    assert!(original_items(ProgressHistoryEvidence {
        file_set: Some(EntryFileSet { lines: Vec::new() }),
        representations: Vec::new(),
        sole_paste: false
    })
    .is_none());
}
