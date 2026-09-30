//! Local device identity implementation.
//!
//! This module provides a filesystem-based persistence layer for the device identity.
//! The device ID is stored as a plain text UUID in the application data directory.
//!
//! ## Architecture Notes
//!
//! - **No Repository pattern needed**: DeviceId is a singleton, not a collection
//! - **Port in core, implementation in infra**: `DeviceIdentityPort` is defined in uc-core
//! - **Fail-fast on init**: If we can't load/create the ID, the app should not start
//! - **Immutable once created**: DeviceId never changes for the lifetime of the installation

mod storage;

use anyhow::Result;
use std::path::PathBuf;
use uc_core::ids::DeviceId;
use uc_core::ports::DeviceIdentityPort;
use uc_observability_contract::uc_info;

use storage::StoredIdentity;

/// Local filesystem-backed device identity.
///
/// This struct implements `DeviceIdentityPort` by storing the device ID
/// as a plain text file in the application data directory.
pub struct LocalDeviceIdentity {
    device_id: DeviceId,
}

impl LocalDeviceIdentity {
    /// Load existing device ID or create a new one.
    ///
    /// This is the primary entry point for obtaining the device identity.
    /// It will:
    /// 1. Try to load from disk
    /// 2. If not found, generate a new UUID v4 and persist it
    /// 3. Fail-fast on any I/O error (app should not start without valid identity)
    pub fn load_or_create(config_dir: PathBuf) -> Result<Self> {
        let reason = match storage::load_from_disk(&config_dir)? {
            StoredIdentity::Present(id) => return Ok(Self { device_id: id }),
            StoredIdentity::Missing => "missing",
            StoredIdentity::Empty => "empty",
        };
        // 仅凭文件缺失无法区分首次运行与数据目录被清空；对端会把新身份视为新设备，因此每次重新生成都留痕，不记 id 值。
        uc_info!(reason = reason, "device identity generated");
        let id = DeviceId::new(uuid::Uuid::new_v4().to_string());
        storage::save_to_disk(&config_dir, &id)?;
        Ok(Self { device_id: id })
    }
}

impl DeviceIdentityPort for LocalDeviceIdentity {
    fn current_device_id(&self) -> DeviceId {
        self.device_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regenerating_the_identity_is_recorded_and_an_existing_one_is_silent() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let directory = tempfile::tempdir().unwrap();

        let first = LocalDeviceIdentity::load_or_create(directory.path().to_path_buf()).unwrap();
        let second = LocalDeviceIdentity::load_or_create(directory.path().to_path_buf()).unwrap();
        std::fs::write(directory.path().join("device_id.txt"), "  \n").unwrap();
        let third = LocalDeviceIdentity::load_or_create(directory.path().to_path_buf()).unwrap();

        assert_eq!(first.current_device_id(), second.current_device_id());
        assert_ne!(first.current_device_id(), third.current_device_id());
        assert_eq!(logs.count("device identity generated"), 2);
        assert!(logs.output().contains("reason=\"missing\""));
        assert!(logs.output().contains("reason=\"empty\""));
        assert!(!logs
            .output()
            .contains(&third.current_device_id().to_string()));
    }
}
