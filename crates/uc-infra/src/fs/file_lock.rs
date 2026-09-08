use std::fs::{File, TryLockError};

/// 非阻塞地取得文件独占锁；锁由文件句柄持有，关闭后自动释放。
pub fn try_lock_exclusive(file: &File) -> Result<(), TryLockError> {
    #[cfg(target_os = "android")]
    {
        use std::os::fd::AsRawFd;

        // 当前 Rust 标准库未在 Android 实现文件锁；直接使用系统的同等能力。
        // SAFETY: 借用的 File 保证调用期间文件描述符有效，flock 不持有指针。
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Err(TryLockError::WouldBlock)
        } else {
            Err(TryLockError::Error(error))
        }
    }
    #[cfg(not(target_os = "android"))]
    file.try_lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "uc-file-lock-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn open(path: &Path) -> File {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .unwrap()
    }

    #[test]
    fn independent_handles_contend_until_owner_closes() {
        let fixture = Fixture::new();
        let path = fixture.0.join("lease");
        let owner = open(&path);
        let contender = open(&path);
        try_lock_exclusive(&owner).unwrap();
        assert!(matches!(
            try_lock_exclusive(&contender),
            Err(TryLockError::WouldBlock)
        ));
        drop(owner);
        try_lock_exclusive(&contender).unwrap();
    }

    #[test]
    fn independent_files_do_not_contend() {
        let fixture = Fixture::new();
        let first = open(&fixture.0.join("first"));
        let second = open(&fixture.0.join("second"));
        try_lock_exclusive(&first).unwrap();
        try_lock_exclusive(&second).unwrap();
    }

    #[test]
    fn cross_process_probe() {
        let Some(path) = std::env::var_os("UC_FILE_LOCK_TEST_PATH") else {
            return;
        };
        let file = open(Path::new(&path));
        if std::env::var_os("UC_FILE_LOCK_TEST_BUSY").is_some() {
            assert!(matches!(
                try_lock_exclusive(&file),
                Err(TryLockError::WouldBlock)
            ));
        } else {
            try_lock_exclusive(&file).unwrap();
            // 不执行析构，验证进程退出也会释放锁。
            std::process::exit(0);
        }
    }

    #[test]
    fn other_process_contends_and_exit_releases_lock() {
        let fixture = Fixture::new();
        let path = fixture.0.join("lease");
        let owner = open(&path);
        try_lock_exclusive(&owner).unwrap();
        let child = |busy: bool| {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .arg("cross_process_probe")
                .arg("--nocapture")
                .env("UC_FILE_LOCK_TEST_PATH", &path)
                .env_remove("UC_FILE_LOCK_TEST_BUSY");
            if busy {
                command.env("UC_FILE_LOCK_TEST_BUSY", "1");
            }
            let output = command.output().unwrap();
            assert!(output.status.success(), "{:?}", output);
        };
        child(true);
        drop(owner);
        child(false);
        try_lock_exclusive(&open(&path)).unwrap();
    }
}
