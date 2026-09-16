use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// 刷新当前写入流程拥有的已有普通文件；不创建、不截断，也不修改访问权限。
/// Windows 的 FlushFileBuffers 要求写权限，不能使用 File::open 的只读句柄。
pub(crate) fn sync_existing_file(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_preserves_existing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing");
        std::fs::write(&path, b"synthetic retained bytes").unwrap();
        sync_existing_file(&path).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"synthetic retained bytes");
    }

    #[test]
    fn sync_does_not_create_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("missing");
        assert_eq!(
            sync_existing_file(&path).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!path.exists());
    }

    #[test]
    fn sync_rejects_directories() {
        let root = tempfile::tempdir().unwrap();
        assert!(sync_existing_file(root.path()).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_readonly_flush_fails_but_writable_flush_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing");
        std::fs::write(&path, b"synthetic retained bytes").unwrap();
        let error = std::fs::File::open(&path).unwrap().sync_all().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(5));
        sync_existing_file(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_sharing_conflict_is_not_suppressed() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing");
        std::fs::write(&path, b"synthetic retained bytes").unwrap();
        let held = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert_eq!(
            sync_existing_file(&path).unwrap_err().raw_os_error(),
            Some(32)
        );
        drop(held);
        sync_existing_file(&path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_readonly_attribute_is_not_bypassed() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("existing");
        std::fs::write(&path, b"synthetic retained bytes").unwrap();
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        let result = sync_existing_file(&path);
        std::fs::set_permissions(&path, original).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(std::fs::read(path).unwrap(), b"synthetic retained bytes");
    }
}
