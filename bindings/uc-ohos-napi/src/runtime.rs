use std::sync::Arc;
use std::time::Duration;

use napi::bindgen_prelude::Buffer;
use napi::Status;
use napi_derive::napi;
use uc_engine::{
    ClipboardRestoreMode, ClipboardRestoreOutcome, CreateSpaceInput, Engine, EngineConfig,
    EngineError, EngineEvent, EngineState, EventStream, ExportEntryInput, HostFileHandle,
    InvitationAvailability, JoinSpaceInput, Operation, OperationResult, OperationTerminal,
    QuerySharedDeviceRefreshInput, RecoverSessionInput, RefreshReason, RemoveMemberInput,
    RestoreClipboardInput, SecretString, SendFilesInput, SendImageInput, SendReportSummary,
    SendTextInput,
};
use zeroize::Zeroizing;

use crate::{
    host, OhActiveClipboard, OhEngineConfig, OhEngineEvent, OhHost, OhInvitationIssued,
    OhLocalDevice, OhMemberRemoval, OhMembershipConvergence, OhNetworkRecoveryStatus, OhSendReport,
    OhSessionRecovery, OhSharedDeviceRefresh, OhSharedDeviceRefreshDevice,
    OhSharedDeviceRefreshStarted, OhSpaceCreated, OhSpaceJoined,
};

#[napi]
pub struct OhEngine {
    engine: Arc<Engine>,
    events: tokio::sync::Mutex<EventStream>,
}

impl OhEngine {
    pub(crate) async fn start(config: OhEngineConfig, host: OhHost) -> napi::Result<Self> {
        let capabilities = host::capabilities(host)?;
        let config = EngineConfig::new(config.app_version).with_profile_id(config.profile_id);
        let (engine, events) = Engine::start(config, capabilities)
            .await
            .map_err(engine_error)?;
        Ok(Self {
            engine: Arc::new(engine),
            events: tokio::sync::Mutex::new(events),
        })
    }
}

