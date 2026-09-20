use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// 刷新当前写入流程拥有的已有普通文件；不创建、不截断，也不修改访问权限。
/// Windows 的 FlushFileBuffers 要求写权限，不能使用 File::open 的只读句柄。
pub(crate) fn sync_existing_file(path: &Path) -> io::Result<()> {
    OpenOptions::new().write(true).open(path)?.sync_all()
}

#[cfg(not(windows))]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let wide = |path: &Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    // SAFETY: both buffers are owned, NUL-terminated UTF-16 paths and remain
    // alive for the duration of the Win32 call.
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    // Windows commits the replacement itself with MOVEFILE_WRITE_THROUGH;
    // opening a directory does not provide a portable FlushFileBuffers handle.
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
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

    #[test]
    fn replacement_overwrites_an_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("destination");
        let source = root.path().join("source");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::write(&source, b"new").unwrap();
        replace_file(&source, &destination).unwrap();
        sync_directory(root.path()).unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"new");
        assert!(!source.exists());
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
