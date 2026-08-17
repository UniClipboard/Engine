//! HarmonyOS N-API bindings for the public `uc-engine` interface.

mod host;
mod runtime;

use napi::bindgen_prelude::{Buffer, External};
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction};
use napi::Env;
use napi_derive::napi;

pub use runtime::OhEngine;

#[napi(object)]
pub struct OhEngineConfig {
    pub app_version: String,
    pub profile_id: String,
}

#[napi(object, object_to_js = false)]
pub struct OhHost {
    pub private_data_directory: String,
    pub cache_directory: String,
    pub temporary_directory: String,
    pub secure_storage_get: ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    pub secure_storage_set: ThreadsafeFunction<(String, Buffer), ErrorStrategy::Fatal>,
    pub secure_storage_delete: ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    pub file_metadata: ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    pub file_read_chunk: ThreadsafeFunction<(String, String, u32), ErrorStrategy::Fatal>,
    pub file_write_chunk: ThreadsafeFunction<(String, String, Buffer), ErrorStrategy::Fatal>,
    pub file_finish_write: ThreadsafeFunction<String, ErrorStrategy::Fatal>,
    pub clipboard_read: ThreadsafeFunction<(), ErrorStrategy::Fatal>,
    pub clipboard_write: ThreadsafeFunction<OhClipboardSnapshot, ErrorStrategy::Fatal>,
}

#[napi(object)]
pub struct OhClipboardRepresentation {
    pub kind: String,
    pub format: String,
    pub mime_type: Option<String>,
    pub bytes: Option<Buffer>,
    pub handle: Option<String>,
    pub display_name: Option<String>,
    pub size_bytes: Option<String>,
}

#[napi(object)]
pub struct OhClipboardSnapshot {
    pub observed_at_ms: f64,
    pub representations: Vec<OhClipboardRepresentation>,
}

#[napi(object)]
pub struct OhSpaceCreated {
    pub space_id: String,
    pub self_device_id: String,
    pub identity_fingerprint: String,
}

#[napi(object)]
pub struct OhSessionRecovery {
    pub unlocked: bool,
    pub resumed: bool,
}

#[napi(object)]
pub struct OhNetworkRecoveryStatus {
    pub phase: String,
    pub retryable: bool,
    pub next_retry_in_ms: Option<f64>,
}

#[napi(object)]
pub struct OhLocalDevice {
    pub device_id: String,
    pub display_name: String,
}

#[napi(object)]
pub struct OhWorkspaceConvergence {
    pub phase: String,
    pub revision: f64,
    pub history_event_count: u32,
    pub effective_member_count: u32,
    pub pending_removal_decision_device_ids: Vec<String>,
    pub pending_removal_decision_event_id: Option<String>,
    pub diverged_peer_device_ids: Vec<String>,
    pub upgrade_required_peer_device_ids: Vec<String>,
    pub convergence_digest: Option<String>,
    pub removed: bool,
    pub updated_at_ms: f64,
    pub failure_category: Option<String>,
}

#[napi(object)]
pub struct OhActiveClipboard {
    pub entry_id: String,
    pub activated_by: String,
}

#[napi(object)]
pub struct OhInvitationIssued {
    pub invitation_code: String,
    pub expires_at_ms: f64,
    pub availability: String,
}

#[napi(object)]
pub struct OhJoinedSpace {
    pub sponsor_device_id: String,
    pub sponsor_identity_fingerprint: String,
    pub space_id: String,
    pub self_device_id: String,
    pub self_identity_fingerprint: String,
    pub migrated_records: Option<String>,
    pub preserved_unreadable_records: Option<String>,
}

#[napi(object)]
pub struct OhJoinSpaceStatus {
    pub status: String,
    pub join_id: String,
    pub joined_space: Option<OhJoinedSpace>,
    pub target_space_id: Option<String>,
    pub sponsor_device_id: Option<String>,
    pub sponsor_identity_fingerprint: Option<String>,
    pub cancel_requested: Option<bool>,
    pub rejection_reason: Option<String>,
}

#[napi(object)]
pub struct OhSendReport {
    pub entry_id: String,
    pub at_ms: f64,
    pub total_accepted: u32,
    pub total_duplicate: u32,
    pub total_offline: u32,
    pub total_errored: u32,
    pub total_pending: u32,
}

#[napi(object)]
pub struct OhEngineEvent {
    pub kind: String,
    pub state: Option<String>,
    pub refresh_reason: Option<String>,
    pub operation_id: Option<String>,
    pub terminal: Option<String>,
    pub lifecycle_action: Option<String>,
    pub error_code: Option<u32>,
    pub error_category: Option<String>,
    pub retryable: Option<bool>,
    pub workspace_convergence: Option<OhWorkspaceConvergence>,
    pub device_trust_revision: Option<f64>,
    pub network_recovery_phase: Option<String>,
    pub next_retry_in_ms: Option<f64>,
}

pub struct PreparedHost {
    host: Option<OhHost>,
}

#[napi]
pub fn core_version() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

#[napi]
pub fn prepare_host(env: Env, mut host: OhHost) -> napi::Result<External<PreparedHost>> {
    host::unref_callbacks(&mut host, &env)?;
    Ok(External::new(PreparedHost { host: Some(host) }))
}

#[napi]
pub async fn start_engine(
    config: OhEngineConfig,
    mut prepared_host: External<PreparedHost>,
) -> napi::Result<OhEngine> {
    let host = prepared_host.host.take().ok_or_else(|| {
        napi::Error::new(
            napi::Status::InvalidArg,
            "OHOS_HOST_ALREADY_CONSUMED".to_owned(),
        )
    })?;
    OhEngine::start(config, host).await
}

#[cfg(target_env = "ohos")]
mod ohos_registration {
    #[napi::bindgen_prelude::ctor]
    fn register_ohos_napi_module() {
        const MODULE_NAME: &[u8] = b"uc_ohos_napi\0";
        static mut MODULE: napi::sys::napi_module = napi::sys::napi_module {
            nm_version: 1,
            nm_flags: 0,
            nm_filename: std::ptr::null(),
            nm_register_func: Some(napi::bindgen_prelude::napi_register_module_v1),
            nm_modname: MODULE_NAME.as_ptr().cast(),
            nm_priv: std::ptr::null_mut(),
            reserved: [std::ptr::null_mut(); 4],
        };

        // HarmonyOS discovers N-API exports through constructor-time registration.
        unsafe {
            napi::sys::napi_module_register(&raw mut MODULE);
        }
    }
}