#[napi]
impl OhEngine {
    #[napi]
    pub async fn create_space(
        &self,
        device_name: Option<String>,
        passphrase: String,
    ) -> napi::Result<OhSpaceCreated> {
        let passphrase = Zeroizing::new(passphrase);
        let result = self
            .engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name,
                passphrase: SecretString::new(passphrase.as_str()),
                passphrase_confirmation: SecretString::new(passphrase.as_str()),
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::SpaceCreated {
                space_id,
                self_device_id,
                identity_fingerprint,
            } => Ok(OhSpaceCreated {
                space_id,
                self_device_id,
                identity_fingerprint,
            }),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn recover_session(
        &self,
        allow_secure_storage_unlock: bool,
    ) -> napi::Result<OhSessionRecovery> {
        let result = self
            .engine
            .execute(Operation::RecoverSession(RecoverSessionInput {
                allow_secure_storage_unlock,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::SessionRecovered { unlocked, resumed } => {
                Ok(OhSessionRecovery { unlocked, resumed })
            }
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn recover_network(&self) -> napi::Result<()> {
        match self
            .engine
            .execute(Operation::RecoverNetwork)
            .await
            .map_err(engine_error)?
        {
            OperationResult::NetworkRecovered => Ok(()),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_network_recovery_status(&self) -> napi::Result<OhNetworkRecoveryStatus> {
        match self
            .engine
            .execute(Operation::QueryNetworkRecoveryStatus)
            .await
            .map_err(engine_error)?
        {
            OperationResult::NetworkRecoveryStatus(status) => Ok(OhNetworkRecoveryStatus {
                phase: recovery_phase(status.phase).to_string(),
                retryable: status.retryable,
                next_retry_in_ms: status.next_retry_in_ms.map(|value| value as f64),
            }),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_local_device(&self) -> napi::Result<OhLocalDevice> {
        let result = self
            .engine
            .execute(Operation::QueryLocalDevice)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::LocalDevice(device) => Ok(OhLocalDevice {
                device_id: device.device_id,
                display_name: device.display_name,
            }),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_membership_convergence(&self) -> napi::Result<OhMembershipConvergence> {
        let result = self
            .engine
            .execute(Operation::QueryMembershipConvergence)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::MembershipConvergence(summary) => membership_convergence(summary),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn refresh_shared_devices(&self) -> napi::Result<OhSharedDeviceRefreshStarted> {
        let result = self
            .engine
            .execute(Operation::RefreshSharedDevices)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::SharedDeviceRefreshStarted(summary) => {
                Ok(OhSharedDeviceRefreshStarted {
                    request_id: summary.request_id,
                })
            }
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_shared_device_refresh(
        &self,
        request_id: String,
    ) -> napi::Result<Option<OhSharedDeviceRefresh>> {
        let result = self
            .engine
            .execute(Operation::QuerySharedDeviceRefresh(
                QuerySharedDeviceRefreshInput { request_id },
            ))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::SharedDeviceRefresh(summary) => {
                summary.map(shared_device_refresh).transpose()
            }
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn remove_member(&self, device_id: String) -> napi::Result<OhMemberRemoval> {
        let result = self
            .engine
            .execute(Operation::RemoveMember(RemoveMemberInput { device_id }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::MemberRemoved(summary) => member_removal(summary),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_member_removal(&self) -> napi::Result<OhMemberRemoval> {
        let result = self
            .engine
            .execute(Operation::QueryMemberRemoval)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::MemberRemovalStatus(summary) => member_removal(summary),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn issue_invitation(&self) -> napi::Result<OhInvitationIssued> {
        let result = self
            .engine
            .execute(Operation::IssueInvitation)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::InvitationIssued {
                invitation_code,
                expires_at_ms,
                availability,
            } => Ok(OhInvitationIssued {
                invitation_code,
                expires_at_ms: expires_at_ms as f64,
                availability: invitation_availability(availability).to_owned(),
            }),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn join_space(
        &self,
        invitation_code: String,
        device_name: Option<String>,
        passphrase: String,
        preserve_unreadable_history: bool,
    ) -> napi::Result<OhSpaceJoined> {
        let invitation_code = Zeroizing::new(invitation_code);
        let passphrase = Zeroizing::new(passphrase);
        let result = self
            .engine
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code: invitation_code.to_string(),
                device_name,
                passphrase: SecretString::new(passphrase.as_str()),
                preserve_unreadable_history,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::SpaceJoined {
                sponsor_device_id,
                sponsor_identity_fingerprint,
                space_id,
                self_device_id,
                self_identity_fingerprint,
                migrated_records,
                preserved_unreadable_records,
            } => Ok(OhSpaceJoined {
                sponsor_device_id,
                sponsor_identity_fingerprint,
                space_id,
                self_device_id,
                self_identity_fingerprint,
                migrated_records: migrated_records.map(|count| count.to_string()),
                preserved_unreadable_records: preserved_unreadable_records
                    .map(|count| count.to_string()),
            }),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn send_text(
        &self,
        text: String,
        target_devices: Vec<String>,
    ) -> napi::Result<OhSendReport> {
        let text = Zeroizing::new(text);
        let result = self
            .engine
            .execute(Operation::SendText(SendTextInput {
                text: text.to_string(),
                target_devices,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::EntrySent(report) => send_report(report),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn send_image(
        &self,
        bytes: Buffer,
        mime_type: String,
        target_devices: Vec<String>,
    ) -> napi::Result<OhSendReport> {
        let bytes = Zeroizing::new(bytes.to_vec());
        let result = self
            .engine
            .execute(Operation::SendImage(SendImageInput {
                bytes: bytes.to_vec(),
                mime_type,
                target_devices,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::EntrySent(report) => send_report(report),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn send_files(
        &self,
        file_handles: Vec<String>,
        target_devices: Vec<String>,
    ) -> napi::Result<OhSendReport> {
        let file_handles = Zeroizing::new(file_handles);
        let result = self
            .engine
            .execute(Operation::SendFiles(SendFilesInput {
                files: file_handles
                    .iter()
                    .cloned()
                    .map(HostFileHandle::new)
                    .collect(),
                target_devices,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::EntrySent(report) => send_report(report),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn capture_current_clipboard(&self) -> napi::Result<Option<String>> {
        let result = self
            .engine
            .execute(Operation::CaptureCurrentClipboard)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::ClipboardCaptured { entry_id } => Ok(entry_id),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn query_active_clipboard(&self) -> napi::Result<Option<OhActiveClipboard>> {
        let result = self
            .engine
            .execute(Operation::QueryActiveClipboard)
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::ActiveClipboard(active) => {
                Ok(active.map(|active| OhActiveClipboard {
                    entry_id: active.entry_id,
                    activated_by: active.activated_by,
                }))
            }
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn restore_clipboard(&self, entry_id: String, mode: String) -> napi::Result<String> {
        let mode = match mode.as_str() {
            "standard" => ClipboardRestoreMode::Standard,
            "plain_text" => ClipboardRestoreMode::PlainText,
            "file_paths" => ClipboardRestoreMode::FilePaths,
            _ => return Err(invalid_restore_mode()),
        };
        let result = self
            .engine
            .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                entry_id,
                mode,
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::ClipboardRestored(ClipboardRestoreOutcome::Restored) => {
                Ok("restored".to_owned())
            }
            OperationResult::ClipboardRestored(ClipboardRestoreOutcome::PayloadUnavailable {
                ..
            }) => Ok("payload_unavailable".to_owned()),
            OperationResult::ClipboardRestored(ClipboardRestoreOutcome::NotApplicable {
                ..
            }) => Ok("not_applicable".to_owned()),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn export_entry(
        &self,
        entry_id: String,
        destination_handle: String,
    ) -> napi::Result<()> {
        let destination_handle = Zeroizing::new(destination_handle);
        let result = self
            .engine
            .execute(Operation::ExportEntry(ExportEntryInput {
                entry_id,
                destination: HostFileHandle::new(destination_handle.to_string()),
            }))
            .await
            .map_err(engine_error)?;
        match result {
            OperationResult::EntryExported => Ok(()),
            _ => Err(unexpected_result()),
        }
    }

    #[napi]
    pub async fn suspend(&self) -> napi::Result<()> {
        self.engine.suspend().await.map_err(engine_error)
    }

    #[napi]
    pub async fn lifecycle_state(&self) -> String {
        engine_state(self.engine.lifecycle_state().await).to_owned()
    }

    #[napi]
    pub async fn resume(&self) -> napi::Result<()> {
        self.engine.resume().await.map_err(engine_error)
    }

    #[napi]
    pub async fn next_event(&self, timeout_ms: u32) -> napi::Result<Option<OhEngineEvent>> {
        let mut events = self.events.lock().await;
        match tokio::time::timeout(Duration::from_millis(u64::from(timeout_ms)), events.next())
            .await
        {
            Ok(Some(event)) => Ok(Some(map_event(event))),
            Ok(None) | Err(_) => Ok(None),
        }
    }

    #[napi]
    pub async fn shutdown(&self, deadline_ms: u32) -> napi::Result<()> {
        self.engine
            .shutdown(Duration::from_millis(u64::from(deadline_ms)))
            .await
            .map_err(engine_error)
    }
}

fn member_removal(summary: uc_engine::MemberRemovalSummary) -> napi::Result<OhMemberRemoval> {
    Ok(OhMemberRemoval {
        phase: match summary.phase {
            uc_engine::MemberRemovalPhase::Applied => "applied",
            uc_engine::MemberRemovalPhase::Converging => "converging",
            uc_engine::MemberRemovalPhase::Complete => "complete",
            uc_engine::MemberRemovalPhase::RecoveryRequired => "recovery_required",
        }
        .to_owned(),
        intent_count: count_u64(summary.intent_count)?,
        effective_member_count: count_u64(summary.effective_member_count)?,
        convergence_digest: summary.convergence_digest,
        updated_at_ms: summary.updated_at_ms as f64,
    })
}

fn engine_error(error: EngineError) -> napi::Error {
    napi::Error::new(
        Status::GenericFailure,
        format!(
            "UC_ENGINE:{}:{}:{}",
            error.code(),
            error.category(),
            error.is_retryable()
        ),
    )
}

fn invitation_availability(availability: InvitationAvailability) -> &'static str {
    match availability {
        InvitationAvailability::CrossNetwork => "cross_network",
        InvitationAvailability::SameLocalNetwork => "same_local_network",
    }
}

fn send_report(report: SendReportSummary) -> napi::Result<OhSendReport> {
    Ok(OhSendReport {
        entry_id: report.entry_id,
        at_ms: report.at_ms as f64,
        total_accepted: count(report.total_accepted)?,
        total_duplicate: count(report.total_duplicate)?,
        total_offline: count(report.total_offline)?,
        total_errored: count(report.total_errored)?,
        total_pending: count(report.total_pending)?,
    })
}

fn count(value: usize) -> napi::Result<u32> {
    u32::try_from(value).map_err(|_| unexpected_result())
}

fn membership_convergence(
    summary: uc_engine::MembershipConvergenceSummary,
) -> napi::Result<OhMembershipConvergence> {
    Ok(OhMembershipConvergence {
        state: match summary.state {
            uc_engine::MembershipConvergenceStateSummary::Complete => "complete",
            uc_engine::MembershipConvergenceStateSummary::Converging => "converging",
            uc_engine::MembershipConvergenceStateSummary::WaitingForUpgrade => {
                "waiting_for_upgrade"
            }
            uc_engine::MembershipConvergenceStateSummary::Blocked => "blocked",
        }
        .to_owned(),
        pending_count: count_u64(summary.pending_count)?,
        waiting_for_peer_count: count_u64(summary.waiting_for_peer_count)?,
        waiting_for_update_count: count_u64(summary.waiting_for_update_count)?,
        version_incompatible_count: count_u64(summary.version_incompatible_count)?,
        blocked_count: count_u64(summary.blocked_count)?,
        rejected_count: count_u64(summary.rejected_count)?,
    })
}

fn shared_device_refresh(
    summary: uc_engine::SharedDeviceRefreshSummary,
) -> napi::Result<OhSharedDeviceRefresh> {
    Ok(OhSharedDeviceRefresh {
        request_id: summary.request_id,
        phase: match summary.phase {
            uc_engine::SharedDeviceRefreshPhaseSummary::Started => "started",
            uc_engine::SharedDeviceRefreshPhaseSummary::Discovering => "discovering",
            uc_engine::SharedDeviceRefreshPhaseSummary::Connecting => "connecting",
            uc_engine::SharedDeviceRefreshPhaseSummary::RoundCompleted => "round_completed",
        }
        .to_owned(),
        devices: summary
            .devices
            .into_iter()
            .map(|device| {
                Ok(OhSharedDeviceRefreshDevice {
                    device_id: device.device_id,
                    display_name: device.display_name,
                    state: match device.state {
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::Discovered => {
                            "discovered"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::Connecting => {
                            "connecting"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::Connected => "connected",
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::AlreadyPresent => {
                            "already_present"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::WaitingForPeer => {
                            "waiting_for_peer"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::WaitingForUpdate => {
                            "waiting_for_update"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::VersionIncompatible => {
                            "version_incompatible"
                        }
                        uc_engine::SharedDeviceRefreshDeviceStateSummary::Rejected => "rejected",
                    }
                    .to_owned(),
                })
            })
            .collect::<napi::Result<Vec<_>>>()?,
        total_count: count_u64(summary.total_count)?,
        discovered_count: count_u64(summary.discovered_count)?,
        connecting_count: count_u64(summary.connecting_count)?,
        connected_count: count_u64(summary.connected_count)?,
        already_present_count: count_u64(summary.already_present_count)?,
        waiting_for_peer_count: count_u64(summary.waiting_for_peer_count)?,
        waiting_for_update_count: count_u64(summary.waiting_for_update_count)?,
        version_incompatible_count: count_u64(summary.version_incompatible_count)?,
        rejected_count: count_u64(summary.rejected_count)?,
        unavailable_source_count: count_u64(summary.unavailable_source_count)?,
    })
}

fn count_u64(value: u64) -> napi::Result<u32> {
    u32::try_from(value).map_err(|_| unexpected_result())
}

fn map_event(event: EngineEvent) -> OhEngineEvent {
    let kind = event.kind().to_owned();
    let mut mapped = OhEngineEvent {
        kind,
        state: None,
        refresh_reason: None,
        operation_id: None,
        terminal: None,
        lifecycle_action: None,
        error_code: None,
        error_category: None,
        retryable: None,
        member_removal: None,
        shared_device_refresh: None,
        network_recovery_phase: None,
        next_retry_in_ms: None,
    };
    match event {
        EngineEvent::StateChanged { state } => mapped.state = Some(engine_state(state).to_owned()),
        EngineEvent::RefreshRequired { reason } => {
            mapped.refresh_reason = Some(refresh_reason(reason).to_owned());
        }
        EngineEvent::OperationFinished {
            operation_id,
            terminal,
        } => {
            mapped.operation_id = Some(operation_id);
            map_terminal(terminal, &mut mapped);
        }
        EngineEvent::LifecycleFailed { action, error } => {
            mapped.lifecycle_action = Some(
                match action {
                    uc_engine::LifecycleAction::Suspend => "suspend",
                    uc_engine::LifecycleAction::Resume => "resume",
                }
                .to_owned(),
            );
            map_event_error(error, &mut mapped);
        }
        EngineEvent::Fatal { error } => map_event_error(error, &mut mapped),
        EngineEvent::MemberRemovalChanged(summary) => {
            mapped.member_removal = member_removal(summary).ok();
        }
        EngineEvent::SharedDeviceRefreshChanged(summary) => {
            mapped.shared_device_refresh = shared_device_refresh(summary).ok();
        }
        EngineEvent::NetworkRecoveryChanged(status) => {
            mapped.network_recovery_phase = Some(recovery_phase(status.phase).to_owned());
            mapped.retryable = Some(status.retryable);
            mapped.next_retry_in_ms = status.next_retry_in_ms.map(|value| value as f64);
        }
        _ => {}
    }
    mapped
}

fn recovery_phase(phase: uc_engine::NetworkRecoveryPhaseSummary) -> &'static str {
    match phase {
        uc_engine::NetworkRecoveryPhaseSummary::Idle => "idle",
        uc_engine::NetworkRecoveryPhaseSummary::Recovering => "recovering",
        uc_engine::NetworkRecoveryPhaseSummary::RetryScheduled => "retry_scheduled",
        uc_engine::NetworkRecoveryPhaseSummary::Failed => "failed",
    }
}

fn engine_state(state: EngineState) -> &'static str {
    match state {
        EngineState::Running => "running",
        EngineState::Quiescing => "quiescing",
        EngineState::Quiesced => "quiesced",
        EngineState::Suspended => "suspended",
        EngineState::ShuttingDown => "shutting_down",
        EngineState::Stopped => "stopped",
    }
}

fn refresh_reason(reason: RefreshReason) -> &'static str {
    match reason {
        RefreshReason::ConsumerLagged => "consumer_lagged",
        RefreshReason::StateInvalidated => "state_invalidated",
    }
}

fn map_terminal(terminal: OperationTerminal, mapped: &mut OhEngineEvent) {
    match terminal {
        OperationTerminal::Succeeded => mapped.terminal = Some("succeeded".to_owned()),
        OperationTerminal::Cancelled => mapped.terminal = Some("cancelled".to_owned()),
        OperationTerminal::Failed(error) => {
            mapped.terminal = Some("failed".to_owned());
            map_event_error(error, mapped);
        }
    }
}

fn map_event_error(error: EngineError, mapped: &mut OhEngineEvent) {
    mapped.error_code = Some(error.code());
    mapped.error_category = Some(error.category().to_string());
    mapped.retryable = Some(error.is_retryable());
}

fn unexpected_result() -> napi::Error {
    napi::Error::new(Status::GenericFailure, "UC_ENGINE:UNEXPECTED_RESULT")
}

fn invalid_restore_mode() -> napi::Error {
    napi::Error::new(Status::InvalidArg, "OHOS_INVALID_CLIPBOARD_RESTORE_MODE")
}

#[cfg(test)]
mod tests {
    use uc_engine::{
        EngineError, EngineErrorCategory, EngineEvent, OperationTerminal, RefreshReason,
    };

    use super::{count, map_event, membership_convergence};

    #[test]
    fn member_removal_event_keeps_the_complete_snapshot() {
        let event = map_event(EngineEvent::MemberRemovalChanged(
            uc_engine::MemberRemovalSummary {
                phase: uc_engine::MemberRemovalPhase::Converging,
                intent_count: 2,
                effective_member_count: 1,
                convergence_digest: Some("digest-1".into()),
                updated_at_ms: 42,
                removed: true,
            },
        ));

        assert_eq!(event.kind, "member_removal_changed");
        let removal = event.member_removal.unwrap();
        assert_eq!(removal.phase, "converging");
        assert_eq!(removal.intent_count, 2);
        assert_eq!(removal.effective_member_count, 1);
        assert_eq!(removal.convergence_digest.as_deref(), Some("digest-1"));
        assert_eq!(removal.updated_at_ms, 42.0);
    }

    #[test]
    fn shared_device_refresh_event_keeps_the_complete_snapshot() {
        let event = map_event(EngineEvent::SharedDeviceRefreshChanged(
            uc_engine::SharedDeviceRefreshSummary {
                request_id: "refresh-1".into(),
                phase: uc_engine::SharedDeviceRefreshPhaseSummary::RoundCompleted,
                devices: vec![uc_engine::SharedDeviceRefreshDeviceSummary {
                    device_id: "device-c".into(),
                    display_name: "Device C".into(),
                    state: uc_engine::SharedDeviceRefreshDeviceStateSummary::Connected,
                }],
                total_count: 1,
                discovered_count: 0,
                connecting_count: 0,
                connected_count: 1,
                already_present_count: 0,
                waiting_for_peer_count: 0,
                waiting_for_update_count: 0,
                version_incompatible_count: 0,
                rejected_count: 0,
                unavailable_source_count: 2,
            },
        ));

        assert_eq!(event.kind, "shared_device_refresh_changed");
        let refresh = event.shared_device_refresh.unwrap();
        assert_eq!(refresh.request_id, "refresh-1");
        assert_eq!(refresh.phase, "round_completed");
        assert_eq!(
            refresh
                .devices
                .into_iter()
                .map(|device| (device.device_id, device.display_name, device.state))
                .collect::<Vec<_>>(),
            vec![("device-c".into(), "Device C".into(), "connected".into(),)]
        );
        assert_eq!(refresh.total_count, 1);
        assert_eq!(refresh.connected_count, 1);
        assert_eq!(refresh.unavailable_source_count, 2);
    }

    #[test]
    fn refresh_event_keeps_only_the_stable_reason() {
        let event = map_event(EngineEvent::RefreshRequired {
            reason: RefreshReason::ConsumerLagged,
        });

        assert_eq!(event.kind, "refresh_required");
        assert_eq!(event.refresh_reason.as_deref(), Some("consumer_lagged"));
        assert_eq!(event.operation_id, None);
        assert_eq!(event.error_code, None);
    }

    #[test]
    fn network_recovery_event_keeps_the_stable_status() {
        let event = map_event(EngineEvent::NetworkRecoveryChanged(
            uc_engine::NetworkRecoveryStatusSummary {
                phase: uc_engine::NetworkRecoveryPhaseSummary::RetryScheduled,
                retryable: true,
                next_retry_in_ms: Some(500),
            },
        ));

        assert_eq!(event.kind, "network_recovery_changed");
        assert_eq!(
            event.network_recovery_phase.as_deref(),
            Some("retry_scheduled")
        );
        assert_eq!(event.retryable, Some(true));
        assert_eq!(event.next_retry_in_ms, Some(500.0));
    }

    #[test]
    fn failed_operation_event_keeps_only_the_stable_error_summary() {
        let event = map_event(EngineEvent::OperationFinished {
            operation_id: "operation-1".to_owned(),
            terminal: OperationTerminal::Failed(EngineError::new(
                1214,
                EngineErrorCategory::Unavailable,
                true,
            )),
        });

        assert_eq!(event.kind, "operation_finished");
        assert_eq!(event.operation_id.as_deref(), Some("operation-1"));
        assert_eq!(event.terminal.as_deref(), Some("failed"));
        assert_eq!(event.error_code, Some(1214));
        assert_eq!(event.error_category.as_deref(), Some("unavailable"));
        assert_eq!(event.retryable, Some(true));
    }

    #[test]
    fn lifecycle_failure_event_keeps_the_action_and_stable_error_summary() {
        let event = map_event(EngineEvent::LifecycleFailed {
            action: uc_engine::LifecycleAction::Resume,
            error: EngineError::new(1214, EngineErrorCategory::Unavailable, true),
        });

        assert_eq!(event.kind, "lifecycle_failed");
        assert_eq!(event.lifecycle_action.as_deref(), Some("resume"));
        assert_eq!(event.error_code, Some(1214));
        assert_eq!(event.error_category.as_deref(), Some("unavailable"));
        assert_eq!(event.retryable, Some(true));
    }

    #[test]
    fn oversized_delivery_counts_are_rejected() {
        assert!(count(usize::MAX).is_err());
    }

    #[test]
    fn membership_convergence_maps_state_and_counts() {
        let status = membership_convergence(uc_engine::MembershipConvergenceSummary {
            state: uc_engine::MembershipConvergenceStateSummary::WaitingForUpgrade,
            pending_count: 7,
            waiting_for_peer_count: 2,
            waiting_for_update_count: 1,
            version_incompatible_count: 3,
            blocked_count: 0,
            rejected_count: 1,
        })
        .expect("membership convergence must map");

        assert_eq!(status.state, "waiting_for_upgrade");
        assert_eq!(status.pending_count, 7);
        assert_eq!(status.version_incompatible_count, 3);
    }
}
