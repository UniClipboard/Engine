//! Private storage implementation for device identity.
//!
//! This module handles the low-level file I/O for persisting the device ID.
//! It is not part of the public API of the device module.

use anyhow::{Context, Result};
use std::path::PathBuf;
use uc_core::ids::DeviceId;

const DEVICE_ID_FILE: &str = "device_id.txt";

/// Load device ID from disk, returning None if file doesn't exist.
///
/// This is a private implementation detail of the device module.
pub(crate) fn load_from_disk(config_dir: &PathBuf) -> Result<Option<DeviceId>> {
    let path = config_dir.join(DEVICE_ID_FILE);

    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path).context("read device_id file failed")?;

    let id_str = content.trim();
    if id_str.is_empty() {
        return Ok(None);
    }

    // Validate UUID format
    uuid::Uuid::parse_str(id_str).context("invalid device_id UUID in file")?;

    Ok(Some(DeviceId::new(id_str)))
}

/// Save device ID to disk, creating parent directory if needed.
///
/// This is a private implementation detail of the device module.
pub(crate) fn save_to_disk(config_dir: &PathBuf, id: &DeviceId) -> Result<()> {
    // Ensure parent directory exists
    std::fs::create_dir_all(config_dir).context("create config dir failed")?;

    let path = config_dir.join(DEVICE_ID_FILE);

    // Try atomic write using temp file + rename first
    // If rename fails (e.g., cross-device link in CI environments), fall back to direct write
    let tmp_path = path.with_extension("txt.tmp");
    std::fs::write(&tmp_path, id.as_str()).context("write temp device_id failed")?;

    match std::fs::rename(&tmp_path, &path) {
        Ok(_) => Ok(()),
        Err(_) => {
            // Rename failed - likely cross-device link or permission issue
            // Fall back to direct write (non-atomic but better than failing)
            // 改名失败已由回退写入处理；回退写入失败时以该写入错误为来源。
            std::fs::write(&path, id.as_str())
                .context("direct write device_id failed after rename error")?;
            // Clean up temp file if it still exists
            let _ = std::fs::remove_file(&tmp_path);
            Ok(())
        }
    }
}
