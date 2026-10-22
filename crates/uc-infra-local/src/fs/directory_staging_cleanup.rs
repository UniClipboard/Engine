use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use uc_core::ports::{
    directory_staging_area_for_root, CleanupDirectoryStagingPort, DirectoryStagingCleanupError,
};

#[derive(Debug, Default)]
pub struct FsDirectoryStagingCleaner;

impl FsDirectoryStagingCleaner {
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self)
    }
}

#[async_trait]
impl CleanupDirectoryStagingPort for FsDirectoryStagingCleaner {
    async fn cleanup_staging_roots(
        &self,
        staged_roots: &[PathBuf],
    ) -> Result<(), DirectoryStagingCleanupError> {
        let areas = staged_roots
            .iter()
            .map(|root| directory_staging_area_for_root(root))
            .collect::<Result<BTreeSet<_>, _>>()?;

        for area in areas {
            match tokio::fs::remove_dir_all(&area).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(DirectoryStagingCleanupError::Backend(error.into()));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn rejects_non_staging_parent() {
        let result = directory_staging_area_for_root(Path::new("/tmp/user-files/root"));
        assert!(matches!(
            result,
            Err(DirectoryStagingCleanupError::InvalidPath)
        ));
    }

    #[test]
    fn accepts_receive_staging_parent() {
        let root = Path::new("/tmp/.uniclip-incoming-entry/root");
        assert_eq!(
            directory_staging_area_for_root(root).expect("valid staging root"),
            PathBuf::from("/tmp/.uniclip-incoming-entry")
        );
    }
}
