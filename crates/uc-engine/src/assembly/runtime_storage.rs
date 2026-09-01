use std::path::{Path, PathBuf};

use uc_infra::security::{space_generation_directory, ActiveRuntimeManifest, ProfileRuntimeLayout};

/// 启动 manifest 已认证后解析出的完整存储选择。
///
/// 调用方不能分别选择 profile database、control database、blob root 或 payload
/// 格式；这四项必须来自同一个 manifest 版本，避免混合 generation 对象图。
pub(crate) struct RuntimeStorageSelection {
    profile_database: PathBuf,
    control_database: PathBuf,
    blob_root: PathBuf,
    v3: bool,
    fresh_generations: Option<([u8; 16], [u8; 16])>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RuntimeStorageSelectionError {
    #[error("active profile database generation is unavailable")]
    ProfileDatabaseUnavailable,
    #[error("active control database generation is unavailable")]
    ControlDatabaseUnavailable,
    #[error("active blob generation is unavailable")]
    BlobGenerationUnavailable,
}

impl RuntimeStorageSelection {
    pub(crate) fn resolve(
        profile_root: &Path,
        legacy_database: PathBuf,
        legacy_blob_root: PathBuf,
        manifest: Option<&ActiveRuntimeManifest>,
    ) -> Result<Self, RuntimeStorageSelectionError> {
        match manifest {
            None => Ok(Self {
                profile_database: legacy_database.clone(),
                control_database: legacy_database,
                blob_root: legacy_blob_root,
                v3: false,
                fresh_generations: None,
            }),
            Some(ActiveRuntimeManifest::V2(manifest)) => {
                let directory = space_generation_directory(
                    &profile_root.join("space-generations"),
                    &manifest.space_id,
                    &manifest.database_generation,
                );
                let database = directory.join("target.sqlite");
                if !database.is_file() {
                    return Err(RuntimeStorageSelectionError::ProfileDatabaseUnavailable);
                }
                Ok(Self {
                    profile_database: database.clone(),
                    control_database: database,
                    blob_root: directory.join("blobs"),
                    v3: false,
                    fresh_generations: None,
                })
            }
            Some(ActiveRuntimeManifest::V3(manifest)) => {
                let layout = ProfileRuntimeLayout::v3(profile_root, manifest);
                if !layout.profile_database().is_file() {
                    return Err(RuntimeStorageSelectionError::ProfileDatabaseUnavailable);
                }
                if !layout.control_database().is_file() {
                    return Err(RuntimeStorageSelectionError::ControlDatabaseUnavailable);
                }
                if !layout.blob_root().is_dir() {
                    return Err(RuntimeStorageSelectionError::BlobGenerationUnavailable);
                }
                Ok(Self {
                    profile_database: layout.profile_database().to_path_buf(),
                    control_database: layout.control_database().to_path_buf(),
                    blob_root: layout.blob_root().to_path_buf(),
                    v3: true,
                    fresh_generations: None,
                })
            }
        }
    }

    pub(crate) fn fresh_v3(
        profile_root: &Path,
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
    ) -> Result<Self, RuntimeStorageSelectionError> {
        let layout = ProfileRuntimeLayout::prepared(
            profile_root,
            &profile_data_generation,
            &space_control_generation,
        );
        if !layout.profile_database().is_file() {
            return Err(RuntimeStorageSelectionError::ProfileDatabaseUnavailable);
        }
        if !layout.control_database().is_file() {
            return Err(RuntimeStorageSelectionError::ControlDatabaseUnavailable);
        }
        if !layout.blob_root().is_dir() {
            return Err(RuntimeStorageSelectionError::BlobGenerationUnavailable);
        }
        Ok(Self {
            profile_database: layout.profile_database().to_path_buf(),
            control_database: layout.control_database().to_path_buf(),
            blob_root: layout.blob_root().to_path_buf(),
            v3: true,
            fresh_generations: Some((profile_data_generation, space_control_generation)),
        })
    }

    pub(crate) fn profile_database(&self) -> &Path {
        &self.profile_database
    }

    pub(crate) fn control_database(&self) -> &Path {
        &self.control_database
    }

    pub(crate) fn blob_root(&self) -> &Path {
        &self.blob_root
    }

    pub(crate) const fn is_v3(&self) -> bool {
        self.v3
    }

    pub(crate) const fn fresh_generations(&self) -> Option<([u8; 16], [u8; 16])> {
        self.fresh_generations
    }
}

#[cfg(test)]
mod tests {
    use uc_core::ids::SpaceId;
    use uc_core::membership::ActiveRuntimeLayout;
    use uc_infra::security::{
        ActiveRuntimeManifest, ActiveRuntimeManifestV3, ProfileRuntimeLayout,
    };

    use super::RuntimeStorageSelection;

    #[test]
    fn v3_selection_requires_and_preserves_two_independent_databases() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(SpaceId::from_str("private-space"), [0x41; 16], [0x42; 16])
                .unwrap(),
            [0x43; 16],
        )
        .unwrap();
        let expected = ProfileRuntimeLayout::v3(directory.path(), &manifest);
        std::fs::create_dir_all(expected.profile_database().parent().unwrap()).unwrap();
        std::fs::create_dir_all(expected.control_database().parent().unwrap()).unwrap();
        std::fs::create_dir_all(expected.blob_root()).unwrap();
        std::fs::write(expected.profile_database(), b"profile").unwrap();
        std::fs::write(expected.control_database(), b"control").unwrap();

        let selection = RuntimeStorageSelection::resolve(
            directory.path(),
            directory.path().join("legacy.sqlite"),
            directory.path().join("legacy-blobs"),
            Some(&ActiveRuntimeManifest::V3(manifest)),
        )
        .unwrap();

        assert!(selection.is_v3());
        assert_eq!(selection.profile_database(), expected.profile_database());
        assert_eq!(selection.control_database(), expected.control_database());
        assert_eq!(selection.blob_root(), expected.blob_root());
        assert_ne!(selection.profile_database(), selection.control_database());
    }

    #[test]
    fn fresh_selection_carries_the_only_prepared_generation_pair() {
        let directory = tempfile::tempdir().unwrap();
        let layout = ProfileRuntimeLayout::prepared(directory.path(), &[0x51; 16], &[0x52; 16]);
        std::fs::create_dir_all(layout.profile_database().parent().unwrap()).unwrap();
        std::fs::create_dir_all(layout.control_database().parent().unwrap()).unwrap();
        std::fs::create_dir_all(layout.blob_root()).unwrap();
        std::fs::write(layout.profile_database(), b"profile").unwrap();
        std::fs::write(layout.control_database(), b"control").unwrap();

        let selection =
            RuntimeStorageSelection::fresh_v3(directory.path(), [0x51; 16], [0x52; 16]).unwrap();

        assert!(selection.is_v3());
        assert_eq!(
            selection.fresh_generations(),
            Some(([0x51; 16], [0x52; 16]))
        );
    }
}
