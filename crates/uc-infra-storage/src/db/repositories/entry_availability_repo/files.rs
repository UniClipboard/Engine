//! 文件可用性只读核对；目录必须有已认证清单，不能以根目录存在代替成员完整。
use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::AsyncReadExt;

use uc_core::clipboard::{EntryFileSet, EntryFileSetLineKind, FileSetMemberKind};
use unicode_normalization::UnicodeNormalization;

use super::file_uri_to_path;

pub(super) async fn local_path_is_available(path: &Path, file_set: Option<&EntryFileSet>) -> bool {
    let rows: Vec<_> = file_set
        .into_iter()
        .flat_map(|set| &set.lines)
        .filter(|line| file_uri_to_path(&line.original_text).as_deref() == Some(path))
        .collect();
    let row_count = file_set.map_or(0, |set| set.lines.len() as i64);
    let directory = rows
        .iter()
        .any(|line| line.indicates_directory_root(row_count));
    if !directory {
        // 先排除链接及特殊文件，不主动打开FIFO等不支持的目标。
        let Ok(metadata) = fs::symlink_metadata(path).await else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        let Ok(file) = File::open(path).await else {
            return false;
        };
        return file
            .metadata()
            .await
            .is_ok_and(|metadata| metadata.is_file());
    }
    let Ok(metadata) = fs::symlink_metadata(path).await else {
        return false;
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    let mut expected = BTreeSet::new();
    for row in &rows {
        let Some(location) = &row.member_location else {
            return false;
        };
        let relative = Path::new(&location.relative_path);
        let empty_root =
            location.kind == FileSetMemberKind::EmptyDirectory && location.relative_path == ".";
        if empty_root {
            if rows.len() != 1 {
                return false;
            }
            continue;
        }
        if location.relative_path.contains('\\')
            || location
                .relative_path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return false;
        }
        let mut ancestor = relative;
        while !ancestor.as_os_str().is_empty() {
            expected.insert(ancestor.to_path_buf());
            let Some(parent) = ancestor.parent() else {
                return false;
            };
            ancestor = parent;
        }
    }
    // 先核对整棵树，拒绝额外成员、符号链接和特殊文件后才打开文件读回。
    let Some(members) = read_tree_members(path, expected).await else {
        return false;
    };
    for row in rows {
        let Some(location) = &row.member_location else {
            return false;
        };
        let member = if location.relative_path == "." {
            path.to_path_buf()
        } else {
            let Some(actual) = members.get(Path::new(&location.relative_path)) else {
                return false;
            };
            path.join(actual)
        };
        match (&row.kind, location.kind) {
            (
                EntryFileSetLineKind::File {
                    content_hash,
                    size_bytes,
                    ..
                },
                FileSetMemberKind::File | FileSetMemberKind::Executable,
            ) => {
                let Ok(mut file) = File::open(&member).await else {
                    return false;
                };
                let Ok(metadata) = file.metadata().await else {
                    return false;
                };
                if !metadata.is_file()
                    || size_bytes.is_some_and(|size| size < 0 || metadata.len() != size as u64)
                {
                    return false;
                }
                #[cfg(unix)]
                if (metadata.permissions().mode() & 0o111 != 0)
                    != (location.kind == FileSetMemberKind::Executable)
                {
                    return false;
                }
                let mut hasher = blake3::Hasher::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let Ok(read) = file.read(&mut buffer).await else {
                        return false;
                    };
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
                if hasher.finalize().as_bytes() != &content_hash.bytes {
                    return false;
                }
            }
            (EntryFileSetLineKind::NonFile, FileSetMemberKind::EmptyDirectory) => {
                let Ok(mut children) = fs::read_dir(&member).await else {
                    return false;
                };
                if !matches!(children.next_entry().await, Ok(None)) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

/// 名称按既有NFC身份比较，读回仍用文件系统给出的实际名称；规范化重名不合并。
async fn read_tree_members(
    root: &Path,
    mut expected: BTreeSet<PathBuf>,
) -> Option<BTreeMap<PathBuf, PathBuf>> {
    let mut members = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut children = fs::read_dir(directory).await.ok()?;
        while let Some(child) = children.next_entry().await.ok()? {
            let path = child.path();
            let relative = path.strip_prefix(root).ok()?.to_path_buf();
            let canonical = PathBuf::from(relative.to_str()?.nfc().collect::<String>());
            if !expected.remove(&canonical) {
                return None;
            }
            let kind = child.file_type().await.ok()?;
            if kind.is_symlink() {
                return None;
            }
            members.insert(canonical, relative);
            if kind.is_dir() {
                pending.push(path);
            } else if !kind.is_file() {
                return None;
            }
        }
    }
    expected.is_empty().then_some(members)
}
