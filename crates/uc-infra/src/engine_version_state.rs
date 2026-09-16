use std::path::PathBuf;

use crate::fs::VaultLayout;
use async_trait::async_trait;
use uc_core::ports::{
    AppVersionStateError, AppVersionStatePort, EngineVersionStateError, EngineVersionStatePort,
};

use crate::FileAppVersionStateRepository;

pub struct FileEngineVersionStateRepository {
    inner: FileAppVersionStateRepository,
}

impl FileEngineVersionStateRepository {
    pub fn with_defaults(app_data_root: PathBuf) -> Self {
        let path = VaultLayout::new(app_data_root).engine_upgrade_cursor_path();
        Self {
            inner: FileAppVersionStateRepository::new(path),
        }
    }
}

#[async_trait]
impl EngineVersionStatePort for FileEngineVersionStateRepository {
    async fn read(&self) -> Result<Option<String>, EngineVersionStateError> {
        self.inner.read().await.map_err(map_read_error)
    }

    async fn write(&self, version: &str) -> Result<(), EngineVersionStateError> {
        self.inner.write(version).await.map_err(map_write_error)
    }
}

fn map_read_error(error: AppVersionStateError) -> EngineVersionStateError {
    match error {
        AppVersionStateError::Read(message) => EngineVersionStateError::Read(message),
        AppVersionStateError::Corrupt(message) => EngineVersionStateError::Invalid(message),
        AppVersionStateError::Write(message) => EngineVersionStateError::Read(message),
    }
}

fn map_write_error(error: AppVersionStateError) -> EngineVersionStateError {
    EngineVersionStateError::Write(error.to_string())
}
