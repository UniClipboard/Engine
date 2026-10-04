use std::{fs, io, path::PathBuf};

use uc_core::ports::{SecureStorageError, SecureStoragePort};

/// File-backed storage for secret material that must remain in a dedicated
/// application-managed directory.
#[derive(Clone)]
pub struct FileSecureStorage {
    base_dir: PathBuf,
}

impl FileSecureStorage {
    pub fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn file_path(&self, key: &str) -> PathBuf {
        let encoded = hex::encode(key.as_bytes());
        self.base_dir.join(format!("{encoded}.bin"))
    }

    fn map_io_error(context: &'static str, error: io::Error) -> SecureStorageError {
        SecureStorageError::StorageFailed {
            source: anyhow::Error::new(error).context(context),
        }
    }
}

impl SecureStoragePort for FileSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        match fs::read(self.file_path(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Self::map_io_error(
                "failed to read secure storage file",
                error,
            )),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        fs::create_dir_all(&self.base_dir).map_err(|error| {
            Self::map_io_error("failed to create secure storage directory", error)
        })?;
        let path = self.file_path(key);
        let temporary_path = path.with_extension("tmp");
        fs::write(&temporary_path, value).map_err(|error| {
            Self::map_io_error("failed to write secure storage temporary file", error)
        })?;
        fs::rename(&temporary_path, &path)
            .map_err(|error| Self::map_io_error("failed to replace secure storage file", error))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
                Self::map_io_error("failed to set secure storage file permissions", error)
            })?;
        }

        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        match fs::remove_file(self.file_path(key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Self::map_io_error(
                "failed to delete secure storage file",
                error,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use uc_observability_contract::error_source::find_source;

    use super::*;

    #[test]
    fn read_failure_keeps_io_source_without_path_text() {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileSecureStorage::with_base_dir(dir.path().to_path_buf());
        // 在密钥文件位置放一个目录，让读取返回真实的 IO 错误（而非 NotFound）。
        fs::create_dir(storage.file_path("private-key")).unwrap();

        let error = storage.get("private-key").unwrap_err();

        assert!(matches!(error, SecureStorageError::StorageFailed { .. }));
        assert!(find_source::<io::Error>(&error).is_some());
        let dir_text = dir.path().display().to_string();
        let chain = std::iter::successors(Some(&error as &(dyn Error + 'static)), |&current| {
            current.source()
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>();
        assert!(
            chain.iter().all(|text| !text.contains(&dir_text)),
            "{chain:?}"
        );
    }
}
