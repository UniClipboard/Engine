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
            "network log run lease is held by another writer",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(file)
}

pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    // 先清掉可能残留的临时文件（含符号链接本身），再以独占创建写入，不跟随任何已有路径。
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

/// 回收没有活动写入者的旧运行目录：先按保留时间，再在根目录超额时从最旧的起。
///
/// 清理锁串行不同进程的扫描与运行目录的建立；运行是否活动只看它的 lease 锁，成功取得锁才可删除。
/// 锁忙、权限失败或任何 I/O 错误都跳过，不等待、不影响采集。
/// 取得串行各进程扫描的清理锁；忙时返回 `WouldBlock`。
pub(crate) fn open_cleanup_lock(network_root: &Path) -> io::Result<File> {
    create_private_directory(network_root)?;
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let cleanup = options
        .create(true)
        .truncate(false)
        .write(true)
        .open(network_root.join(CLEANUP_LOCK_FILE))?;
    cleanup.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "network log cleanup is running in another process",
        ),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(cleanup)
}

/// 调用方已持有清理锁。
pub(crate) fn collect_old_runs(
    network_root: &Path,
    own_directory: &Path,
    retention: Duration,
    root_bytes: u64,
) -> io::Result<()> {
    let mut runs = Vec::new();
    for role in std::fs::read_dir(network_root)? {
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
        if remove_inactive_run(&run.path) {
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

/// 取得 lease 锁才说明运行不再活动；持锁期间删除目录，缺少 lease 的目录视为孤儿。
fn remove_inactive_run(directory: &Path) -> bool {
    let lease_path = directory.join(LEASE_FILE);
    let lease = match OpenOptions::new().write(true).open(&lease_path) {
        Ok(lease) => Some(lease),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return false,
    };
    if let Some(lease) = &lease {
        if lease.try_lock().is_err() {
            return false;
        }
    }
    #[cfg(windows)]
    drop(lease);
    std::fs::remove_dir_all(directory).is_ok()
}
