use std::path::{Path, PathBuf};

use super::ActiveRuntimeManifestV3;

const PROFILE_DATA_DIRECTORY: &str = "profile-data-generations";
const SPACE_CONTROL_DIRECTORY: &str = "space-control-generations";
pub(crate) const PROFILE_DATABASE_FILE: &str = "profile.sqlite";
pub(crate) const CONTROL_DATABASE_FILE: &str = "control.sqlite";
pub(crate) const PAYLOAD_OUTPUT_DIRECTORY: &str = "v3-payloads";
const BLOB_DIRECTORY: &str = "blobs";
const GENERATION_PATH_DOMAIN: &[u8] = b"uniclipboard/profile-upgrade-generation-path/v1\0";

/// 已提升 V3 manifest 对应的唯一 production 文件布局。
///
/// 路径只由 profile 根目录和两个 opaque generation 派生，不读取或编码
/// 当前 SpaceId。升级 staging 与正常运行期必须共同使用此模块，防止两边
/// 对同一 manifest 解释出不同位置。
pub struct ProfileRuntimeLayout {
    profile_database: PathBuf,
    blob_root: PathBuf,
    control_database: PathBuf,
}

impl ProfileRuntimeLayout {
    pub fn v3(profile_root: &Path, manifest: &ActiveRuntimeManifestV3) -> Self {
        Self::from_generations(
            profile_root,
            manifest.layout().profile_data_generation(),
            manifest.layout().space_control_generation(),
        )
    }

    /// 尚无活动 Space 时，由 profile upgrade gate 准备的首个 V3 布局。
    pub fn prepared(
        profile_root: &Path,
        profile_data_generation: &[u8; 16],
        space_control_generation: &[u8; 16],
    ) -> Self {
        Self::from_generations(
            profile_root,
            profile_data_generation,
            space_control_generation,
        )
    }

    pub(crate) fn from_generations(
        profile_root: &Path,
        profile_data_generation: &[u8; 16],
        space_control_generation: &[u8; 16],
    ) -> Self {
        let payload_root = profile_generation_directory(profile_root, profile_data_generation)
            .join(PAYLOAD_OUTPUT_DIRECTORY);
        let control_root = control_generation_directory(profile_root, space_control_generation);
        Self {
            profile_database: payload_root.join(PROFILE_DATABASE_FILE),
            blob_root: payload_root.join(BLOB_DIRECTORY),
            control_database: control_root.join(CONTROL_DATABASE_FILE),
        }
    }

    pub fn profile_database(&self) -> &Path {
        &self.profile_database
    }

    pub fn blob_root(&self) -> &Path {
        &self.blob_root
    }

    pub fn control_database(&self) -> &Path {
        &self.control_database
    }
}

pub(crate) fn profile_generation_directory(profile_root: &Path, generation: &[u8; 16]) -> PathBuf {
    profile_root
        .join(PROFILE_DATA_DIRECTORY)
        .join(generation_token(generation))
}

pub(crate) fn control_generation_directory(profile_root: &Path, generation: &[u8; 16]) -> PathBuf {
    profile_root
        .join(SPACE_CONTROL_DIRECTORY)
        .join(generation_token(generation))
}

fn generation_token(generation: &[u8; 16]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(GENERATION_PATH_DOMAIN);
    hasher.update(generation);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use uc_core::ids::SpaceId;
    use uc_core::membership::ActiveRuntimeLayout;

    use super::ProfileRuntimeLayout;
    use crate::security::ActiveRuntimeManifestV3;

    #[test]
    fn v3_runtime_layout_uses_opaque_independent_generation_paths() {
        let manifest = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(
                SpaceId::from_str("sensitive-space-name"),
                [0x31; 16],
                [0x32; 16],
            )
            .unwrap(),
            [0x33; 16],
        )
        .unwrap();

        let layout = ProfileRuntimeLayout::v3(Path::new("profile-root"), &manifest);

        assert_eq!(
            layout.profile_database().file_name().unwrap(),
            "profile.sqlite"
        );
        assert_eq!(layout.blob_root().file_name().unwrap(), "blobs");
        assert_eq!(
            layout.control_database().file_name().unwrap(),
            "control.sqlite"
        );
        assert_ne!(
            layout.profile_database().parent().unwrap(),
            layout.control_database().parent().unwrap()
        );
        for path in [
            layout.profile_database(),
            layout.blob_root(),
            layout.control_database(),
        ] {
            let rendered = path.to_string_lossy();
            assert!(!rendered.contains("sensitive-space-name"));
            assert!(!rendered.contains("3131313131313131"));
            assert!(!rendered.contains("3232323232323232"));
        }
    }
}
