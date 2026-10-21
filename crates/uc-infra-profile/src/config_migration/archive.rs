//! Plaintext tar archive packing/unpacking for a config bundle.
//!
//! The archive is an uncompressed tar carrying named members
//! (`manifest.json`, `db/uniclipboard.db`, `vault/...`, `settings.json`,
//! `secrets.json`, optional `ui-state/*.json`). This module only deals with the
//! archive bytes; it does not know what the members *mean* — the adapter
//! assembles and interprets them.
//!
//! Unpacking validates member paths (no absolute paths, no `..` traversal) and
//! bounds total extracted size, so a hostile archive cannot escape the staging
//! directory or exhaust memory.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read};

use super::bundle::MAX_EXPORT_ARCHIVE_BYTES;
use std::path::{Component, Path, PathBuf};

/// Hard ceiling on the cumulative uncompressed size of all members read out of
/// an archive. Mirrors the sealed-payload ceiling in [`super::bundle`]; tar adds
/// only fixed-size headers, so reusing the same order of magnitude is safe.
const MAX_ARCHIVE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Archive-level failures.
#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    /// The tar stream is malformed or a member could not be read.
    #[error("malformed archive")]
    Malformed {
        #[source]
        source: Option<anyhow::Error>,
    },
    /// A member path is unsafe (absolute, or escapes via `..`).
    #[error("unsafe member path")]
    UnsafePath {
        #[source]
        source: Option<anyhow::Error>,
    },
    /// Total extracted size exceeded the in-memory ceiling.
    #[error("archive exceeds size ceiling")]
    TooLarge,
}

/// 纯状态或输入校验失败时 `source` 为空；有下层错误时保留为来源。
impl ArchiveError {
    pub fn malformed() -> Self {
        Self::Malformed { source: None }
    }

    pub fn malformed_from(source: impl Into<anyhow::Error>) -> Self {
        Self::Malformed {
            source: Some(source.into()),
        }
    }

    pub fn unsafe_path() -> Self {
        Self::UnsafePath { source: None }
    }

    pub fn unsafe_path_from(source: impl Into<anyhow::Error>) -> Self {
        Self::UnsafePath {
            source: Some(source.into()),
        }
    }
}

/// An in-memory archive: ordered map of member path → bytes.
///
/// `BTreeMap` keeps a deterministic member order so packing is reproducible
/// (helps round-trip tests and keeps the manifest's `included` list stable).
#[derive(Debug, Default, Clone)]
pub struct BundleArchive {
    members: BTreeMap<String, Vec<u8>>,
}

impl BundleArchive {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a member.
    pub fn insert(&mut self, path: impl Into<String>, bytes: Vec<u8>) {
        self.members.insert(path.into(), bytes);
    }

    /// Borrow a member's bytes, if present.
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.members.get(path).map(Vec::as_slice)
    }

    /// Member paths present, in deterministic order.
    pub fn member_paths(&self) -> Vec<String> {
        self.members.keys().cloned().collect()
    }

    /// Iterate over members in deterministic order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<u8>)> {
        self.members.iter()
    }

    /// 预检所有成员的累计 tar 大小，再流式读取磁盘成员，避免整份 blob 缓冲副本。
    pub fn to_tar_bytes(&self, files: Vec<(String, PathBuf)>) -> Result<Vec<u8>, ArchiveError> {
        let mut sizes = Vec::with_capacity(files.len());
        let mut total = 1024_u64; // tar 结束块。
        for size in self.members.values().map(|bytes| bytes.len() as u64) {
            total = tar_size_after(total, size)?;
        }
        for (_, file) in &files {
            let metadata = fs::symlink_metadata(file).map_err(ArchiveError::malformed_from)?;
            if !metadata.is_file() {
                return Err(ArchiveError::malformed());
            }
            total = tar_size_after(total, metadata.len())?;
            sizes.push(metadata.len());
        }
        let mut builder = tar::Builder::new(Vec::new());
        for (path, bytes) in &self.members {
            let mut header = tar::Header::new_gnu();
            header
                .set_path(path)
                .map_err(ArchiveError::unsafe_path_from)?;
            header.set_size(bytes.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append(&header, Cursor::new(bytes))
                .map_err(ArchiveError::malformed_from)?;
        }
        for ((path, source), size) in files.into_iter().zip(sizes) {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
                options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
            }
            let mut file = options.open(source).map_err(ArchiveError::malformed_from)?;
            let metadata = file.metadata().map_err(ArchiveError::malformed_from)?;
            if !metadata.is_file() || metadata.len() != size {
                return Err(ArchiveError::malformed());
            }
            let mut header = tar::Header::new_gnu();
            header
                .set_path(&path)
                .map_err(ArchiveError::unsafe_path_from)?;
            header.set_size(size);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append(&header, (&mut file).take(size))
                .map_err(ArchiveError::malformed_from)?;
            if file.metadata().map_err(ArchiveError::malformed_from)?.len() != size {
                return Err(ArchiveError::malformed());
            }
        }
        let out = builder.into_inner().map_err(ArchiveError::malformed_from)?;
        if out.len() as u64 != total {
            return Err(ArchiveError::malformed());
        }
        Ok(out)
    }

    /// Parse an uncompressed tar byte stream into an in-memory archive,
    /// rejecting unsafe paths and bounding total size.
    pub fn from_tar_bytes(bytes: &[u8]) -> Result<Self, ArchiveError> {
        let mut archive = tar::Archive::new(Cursor::new(bytes));
        let mut members = BTreeMap::new();
        let mut total: u64 = 0;

        let entries = archive.entries().map_err(ArchiveError::malformed_from)?;
        for entry in entries {
            let mut entry = entry.map_err(ArchiveError::malformed_from)?;
            let path = entry.path().map_err(ArchiveError::malformed_from)?;
            let rel = safe_relative_path(&path)?;

            let size = entry
                .header()
                .size()
                .map_err(ArchiveError::malformed_from)?;
            total = total.checked_add(size).ok_or(ArchiveError::TooLarge)?;
            if total > MAX_ARCHIVE_BYTES {
                return Err(ArchiveError::TooLarge);
            }

            let mut buf = Vec::with_capacity(size as usize);
            entry
                .read_to_end(&mut buf)
                .map_err(ArchiveError::malformed_from)?;
            members.insert(rel, buf);
        }

        Ok(Self { members })
    }
}

