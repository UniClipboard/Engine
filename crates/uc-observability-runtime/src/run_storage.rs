//! 受管运行目录、租约与非活动运行回收。
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
const LEASE_FILE: &str = "lease.lock";
const CLEANUP_LOCK_FILE: &str = ".cleanup.lock";
pub(crate) fn create_private_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(directory)
    }
}

pub(crate) fn open_lease(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let file = options
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "log run lease is held by another writer",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    // 先删除残留的临时文件或符号链接，再以独占创建写入，不跟随已有路径。
    match std::fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut file = options.write(true).create_new(true).open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)
}

struct RunEntry {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

/// 获取串行各进程扫描的清理锁；忙时返回 `WouldBlock`。
pub(crate) fn open_cleanup_lock(root: &Path) -> io::Result<File> {
    create_private_directory(root)?;
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let cleanup = options
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(CLEANUP_LOCK_FILE))?;
    cleanup.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "log cleanup is running in another process",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(cleanup)
}

/// 调用方已持有清理锁。
pub(crate) fn collect_old_runs(
    root: &Path,
    own_directory: &Path,
    retention: Duration,
    root_bytes: u64,
) -> io::Result<()> {
    let mut runs = Vec::new();
    for role in std::fs::read_dir(root)? {
        let role = role?;
        if !role.file_type()?.is_dir() {
            continue;
        }
        for run in std::fs::read_dir(role.path())? {
            let run = run?;
            if run.file_type()?.is_dir() {
                runs.push(describe_run(run.path()));
            }
        }
    }
    runs.sort_by_key(|run| run.modified);
    let now = SystemTime::now();
    let mut total: u64 = runs.iter().map(|run| run.bytes).sum();
    for run in runs {
        if run.path == own_directory {
            continue;
        }
        let expired = now
            .duration_since(run.modified)
            .is_ok_and(|age| age > retention);
        if !expired && total <= root_bytes {
            continue;
        }
        if remove_inactive_run(&run.path)? {
            total = total.saturating_sub(run.bytes);
        }
    }
    Ok(())
}

fn describe_run(path: PathBuf) -> RunEntry {
    let mut bytes = 0_u64;
    let mut modified = SystemTime::UNIX_EPOCH;
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                bytes = bytes.saturating_add(metadata.len());
                if let Ok(time) = metadata.modified() {
                    modified = modified.max(time);
                }
            }
        }
    }
    RunEntry {
        path,
        bytes,
        modified,
    }
}

/// 取得租约锁后才删除非活动运行。缺少租约的目录视为孤儿。
fn remove_inactive_run(directory: &Path) -> io::Result<bool> {
    let lease_path = directory.join(LEASE_FILE);
    let lease = match OpenOptions::new().write(true).open(&lease_path) {
        Ok(lease) => Some(lease),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(lease) = &lease {
        match lease.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    #[cfg(windows)]
    drop(lease);
    std::fs::remove_dir_all(directory)?;
    Ok(true)
}
