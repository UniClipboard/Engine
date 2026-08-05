use std::time::Duration;

#[cfg(feature = "dev-tools")]
use super::operation_error_with_code;
use super::ProductionRuntime;
use crate::engine::EngineRuntime;
use crate::operations::clipboard::capture::execute_capture_current_clipboard;
use crate::operations::clipboard::query_active::execute_query_active_clipboard;
use crate::operations::clipboard::restore::execute_restore_clipboard;
use crate::operations::device::device::execute_query_local_device;
use crate::operations::device::member::{
    execute_continue_member_revocation, execute_list_devices,
    execute_query_current_member_revocation, execute_query_legacy_bootstrap,
    execute_query_member_revocation, execute_query_member_sync_preferences,
    execute_query_membership_convergence, execute_query_space_protection, execute_remove_member,
    execute_secure_remove_legacy_member, execute_update_member_sync_preferences,
};
use crate::operations::device::peer_connections::{
    execute_query_peer_connections, execute_refresh_peer_connections,
};
use crate::operations::history::delivery::execute_query_entry_delivery;
use crate::operations::history::history::{
    execute_clear_history, execute_delete_history_entry, execute_get_history_entry,
    execute_get_history_entry_resource, execute_list_history_entries, execute_query_history_stats,
    execute_set_history_entry_favorite,
};
use crate::operations::history::receive::{
    execute_cancel_entry_receive, execute_cancel_inbound_transfer,
    execute_list_entry_receive_progress, execute_query_entry_receive_progress,
};
use crate::operations::history::resend::execute_resend_entry;
use crate::operations::history::resource::{
    execute_read_blob, execute_read_entry_file, execute_read_thumbnail,
};
use crate::operations::history::search::{
    execute_query_search_status, execute_query_search_tags, execute_rebuild_search_index,
    execute_search_entries, history_page_result, history_search_input, map_query_history_error,
};
use crate::operations::settings::config_migration::{
    execute_export_config, execute_preview_config_import, execute_stage_config_import,
};
use crate::operations::settings::diagnostics::{
    execute_export_diagnostic_logs, execute_query_diagnostics, execute_update_debug_mode,
};
use crate::operations::settings::encryption::{
    execute_lock_encryption, execute_query_encryption_state, execute_verify_secure_storage_access,
};
use crate::operations::settings::migration_progress::execute_query_migration_progress;
use crate::operations::settings::settings::{
    execute_probe_relay, execute_query_relay_credential, execute_query_settings,
    execute_save_relay, execute_update_settings,
};
use crate::operations::settings::storage::{
    execute_clear_storage_cache, execute_query_storage_stats,
};
use crate::operations::settings::upgrade::{
    execute_acknowledge_upgrade, execute_query_upgrade_status,
};
use crate::operations::space::cancel_invitation::execute_cancel_invitation;
use crate::operations::space::create_space::execute_create_space;
use crate::operations::space::factory_reset::execute_factory_reset_space;
use crate::operations::space::invitation::execute_issue_invitation;
use crate::operations::space::join_space::execute_join_space;
use crate::operations::space::reset_space::execute_reset_space;
use crate::operations::space::session_recovery::execute_recover_session;
use crate::operations::space::setup_state::execute_query_setup_state;
use crate::operations::space::unlock::execute_unlock_space;
use crate::{EngineError, Operation, OperationResult};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uc_application::receive_reconciliation::EnsureReceiveReadyPort;