/// 计入每个成员的头和 512 字节对齐，同时预留 bundle 的加密开销。
fn tar_size_after(total: u64, size: u64) -> Result<u64, ArchiveError> {
    let padded = size.checked_add(511).ok_or(ArchiveError::TooLarge)? / 512 * 512;
    let next = total
        .checked_add(512)
        .and_then(|total| total.checked_add(padded))
        .ok_or(ArchiveError::TooLarge)?;
    if next > MAX_ARCHIVE_BYTES.min(MAX_EXPORT_ARCHIVE_BYTES) {
        return Err(ArchiveError::TooLarge);
    }
    Ok(next)
}

/// Validate a tar member path: reject absolute paths and any `..` component,
/// and normalize to a forward-slash relative string.
fn safe_relative_path(path: &Path) -> Result<String, ArchiveError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let s = part.to_str().ok_or_else(ArchiveError::unsafe_path)?;
                parts.push(s.to_string());
            }
            // Reject root, prefixes (Windows drive), and parent traversal.
            Component::RootDir
            | Component::Prefix(_)
            | Component::ParentDir
            | Component::CurDir => return Err(ArchiveError::unsafe_path()),
        }
    }
    if parts.is_empty() {
        return Err(ArchiveError::unsafe_path());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_then_unpack_preserves_members() {
        let mut archive = BundleArchive::new();
        archive.insert("manifest.json", b"{\"schema_ver\":1}".to_vec());
        archive.insert("db/uniclipboard.db", vec![0u8, 1, 2, 3, 4]);
        archive.insert("vault/device_id.txt", b"uuid-here".to_vec());

        let tar = archive.to_tar_bytes(Vec::new()).unwrap();
        let back = BundleArchive::from_tar_bytes(&tar).unwrap();

        assert_eq!(back.get("manifest.json"), Some(&b"{\"schema_ver\":1}"[..]));
        assert_eq!(back.get("db/uniclipboard.db"), Some(&[0u8, 1, 2, 3, 4][..]));
        assert_eq!(back.get("vault/device_id.txt"), Some(&b"uuid-here"[..]));
        assert_eq!(back.member_paths().len(), 3);
    }

    #[test]
    fn empty_member_round_trips() {
        let mut archive = BundleArchive::new();
        archive.insert("empty.bin", Vec::new());
        let tar = archive.to_tar_bytes(Vec::new()).unwrap();
        let back = BundleArchive::from_tar_bytes(&tar).unwrap();
        assert_eq!(back.get("empty.bin"), Some(&[][..]));
    }

    #[test]
    fn member_order_is_deterministic() {
        let mut a = BundleArchive::new();
        a.insert("z.json", b"z".to_vec());
        a.insert("a.json", b"a".to_vec());
        assert_eq!(a.member_paths(), vec!["a.json", "z.json"]);
    }

    #[test]
    fn absolute_member_path_is_rejected() {
        let err = safe_relative_path(Path::new("/etc/passwd")).unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafePath { .. }));
    }

    #[test]
    fn parent_traversal_is_rejected() {
        let err = safe_relative_path(Path::new("../../escape")).unwrap_err();
        assert!(matches!(err, ArchiveError::UnsafePath { .. }));
    }

    #[test]
    fn malformed_tar_is_rejected() {
        let err = BundleArchive::from_tar_bytes(b"not a tar at all").unwrap_err();
        // tar tolerates trailing garbage as EOF in some shapes; assert it does
        // not panic and yields either Malformed or an empty archive.
        let _ = err;
    }
}
