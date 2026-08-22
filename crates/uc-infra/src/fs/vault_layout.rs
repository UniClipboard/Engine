use std::path::{Path, PathBuf};

const LEGACY_REBUILD_TARGET_FILE: &str = ".setup_status..legacy_isolation_target";
const RE_PAIRING_STATE_FILE: &str = ".re-pairing-state-v1";
const LEGACY_CURRENT_SPACE_ID_FILE: &str = ".current-space-id-v1";
const ENGINE_UPGRADE_CURSOR_FILE: &str = ".engine-upgrade-cursor.json";

#[derive(Debug, Clone)]
pub struct VaultLayout {
    root: PathBuf,
}

impl VaultLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn space_rebuild_progress_path(&self) -> PathBuf {
        self.root.join(LEGACY_REBUILD_TARGET_FILE)
    }

    pub fn re_pairing_state_path(&self) -> PathBuf {
        self.root.join(RE_PAIRING_STATE_FILE)
    }

    pub fn legacy_current_space_id_path(&self) -> PathBuf {
        self.root.join(LEGACY_CURRENT_SPACE_ID_FILE)
    }

    pub fn engine_upgrade_cursor_path(&self) -> PathBuf {
        self.root.join(ENGINE_UPGRADE_CURSOR_FILE)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_legacy_vault_file_paths() {
        let root = PathBuf::from("/vault");
        let layout = VaultLayout::new(root.clone());

        assert_eq!(
            layout.space_rebuild_progress_path(),
            root.join(".setup_status..legacy_isolation_target")
        );
        assert_eq!(
            layout.re_pairing_state_path(),
            root.join(".re-pairing-state-v1")
        );
        assert_eq!(
            layout.legacy_current_space_id_path(),
            root.join(".current-space-id-v1")
        );
    }
}
