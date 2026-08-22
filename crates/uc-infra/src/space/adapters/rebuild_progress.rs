use std::path::PathBuf;

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uc_application::deps::{SpaceRebuildProgressError, SpaceRebuildProgressPort};
use uc_core::ids::SpaceId;

pub struct FileSpaceRebuildProgress {
    target_path: PathBuf,
}

impl FileSpaceRebuildProgress {
    pub fn new(target_path: PathBuf) -> Self {
        Self { target_path }
    }

    async fn ensure_parent_dir(&self) -> Result<(), SpaceRebuildProgressError> {
        let Some(parent) = self.target_path.parent() else {
            return Ok(());
        };

        fs::create_dir_all(parent)
            .await
            .map_err(|_| SpaceRebuildProgressError::Unavailable)
    }
}

#[async_trait]
impl SpaceRebuildProgressPort for FileSpaceRebuildProgress {
    async fn load_target(&self) -> Result<Option<SpaceId>, SpaceRebuildProgressError> {
        match fs::read_to_string(&self.target_path).await {
            Ok(value) => Ok(Some(SpaceId::from_str(value.trim()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(SpaceRebuildProgressError::Unavailable),
        }
    }

    async fn store_target(&self, space_id: &SpaceId) -> Result<(), SpaceRebuildProgressError> {
        self.ensure_parent_dir().await?;
        let mut file = fs::File::create(&self.target_path)
            .await
            .map_err(|_| SpaceRebuildProgressError::Unavailable)?;
        file.write_all(space_id.as_str().as_bytes())
            .await
            .map_err(|_| SpaceRebuildProgressError::Unavailable)?;
        file.sync_all()
            .await
            .map_err(|_| SpaceRebuildProgressError::Unavailable)?;
        Ok(())
    }

    async fn clear_target(&self) -> Result<(), SpaceRebuildProgressError> {
        match fs::remove_file(&self.target_path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SpaceRebuildProgressError::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn target_survives_recreation_and_clear_is_idempotent() {
        let directory = tempdir().unwrap();
        let target_path = directory.path().join("rebuild-target");
        let target = SpaceId::new();
        let progress = FileSpaceRebuildProgress::new(target_path.clone());

        assert_eq!(progress.load_target().await.unwrap(), None);

        progress.store_target(&target).await.unwrap();
        let reopened = FileSpaceRebuildProgress::new(target_path);
        assert_eq!(reopened.load_target().await.unwrap(), Some(target));

        reopened.clear_target().await.unwrap();
        reopened.clear_target().await.unwrap();
        assert_eq!(reopened.load_target().await.unwrap(), None);
    }
}