#[async_trait]
impl EngineRuntime for ProductionRuntime {
    async fn execute(
        &self,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        match operation {
            Operation::RecoverNetwork => {
                return self
                    .network_recovery
                    .request_recovery()
                    .await
                    .map(|()| OperationResult::NetworkRecovered)
                    .map_err(|_| {
                        EngineError::new(1105, crate::EngineErrorCategory::Unavailable, true)
                    });
            }
            Operation::QueryNetworkRecoveryStatus => {
                let status = self.network_recovery.status().await;
                return Ok(OperationResult::NetworkRecoveryStatus(
                    crate::NetworkRecoveryStatusSummary {
                        phase: match status.phase {
                            uc_application::facade::NetworkRecoveryPhase::Idle => {
                                crate::NetworkRecoveryPhaseSummary::Idle
                            }
                            uc_application::facade::NetworkRecoveryPhase::Recovering => {
                                crate::NetworkRecoveryPhaseSummary::Recovering
                            }
                            uc_application::facade::NetworkRecoveryPhase::RetryScheduled => {
                                crate::NetworkRecoveryPhaseSummary::RetryScheduled
                            }
                            uc_application::facade::NetworkRecoveryPhase::Failed => {
                                crate::NetworkRecoveryPhaseSummary::Failed
                            }
                            uc_application::facade::NetworkRecoveryPhase::Stopped => {
                                crate::NetworkRecoveryPhaseSummary::Failed
                            }
                        },
                        retryable: status.retryable,
                        next_retry_in_ms: status
                            .next_retry_in
                            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64),
                    },
                ));
            }
            _ => {}
        }
        let session_lease = self.session_supervisor.acquire_operation().await?;
        let operation = async {
            match operation {
                Operation::CreateSpace(input) => {
                    execute_create_space(self.current_facade().await?.as_ref(), input).await
                }
                Operation::UnlockSpace(input) => {
                    execute_unlock_space(self.current_facade().await?.as_ref(), input).await
                }
                Operation::RecoverSession(input) => {
                    execute_recover_session(self.current_facade().await?.as_ref(), input).await
                }
                Operation::JoinSpace(input) => {
                    execute_join_space(self.current_facade().await?.as_ref(), input).await
                }
                Operation::IssueInvitation => {
                    execute_issue_invitation(self.current_facade().await?.as_ref()).await
                }
                Operation::CancelInvitation => {
                    execute_cancel_invitation(self.current_facade().await?.as_ref()).await
                }
                Operation::ResetSpace => {
                    execute_reset_space(self.current_facade().await?.as_ref()).await
                }
                Operation::FactoryResetSpace => {
                    execute_factory_reset_space(self.current_facade().await?.as_ref()).await
                }
                Operation::QuerySetupState => {
                    execute_query_setup_state(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryMigrationProgress => {
                    execute_query_migration_progress(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryStorageStats => {
                    execute_query_storage_stats(self.current_facade().await?.as_ref()).await
                }
                Operation::ClearStorageCache => {
                    execute_clear_storage_cache(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryLocalDevice => {
                    execute_query_local_device(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryPeerConnections => {
                    execute_query_peer_connections(self.current_facade().await?.as_ref()).await
                }
                Operation::RefreshPeerConnections => {
                    execute_refresh_peer_connections(self.current_facade().await?.as_ref()).await
                }
                Operation::QuerySettings => {
                    execute_query_settings(self.current_facade().await?.as_ref()).await
                }
                Operation::UpdateSettings(patch) => {
                    execute_update_settings(self.current_facade().await?.as_ref(), *patch).await
                }
                Operation::SaveRelay(input) => {
                    execute_save_relay(self.current_facade().await?.as_ref(), *input).await
                }
                Operation::ProbeRelay(input) => {
                    execute_probe_relay(self.current_facade().await?.as_ref(), input).await
                }
                Operation::QueryRelayCredential(input) => {
                    execute_query_relay_credential(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::QueryUpgradeStatus => {
                    execute_query_upgrade_status(
                        self.current_facade().await?.as_ref(),
                        &self.app_version,
                    )
                    .await
                }
                Operation::AcknowledgeUpgrade => {
                    execute_acknowledge_upgrade(
                        self.current_facade().await?.as_ref(),
                        &self.app_version,
                    )
                    .await
                }
                Operation::QueryDiagnostics => {
                    execute_query_diagnostics(self.current_facade().await?.as_ref()).await
                }
                Operation::UpdateDebugMode(input) => {
                    execute_update_debug_mode(self.current_facade().await?.as_ref(), input).await
                }
                Operation::ExportDiagnosticLogs(input) => {
                    execute_export_diagnostic_logs(
                        self.current_facade().await?.as_ref(),
                        self.files.as_ref(),
                        &self.temporary_dir,
                        input,
                    )
                    .await
                }
                Operation::ExportConfig(input) => {
                    execute_export_config(
                        self.current_facade().await?.as_ref(),
                        self.files.as_ref(),
                        &self.temporary_dir,
                        input,
                    )
                    .await
                }
                Operation::PreviewConfigImport(input) => {
                    execute_preview_config_import(
                        self.current_facade().await?.as_ref(),
                        self.files.as_ref(),
                        &self.temporary_dir,
                        input,
                    )
                    .await
                }
                Operation::StageConfigImport(input) => {
                    execute_stage_config_import(
                        self.current_facade().await?.as_ref(),
                        self.files.as_ref(),
                        &self.temporary_dir,
                        input,
                    )
                    .await
                }
                #[cfg(not(feature = "lan-compat"))]
                Operation::ListMobileDevices
                | Operation::RevokeMobileDevice(_)
                | Operation::AuthenticateMobileRequest(_)
                | Operation::RevalidateMobileCredential(_)
                | Operation::ListMobileLanInterfaces
                | Operation::QueryMobileSyncSettings
                | Operation::UpdateMobileSyncSettings(_)
                | Operation::UpdateMobileLanEndpoint(_)
                | Operation::RegisterMobileDevice(_)
                | Operation::UpdateMobileDevice(_)
                | Operation::CheckMobileContentAvailable(_)
                | Operation::QueryLatestMobileSyncDocument
                | Operation::ApplyMobileSyncDocument(_)
                | Operation::ReadMobileSyncFile(_)
                | Operation::BeginMobileFileUpload(_)
                | Operation::AppendMobileFileUpload(_)
                | Operation::FinishMobileFileUpload(_)
                | Operation::AbortMobileFileUpload(_) => Err(super::operation_unavailable_error()),
                #[cfg(feature = "lan-compat")]
                operation @ (Operation::ListMobileDevices
                | Operation::RevokeMobileDevice(_)
                | Operation::AuthenticateMobileRequest(_)
                | Operation::RevalidateMobileCredential(_)
                | Operation::ListMobileLanInterfaces
                | Operation::QueryMobileSyncSettings
                | Operation::UpdateMobileSyncSettings(_)
                | Operation::UpdateMobileLanEndpoint(_)
                | Operation::RegisterMobileDevice(_)
                | Operation::UpdateMobileDevice(_)
                | Operation::CheckMobileContentAvailable(_)
                | Operation::QueryLatestMobileSyncDocument
                | Operation::ApplyMobileSyncDocument(_)
                | Operation::ReadMobileSyncFile(_)
                | Operation::BeginMobileFileUpload(_)
                | Operation::AppendMobileFileUpload(_)
                | Operation::FinishMobileFileUpload(_)
                | Operation::AbortMobileFileUpload(_)) => {
                    self.execute_lan_compatibility_operation(operation).await
                }
                Operation::QueryReceiveReadiness => {
                    let status = self.file_transfer_lifecycle.receive_readiness_status();
                    Ok(OperationResult::ReceiveReadiness(
                        crate::ReceiveReadinessSummary {
                            ready: status.ready,
                            degraded: status.degraded_reason.is_some(),
                        },
                    ))
                }
                Operation::QueryEncryptionState => {
                    execute_query_encryption_state(self.current_facade().await?.as_ref()).await
                }
                Operation::LockEncryption => {
                    execute_lock_encryption(self.current_facade().await?.as_ref()).await
                }
                Operation::VerifySecureStorageAccess => {
                    execute_verify_secure_storage_access(self.current_facade().await?.as_ref())
                        .await
                }
                Operation::ListDevices => {
                    execute_list_devices(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryMembershipConvergence => {
                    execute_query_membership_convergence(self.current_facade().await?.as_ref())
                        .await
                }
                Operation::QueryMemberSyncPreferences(input) => {
                    execute_query_member_sync_preferences(
                        self.current_facade().await?.as_ref(),
                        input,
                    )
                    .await
                }
                Operation::UpdateMemberSyncPreferences(input) => {
                    execute_update_member_sync_preferences(
                        self.current_facade().await?.as_ref(),
                        input,
                    )
                    .await
                }
                Operation::RemoveMember(input) => {
                    execute_remove_member(self.current_facade().await?.as_ref(), input).await
                }
                Operation::SecureRemoveLegacyMember(input) => {
                    execute_secure_remove_legacy_member(
                        self.current_facade().await?.as_ref(),
                        input,
                    )
                    .await
                }
                Operation::QueryMemberRevocation(input) => {
                    execute_query_member_revocation(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::QueryCurrentMemberRevocation => {
                    execute_query_current_member_revocation(self.current_facade().await?.as_ref())
                        .await
                }
                Operation::ContinueMemberRevocation(input) => {
                    execute_continue_member_revocation(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::QueryLegacyBootstrap(input) => {
                    execute_query_legacy_bootstrap(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::QuerySpaceProtection => {
                    execute_query_space_protection(self.current_facade().await?.as_ref()).await
                }
                Operation::SearchEntries(input) => {
                    execute_search_entries(self.current_facade().await?.as_ref(), input).await
                }
                Operation::QuerySearchTags => {
                    execute_query_search_tags(self.current_facade().await?.as_ref()).await
                }
                Operation::QuerySearchStatus => {
                    execute_query_search_status(self.current_facade().await?.as_ref()).await
                }
                Operation::RebuildSearchIndex => {
                    execute_rebuild_search_index(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryHistory(input) => {
                    let search_input = history_search_input(input)?;
                    let offset = search_input.offset;
                    let limit = search_input.limit;
                    let page = self
                        .current_facade()
                        .await?
                        .search_query(search_input)
                        .await
                        .map_err(map_query_history_error)?;
                    history_page_result(page, offset, limit)
                }
                Operation::ListHistoryEntries(input) => {
                    execute_list_history_entries(self.current_facade().await?.as_ref(), input).await
                }
                Operation::GetHistoryEntry(input) => {
                    execute_get_history_entry(self.current_facade().await?.as_ref(), input).await
                }
                Operation::DeleteHistoryEntry(input) => {
                    execute_delete_history_entry(self.current_facade().await?.as_ref(), input).await
                }
                Operation::SetHistoryEntryFavorite(input) => {
                    execute_set_history_entry_favorite(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::QueryHistoryStats => {
                    execute_query_history_stats(self.current_facade().await?.as_ref()).await
                }
                Operation::GetHistoryEntryResource(input) => {
                    execute_get_history_entry_resource(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::ReadBlob(input) => {
                    execute_read_blob(self.current_facade().await?.as_ref(), input).await
                }
                Operation::ReadThumbnail(input) => {
                    execute_read_thumbnail(self.current_facade().await?.as_ref(), input).await
                }
                Operation::ReadEntryFile(input) => {
                    execute_read_entry_file(self.current_facade().await?.as_ref(), input).await
                }
                Operation::QueryEntryDelivery(input) => {
                    execute_query_entry_delivery(self.current_facade().await?.as_ref(), input).await
                }
                Operation::ClearHistory => {
                    execute_clear_history(self.current_facade().await?.as_ref()).await
                }
                Operation::QueryEntryReceiveProgress(input) => {
                    execute_query_entry_receive_progress(
                        self.current_facade().await?.as_ref(),
                        input,
                    )
                    .await
                }
                Operation::ListEntryReceiveProgress => {
                    execute_list_entry_receive_progress(self.current_facade().await?.as_ref()).await
                }
                Operation::CancelEntryReceive(input) => {
                    execute_cancel_entry_receive(self.current_facade().await?.as_ref(), input).await
                }
                Operation::CancelInboundTransfer(input) => {
                    execute_cancel_inbound_transfer(self.current_facade().await?.as_ref(), input)
                        .await
                }
                Operation::CaptureCurrentClipboard => {
                    execute_capture_current_clipboard(self.current_facade().await?.as_ref()).await
                }
                Operation::ObserveClipboardChange(input) => {
                    Ok(OperationResult::ClipboardChangeObserved {
                        report: self
                            .clipboard_change_runtime
                            .observe_change(input.dispatch)
                            .await?,
                    })
                }
                Operation::QueryActiveClipboard => {
                    execute_query_active_clipboard(self.current_active_clipboard().await?.as_ref())
                        .await
                }
                Operation::RestoreClipboard(input) => {
                    execute_restore_clipboard(self.current_facade().await?.as_ref(), input).await
                }
                Operation::RecoverNetwork | Operation::QueryNetworkRecoveryStatus => {
                    Err(super::operation_unavailable_error())
                }
                Operation::SendText(input) => self.execute_send_text(input).await,
                Operation::SendImage(input) => self.execute_send_image(input).await,
                Operation::SendFiles(input) => self.execute_send_files(input, &cancellation).await,
                Operation::ResendEntry(input) => {
                    execute_resend_entry(
                        self.current_clipboard_sync_runtime().await?.as_ref(),
                        input,
                    )
                    .await
                }
                Operation::ExportEntry(input) => self.execute_export_entry(input).await,
            }
        };
        let session_cancellation = session_lease.cancellation();
        tokio::select! {
            _ = session_cancellation.cancelled() => Err(super::operation_unavailable_error()),
            result = operation => result,
        }
    }

    #[cfg(feature = "dev-tools")]
    async fn execute_dev(
        &self,
        operation: crate::DevOperation,
        _cancellation: CancellationToken,
    ) -> Result<crate::DevOperationResult, EngineError> {
        use tokio_util::bytes::Bytes;
        use uc_application::facade::{FetchBlobCommand, PublishBlobCommand};
        use uc_core::ids::EntryId;
        use uc_core::ports::blob::BlobTicket;

        let facade = self.current_facade().await?;
        match operation {
            crate::DevOperation::SeedText { text } => facade
                .seed_history_text(&text)
                .await
                .map(|entry_id| crate::DevOperationResult::TextSeeded { entry_id })
                .map_err(|error| operation_error_with_code(1903, "seed text", error)),
            crate::DevOperation::CaptureFilePaths { paths } => facade
                .capture_file_paths_for_diagnostics(paths)
                .await
                .map(|captured| {
                    crate::DevOperationResult::FilePathsCaptured(crate::DevCapturedFileSet {
                        entry_id: captured.entry.entry_id,
                        deduplicated: captured.entry.deduplicated,
                        snapshot_hash: captured.entry.snapshot_hash,
                        directory_structure: captured.directory_structure,
                        content_digest_count: captured.content_digest_count,
                        lines: captured
                            .lines
                            .into_iter()
                            .map(|line| crate::DevCapturedFileSetLine {
                                line_index: line.line_index,
                                root_index: line.root_index,
                                root_name: line.root_name,
                                relative_path: line.relative_path,
                                member_kind: line.member_kind,
                                line_kind: line.line_kind,
                                exclude_reason: line.exclude_reason,
                            })
                            .collect(),
                    })
                })
                .map_err(|error| operation_error_with_code(1904, "capture file paths", error)),
            crate::DevOperation::ListPairingInvitationAddresses => facade
                .list_pairing_invitation_addresses()
                .await
                .map(|addresses| {
                    crate::DevOperationResult::PairingInvitationAddresses(
                        addresses
                            .into_iter()
                            .map(|address| crate::DevPairingInvitationAddress {
                                ip: address.ip,
                                port: address.port,
                            })
                            .collect(),
                    )
                })
                .map_err(|error| {
                    operation_error_with_code(1905, "list invitation addresses", error)
                }),
            crate::DevOperation::IssueInvitationForAddress { address } => facade
                .issue_pairing_invitation_for_address(address)
                .await
                .map(|invitation| {
                    crate::DevOperationResult::InvitationIssued(crate::DevInvitation {
                        code: invitation.code.to_string(),
                        expires_at_ms: invitation.expires_at.timestamp_millis(),
                    })
                })
                .map_err(|error| operation_error_with_code(1906, "issue invitation", error)),
            crate::DevOperation::PublishBlob { bytes } => facade
                .publish_blob(PublishBlobCommand {
                    plaintext: Bytes::from(bytes),
                    entry_id: None,
                })
                .await
                .map(|published| {
                    crate::DevOperationResult::BlobPublished(crate::DevBlobPublished {
                        ticket: published.ticket.as_bytes().to_vec(),
                        entry_id: published.entry_id.to_string(),
                        plaintext_hash: published.plaintext_hash.as_bytes().to_vec(),
                        digest: published.digest.as_bytes().to_vec(),
                        reused_existing: published.reused_existing,
                    })
                })
                .map_err(|error| operation_error_with_code(1907, "publish blob", error)),
            crate::DevOperation::FetchBlob { ticket, entry_id } => facade
                .fetch_blob(FetchBlobCommand {
                    ticket: BlobTicket::from_bytes(ticket),
                    entry_id: EntryId::from_string(entry_id),
                    transfer_context: None,
                })
                .await
                .map(|fetched| crate::DevOperationResult::BlobFetched {
                    bytes: fetched.plaintext.to_vec(),
                    entry_id: fetched.entry_id.to_string(),
                    plaintext_hash: fetched.plaintext_hash.as_bytes().to_vec(),
                    digest: fetched.digest.as_bytes().to_vec(),
                })
                .map_err(|error| operation_error_with_code(1910, "fetch blob", error)),
        }
    }

    #[cfg(feature = "dev-tools")]
    async fn dev_pairing_outcomes(&self) -> Result<crate::DevPairingOutcomeStream, EngineError> {
        let receiver = self
            .current_facade()
            .await?
            .subscribe_pairing_completion()
            .map_err(|error| {
                operation_error_with_code(1911, "subscribe pairing outcomes", error)
            })?;
        Ok(crate::DevPairingOutcomeStream::new(receiver))
    }

    async fn suspend(&self) -> Result<(), EngineError> {
        self.session_supervisor.suspend().await
    }

    async fn resume(&self) -> Result<(), EngineError> {
        self.session_supervisor.resume().await
    }

    async fn shutdown(&self, deadline: Duration) -> Result<(), EngineError> {
        self.network_recovery.shutdown().await;
        self.suspend().await?;
        self.session_supervisor.close_file_transfers().await?;
        self.task_registry.shutdown(deadline).await;
        if let Err(error) = std::fs::remove_dir_all(&self.clipboard_import_root) {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %error, "failed to remove host clipboard imports");
            }
        }
        Ok(())
    }
}
