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

    fn map_io_error(context: &str, error: io::Error) -> SecureStorageError {
        SecureStorageError::Other(format!("{context}: {error}"))
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
