// Tracing support for infra layer instrumentation
pub use tracing;

pub mod app_version_state;
pub mod blob;
pub mod clipboard;
pub mod config;
pub mod config_migration;
pub mod db;
pub mod device;
pub mod engine_version_state;
pub mod file_secure_storage;
pub mod file_transfer;
pub mod first_sync_state;
pub mod fs;
pub mod migration_state;
#[cfg(feature = "lan-compat")]
pub mod mobile_sync;
pub mod network;
pub mod pairing;
pub mod rendezvous;
pub mod search;
pub mod security;
pub mod settings;
pub mod space;
pub mod time;

pub use app_version_state::FileAppVersionStateRepository;
pub use engine_version_state::FileEngineVersionStateRepository;
pub use file_secure_storage::FileSecureStorage;
pub use first_sync_state::FileFirstSyncStateRepository;
pub use migration_state::FileLegacyMigrationRecovery;
pub use time::{SystemClock, Timer};

/// 登记本 crate 拥有的错误类型，使模块日志能渲染它们的错误链；由 Engine 装配统一调用。
pub fn register_log_safe_errors() {
    uc_observability_contract::module_log::register_error_layer_renderers(
        &space::error_layer_renderers(),
    );
}
