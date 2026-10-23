//! 待导入现场的只读分类；采用、诊断、恢复与重新暂存共用同一边界。

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use uc_core::ports::config_migration::ConfigImportRejection;

use super::adoption::ADOPTION_JOURNAL;
use super::staging::{
    PendingImportMarker, CONTROL_DB_MEMBER, DB_MEMBER, PENDING_IMPORT_MARKER,
    PENDING_IMPORT_SCHEMA_VER, RUNTIME_MANIFEST_MEMBER, STAGING_DIR_NAME,
};

pub(crate) enum PendingImportState {
    Absent,
    Applicable,
    Rejected(ConfigImportRejection),
}

/// journal 存在时必须交回原采用事务；链接和特殊文件视为存储失败。
pub(crate) fn adoption_journal_present(root: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(root.join(ADOPTION_JOURNAL)) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(unsafe_layout()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// 读取固定记录文件，拒绝链接和特殊文件；不依赖 marker 中的路径。
pub(super) fn read_import_record(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err(unsafe_layout()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
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
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(unsafe_layout());
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(unsafe_layout());
    }
    Ok(Some(bytes))
}

/// 只检查固定暂存树。链接或 I/O 错误保持失败边界，不能误报安全拒绝。
pub(crate) fn inspect_pending_import(root: &Path) -> io::Result<PendingImportState> {
    let Some(bytes) = read_import_record(&root.join(PENDING_IMPORT_MARKER))? else {
        return Ok(PendingImportState::Absent);
    };
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err(unsafe_layout());
    }
    let staging = root.join(STAGING_DIR_NAME);
    let staging_present = match fs::symlink_metadata(&staging) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => return Err(unsafe_layout()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if staging_present {
        let mut pending = vec![staging.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    pending.push(entry.path());
                } else if !kind.is_file() {
                    return Err(unsafe_layout());
                }
            }
        }
    }
    let parsed = match serde_json::from_slice::<PendingImportMarker>(&bytes) {
        Ok(marker) => marker,
        Err(_) => {
            return Ok(PendingImportState::Rejected(
                ConfigImportRejection::MalformedMarker,
            ))
        }
    };
    if parsed.schema_ver != PENDING_IMPORT_SCHEMA_VER || parsed.staging_dir != STAGING_DIR_NAME {
        return Ok(PendingImportState::Rejected(
            ConfigImportRejection::UnsupportedFormat,
        ));
    }
    for member in [DB_MEMBER, CONTROL_DB_MEMBER, RUNTIME_MANIFEST_MEMBER] {
        match fs::symlink_metadata(staging.join(member)) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(unsafe_layout()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(PendingImportState::Rejected(
                    ConfigImportRejection::MissingRuntimeMembers,
                ))
            }
            Err(error) => return Err(error),
        }
    }
    Ok(PendingImportState::Applicable)
}

fn unsafe_layout() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unsafe pending import layout")
}
