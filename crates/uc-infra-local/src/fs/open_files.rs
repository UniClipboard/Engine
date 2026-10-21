//! 本进程已打开、可能被加锁的文件计数，用于证明暂停后不再持有共享目录中的文件。
//!
//! 只枚举已有文件描述符并读取其路径，不新开任何文件：关闭指向同一文件的描述符会让本进程的 POSIX 锁全部失效，
//! 探测本身就会改变结果。结果只含计数，不含路径或文件名。

use std::path::{Path, PathBuf};

/// 位于给定根目录内、可能被加锁的已打开文件数。同一文件被多个描述符打开时按描述符计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenLockableFiles {
    /// SQLite 主库、WAL 与共享内存文件，以及其他 `.db` 数据库文件。
    pub databases: u64,
    /// 租约与锁文件（`*.lock`、`*.lease`、`.lease`）。
    pub leases: u64,
}

impl OpenLockableFiles {
    pub fn total(&self) -> u64 {
        self.databases.saturating_add(self.leases)
    }
}

/// 统计本进程在 `roots` 内打开的数据库与租约文件。当前平台无法枚举描述符路径时返回 `None`，不伪装成零。
/// 会做与描述符数量成正比的系统调用，不在异步运行线程上调用。
pub fn open_lockable_files(roots: &[PathBuf]) -> Option<OpenLockableFiles> {
    let roots: Vec<PathBuf> = roots
        .iter()
        .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
        .collect();
    let mut counts = OpenLockableFiles::default();
    for path in open_paths()? {
        if !roots.iter().any(|root| path.starts_with(root)) {
            continue;
        }
        match classify(&path) {
            Some(FileKind::Database) => counts.databases = counts.databases.saturating_add(1),
            Some(FileKind::Lease) => counts.leases = counts.leases.saturating_add(1),
            None => {}
        }
    }
    Some(counts)
}

enum FileKind {
    Database,
    Lease,
}

fn classify(path: &Path) -> Option<FileKind> {
    let name = path.file_name()?.to_str()?;
    if name.ends_with(".sqlite")
        || name.ends_with(".sqlite-wal")
        || name.ends_with(".sqlite-shm")
        || name.ends_with(".db")
        || name.ends_with(".db-wal")
        || name.ends_with(".db-shm")
    {
        Some(FileKind::Database)
    } else if name.ends_with(".lock") || name.ends_with(".lease") {
        Some(FileKind::Lease)
    } else {
        None
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn open_paths() -> Option<Vec<PathBuf>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;

    // SAFETY: 只读取进程描述符表上限，没有指针参数。
    let limit = unsafe { libc::getdtablesize() };
    let mut paths = Vec::new();
    let mut buffer = [0 as libc::c_char; libc::PATH_MAX as usize];
    for fd in 0..limit {
        // SAFETY: F_GETPATH 向至少 MAXPATHLEN 字节的缓冲区写入以 NUL 结尾的路径；无效或非文件描述符只返回错误。
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, buffer.as_mut_ptr()) } == 0 {
            // SAFETY: 调用成功时缓冲区内是以 NUL 结尾的字符串。
            let path = unsafe { CStr::from_ptr(buffer.as_ptr()) };
            paths.push(PathBuf::from(OsStr::from_bytes(path.to_bytes())));
        }
    }
    Some(paths)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_paths() -> Option<Vec<PathBuf>> {
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    Some(
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read_link(entry.path()).ok())
            .collect(),
    )
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android"
)))]
fn open_paths() -> Option<Vec<PathBuf>> {
    None
}
