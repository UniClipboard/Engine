# Engine 操作跨层调用链对照

> 生成日期：2026-11-05
>
> 权威来源：`crates/uc-engine/src/contract/operation.rs`、`crates/uc-engine/src/runtime/dispatch.rs`、`crates/uc-engine/src/operations/`、`crates/uc-application/src/facade/app_facade.rs`。
> 本文件是可再生导航快照；操作语义、错误码与结果字段以源码和
> [uc-engine 跨平台核心接口](../design-docs/uc-engine-interface.md) 为准。
>
> 重新生成：`node scripts/architecture/render-engine-operation-map.mjs`

当前共 **111** 个操作变体，其中 `dev-tools` 专属 1 个。

## 怎么用

1. 在“契约操作”列找到操作名，它就是 `Operation` 枚举的变体。
2. “适配函数”列是处理该操作的地方，括号里是文件名。
3. “应用入口”列给出接收者类型与方法；`AppFacade` 之外的接收者说明该操作不经过应用聚合 facade。

层职责一句话：`contract` 定义名字；`runtime/dispatch.rs` 决定走哪条路；
`operations/` 在契约与应用之间翻译类型和错误；`AppFacade` 是应用层唯一对外类型；
子 facade 与 use case 拥有业务顺序；`uc-core` 的 port 拥有规则；`uc-infra-*` 提供能力。

“runtime 自持”表示该操作不经过应用 facade，由 Engine 运行期直接实现（含系统剪贴板快照构造）。

## 领域：space（14）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `CreateSpace`(CreateSpaceInput) | `CreateSpace` | `execute_create_space`<br>`lifecycle.rs` | `initialize_space`<br>`SpaceFacade` | `SpaceCreated` |
| `JoinSpace`(JoinSpaceInput) | `JoinSpace` | `execute_join_space`<br>`lifecycle.rs` | `join_space`<br>`SpaceFacade` | `JoinSpace` |
| `CancelJoinSpace`(CancelJoinSpaceInput) | `CancelJoinSpace` | `execute_cancel_join_space`<br>`lifecycle.rs` | `cancel_space_join`<br>`SpaceFacade` | `JoinSpace` |
| `UnlockSpace`(UnlockSpaceInput) | `UnlockSpace` | `execute_unlock_space`<br>`session.rs` | `unlock_space`<br>`SpaceFacade` | `SpaceUnlocked` |
| `RecoverSession`(RecoverSessionInput) | `RecoverSession` | `execute_recover_session`<br>`session.rs` | `recover_space_session`<br>`SpaceFacade` | `SessionRecovered` |
| `ChangeEncryptionPassphrase`(ChangeEncryptionPassphraseInput) | `ChangeEncryptionPassphrase` | `execute_change_encryption_passphrase`<br>`session.rs` | `change_encryption_passphrase`<br>`AppFacade` | `EncryptionPassphraseChanged` |
| `IssueInvitation` | `IssueInvitation` | `execute_issue_invitation`<br>`invitation.rs` | `issue_pairing_invitation`<br>`SpaceFacade` | `InvitationIssued` |
| `CancelInvitation` | `CancelInvitation` | `execute_cancel_invitation`<br>`invitation.rs` | `cancel_invitation`<br>`SpaceFacade` | `InvitationCancelled` |
| `FactoryResetSpace` | `FactoryResetSpace` | `execute_factory_reset_space`<br>`lifecycle.rs` | `execute`<br>`ProfileFactoryResetFacade` | `SpaceFactoryReset` |
| `QuerySetupState` | `QuerySetupState` | `execute_query_setup_state`<br>`lifecycle.rs` | `query_setup_state`<br>`SpaceFacade` | `SetupState` |
| `QueryMembershipReadiness` | `QueryMembershipReadiness` | `execute_query_membership_readiness`<br>`membership.rs` | `query_membership_readiness`<br>`SpaceFacade` | `MembershipReadiness` |
| `QueryDeviceGroupChoices` | `QueryDeviceGroupChoices` | `execute_query_device_group_choices`<br>`device_group_choice.rs` | `query_device_group_choices`<br>`SpaceFacade` | `DeviceGroupChoices` |
| `ChooseDeviceGroup`(ChooseDeviceGroupInput) | `ChooseDeviceGroup` | `execute_choose_device_group`<br>`device_group_choice.rs` | `choose_device_group`<br>`AppFacade` | `DeviceGroupChosen` |
| `QueryMembershipDiagnostics` *(dev-tools)* | `QueryMembershipDiagnostics` | `execute_query_membership_diagnostics`<br>`membership.rs` | `query_membership_diagnostics`<br>`SpaceFacade` | `MembershipDiagnostics` |

## 领域：runtime 自持（18）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `QueryProfileRecovery` | `QueryProfileRecovery` | — | dispatch 内联 | `ProfileRecovery` |
| `ResetSpace` | `ResetSpace` | — | runtime: `session_supervisor.reset_space` | — |
| `NotifyConnectivityOpportunity` | `NotifyConnectivityOpportunity` | — | dispatch: `notify_connectivity_opportunity` | `ConnectivityOpportunityAccepted` |
| `RecoverNetwork` | `RecoverNetwork` | — | runtime: `network_recovery.request_recovery` | `NetworkRecovered` |
| `QueryNetworkRecoveryStatus` | `QueryNetworkRecoveryStatus` | — | runtime: `network_recovery.status` | `NetworkRecoveryStatus` |
| `ListMobileLanInterfaces` | `ListMobileLanInterfaces` | — | dispatch: `collect` | — |
| `UpdateMobileLanEndpoint`(MobileLanEndpointUpdate) | `UpdateMobileLanEndpoint` | — | runtime: `mobile_lan_endpoint.update` | — |
| `BeginMobileFileUpload`(BeginMobileFileUploadInput) | `BeginMobileFileUpload` | — | runtime: `begin_mobile_file_upload` | — |
| `AppendMobileFileUpload`(AppendMobileFileUploadInput) | `AppendMobileFileUpload` | — | runtime: `append_mobile_file_upload` | — |
| `FinishMobileFileUpload`(FinishMobileFileUploadInput) | `FinishMobileFileUpload` | — | runtime: `finish_mobile_file_upload` | — |
| `AbortMobileFileUpload`(AbortMobileFileUploadInput) | `AbortMobileFileUpload` | — | runtime: `abort_mobile_file_upload` | — |
| `QueryReceiveReadiness` | `QueryReceiveReadiness` | — | dispatch: `is_some` | `ReceiveReadiness` |
| `SendText`(SendTextInput) | `SendText` | `execute_send_text`<br>`host_operations/mod.rs` | `send_snapshot`<br>`ProductionRuntime` | — |
| `SendImage`(SendImageInput) | `SendImage` | `execute_send_image`<br>`host_operations/mod.rs` | `send_snapshot`<br>`ProductionRuntime` | — |
| `SendFiles`(SendFilesInput) | `SendFiles` | `execute_send_files`<br>`host_operations/mod.rs` | `import_host_files`<br>`ProductionRuntime` | — |
| `QueryHistory`(QueryHistoryInput) | `QueryHistory` | — | dispatch: `search_query` | — |
| `ObserveClipboardChange`(ObserveClipboardChangeInput) | `ObserveClipboardChange` | — | runtime: `clipboard_change_runtime.observe_change` | `ClipboardChangeObserved` |
| `ExportEntry`(ExportEntryInput) | `ExportEntry` | `execute_export_entry`<br>`export.rs` | runtime 内部<br>`ProductionRuntime` | `EntryExported` |

## 领域：settings（22）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `QueryStorageStats` | `QueryStorageStats` | `execute_query_storage_stats`<br>`maintenance.rs` | `storage_stats`<br>`StorageFacade` | — |
| `ListUpgradeBackups` | `ListUpgradeBackups` | `execute_list_upgrade_backups`<br>`maintenance.rs` | `list_backups`<br>`ProfileUpgradeBackupPort` | `UpgradeBackups` |
| `DeleteUpgradeBackup`(DeleteUpgradeBackupInput) | `DeleteUpgradeBackup` | `execute_delete_upgrade_backup`<br>`maintenance.rs` | `delete_backup`<br>`ProfileUpgradeBackupPort` | `UpgradeBackupDeleted` |
| `ClearStorageCache` | `ClearStorageCache` | `execute_clear_storage_cache`<br>`maintenance.rs` | `clear_storage_cache`<br>`StorageFacade` | `StorageCacheCleared` |
| `QuerySettings` | `QuerySettings` | `execute_query_settings`<br>`settings.rs` | `settings`<br>`SettingsFacade` | `Settings` |
| `QueryCustomRelays` | `QueryCustomRelays` | `execute_query_custom_relays`<br>`relay.rs` | `list_relays`<br>`SettingsFacade` | `CustomRelays` |
| `QueryRelayOverview` | `QueryRelayOverview` | `execute_query_relay_overview`<br>`relay.rs` | `relay_overview`<br>`SettingsFacade` | `RelayOverview` |
| `MutateCustomRelay`(CustomRelayMutation) | `MutateCustomRelay` | `execute_mutate_custom_relay`<br>`relay.rs` | `mutate_relays`<br>`SettingsFacade` | `CustomRelayMutated` |
| `UpdateSettings`(Box<SettingsPatch>) | `UpdateSettings` | `execute_update_settings`<br>`settings.rs` | `update_settings`<br>`SettingsFacade` | `SettingsUpdated` |
| `SaveRelay`(Box<SaveRelayInput>) | `SaveRelay` | `execute_save_relay`<br>`relay.rs` | `save_relay`<br>`SettingsFacade` | `RelaySaved` |
| `ProbeRelay`(RelayProbeInput) | `ProbeRelay` | `execute_probe_relay`<br>`relay.rs` | `probe_relay_url`<br>`SettingsFacade` | `RelayProbed` |
| `QueryRelayCredential`(RelayCredentialInput) | `QueryRelayCredential` | `execute_query_relay_credential`<br>`relay.rs` | `relay_credential_status`<br>`SettingsFacade` | `RelayCredentialStatus` |
| `QueryUpgradeStatus` | `QueryUpgradeStatus` | `execute_query_upgrade_status`<br>`maintenance.rs` | `upgrade_status`<br>`UpgradeFacade` | `UpgradeStatus` |
| `AcknowledgeUpgrade` | `AcknowledgeUpgrade` | `execute_acknowledge_upgrade`<br>`maintenance.rs` | `acknowledge_upgrade`<br>`UpgradeFacade` | `UpgradeAcknowledged` |
| `QueryDiagnostics` | `QueryDiagnostics` | `execute_query_diagnostics`<br>`diagnostics.rs` | `diagnostics_status`<br>`DiagnosticsFacade` | `DiagnosticsStatus` |
| `ExportDiagnosticLogs`(ExportDiagnosticLogsInput) | `ExportDiagnosticLogs` | `execute_export_diagnostic_logs`<br>`diagnostics.rs` | `export_diagnostic_logs`<br>`AppFacade` | `DiagnosticLogsExported` |
| `ExportConfig`(ExportConfigInput) | `ExportConfig` | `execute_export_config`<br>`config_migration.rs` | `export_config`<br>`ConfigMigrationFacade` | `ConfigExport` |
| `PreviewConfigImport`(PreviewConfigImportInput) | `PreviewConfigImport` | `execute_preview_config_import`<br>`config_migration.rs` | `preview_config_import`<br>`ConfigMigrationFacade` | `ConfigImportPreview` |
| `StageConfigImport`(StageConfigImportInput) | `StageConfigImport` | `execute_stage_config_import`<br>`config_migration.rs` | `stage_config_import`<br>`AppFacade` | `ConfigImportStaged` |
| `QueryEncryptionState` | `QueryEncryptionState` | `execute_query_encryption_state`<br>`encryption.rs` | `encryption_state`<br>`SpaceFacade` | `EncryptionState` |
| `LockEncryption` | `LockEncryption` | `execute_lock_encryption`<br>`encryption.rs` | `lock_space_session`<br>`AppFacade` | `EncryptionLocked` |
| `VerifySecureStorageAccess` | `VerifySecureStorageAccess` | `execute_verify_secure_storage_access`<br>`encryption.rs` | `verify_secure_storage_access`<br>`ProbeProfileKeyAccessUseCase` | `SecureStorageAccess` |

## 领域：device（8）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `QueryLocalDevice` | `QueryLocalDevice` | `execute_query_local_device`<br>`connectivity.rs` | `local_device_info`<br>`QueryLocalDeviceUseCase` | `LocalDevice` |
| `QueryPeerConnections` | `QueryPeerConnections` | `execute_query_peer_connections`<br>`connectivity.rs` | `list_peer_snapshots`<br>`SpaceFacade` | `PeerConnections` |
| `RefreshPeerConnections` | `RefreshPeerConnections` | `execute_refresh_peer_connections`<br>`connectivity.rs` | `refresh_peer_reachability`<br>`SpaceFacade` | `PeerConnectionsRefreshed` |
| `ListDevices` | `ListDevices` | `execute_list_devices`<br>`member/mod.rs` | `encryption_state`<br>`SpaceFacade` | `Devices` |
| `QueryMemberSyncPreferences`(QueryMemberSyncPreferencesInput) | `QueryMemberSyncPreferences` | `execute_query_member_sync_preferences`<br>`member/mod.rs` | `member_sync_preferences`<br>`SpaceFacade` | — |
| `UpdateMemberSyncPreferences`(UpdateMemberSyncPreferencesInput) | `UpdateMemberSyncPreferences` | `execute_update_member_sync_preferences`<br>`member/mod.rs` | `update_member_sync_preferences`<br>`AppFacade` | — |
| `RemoveMember`(RemoveMemberInput) | `RemoveMember` | `execute_remove_member`<br>`member/mod.rs` | `remove_space_member`<br>`SpaceFacade` | `DeviceTrust` |
| `QuerySpaceProtection` | `QuerySpaceProtection` | `execute_query_space_protection`<br>`member/mod.rs` | `space_protection`<br>`SpaceFacade` | `SpaceProtection` |

## 领域：compatibility/mobile_lan（12）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `ListMobileDevices` | `ListMobileDevices` | `execute_list_mobile_devices`<br>`operations.rs` | `list_devices`<br>`MobileSyncFacade` | `MobileDevices` |
| `RevokeMobileDevice`(MobileDeviceInput) | `RevokeMobileDevice` | `execute_revoke_mobile_device`<br>`operations.rs` | `revoke_device`<br>`MobileSyncFacade` | `MobileDeviceRevoked` |
| `AuthenticateMobileRequest`(AuthenticateMobileRequestInput) | `AuthenticateMobileRequest` | `execute_authenticate_mobile_request`<br>`operations.rs` | `authenticate_basic`<br>`MobileSyncFacade` | `MobileRequestAuthenticated`, `MobileAuthentication` |
| `RevalidateMobileCredential`(RevalidateMobileCredentialInput) | `RevalidateMobileCredential` | `execute_revalidate_mobile_credential`<br>`operations.rs` | `is_device_credential_current`<br>`MobileSyncFacade` | `MobileCredentialCurrent` |
| `QueryMobileSyncSettings` | `QueryMobileSyncSettings` | `execute_query_mobile_sync_settings`<br>`operations.rs` | `get_settings`<br>`MobileSyncFacade` | `MobileSyncSettings` |
| `UpdateMobileSyncSettings`(Box<MobileSyncSettingsPatch>) | `UpdateMobileSyncSettings` | `execute_update_mobile_sync_settings`<br>`operations.rs` | `update_settings`<br>`MobileSyncFacade` | `MobileSyncSettingsUpdated` |
| `RegisterMobileDevice`(RegisterMobileDeviceInput) | `RegisterMobileDevice` | `execute_register_mobile_device`<br>`operations.rs` | `register_device`<br>`MobileSyncFacade` | `MobileDeviceRegistered` |
| `UpdateMobileDevice`(UpdateMobileDeviceInput) | `UpdateMobileDevice` | `execute_update_mobile_device`<br>`operations.rs` | `update_device`<br>`MobileSyncFacade` | `MobileDeviceUpdated` |
| `CheckMobileContentAvailable`(MobileContentAvailabilityInput) | `CheckMobileContentAvailable` | `execute_check_mobile_content_available`<br>`content_operations.rs` | `check_content_available`<br>`MobileSyncFacade` | `MobileContentAvailability` |
| `QueryLatestMobileSyncDocument` | `QueryLatestMobileSyncDocument` | `execute_query_latest_mobile_sync_document`<br>`content_operations.rs` | `get_latest_sync_doc`<br>`MobileSyncFacade` | `MobileSyncDocument` |
| `ApplyMobileSyncDocument`(Box<ApplyMobileSyncDocumentInput>) | `ApplyMobileSyncDocument` | `execute_apply_mobile_sync_document`<br>`content_operations.rs` | `put_sync_doc`<br>`MobileSyncFacade` | `MobileSyncDocumentApplied` |
| `ReadMobileSyncFile`(ReadMobileSyncFileInput) | `ReadMobileSyncFile` | `execute_read_mobile_sync_file`<br>`content_operations.rs` | `get_clipboard_file`<br>`MobileSyncFacade` | `MobileSyncFile` |

## 领域：history（34）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `SearchEntries`(SearchEntriesInput) | `SearchEntries` | `execute_search_entries`<br>`search.rs` | `search_query`<br>`SearchFacade` | `SearchPage` |
| `CountSearchEntries`(CountSearchEntriesInput) | `CountSearchEntries` | `execute_count_search_entries`<br>`search.rs` | `search_count`<br>`SearchFacade` | `SearchCounts` |
| `QueryDailyEntryCounts`(DailyEntryCountsInput) | `QueryDailyEntryCounts` | `execute_query_daily_entry_counts`<br>`search.rs` | `search_daily_counts`<br>`SearchFacade` | `DailyEntryCounts` |
| `QuerySearchTags` | `QuerySearchTags` | `execute_query_search_tags`<br>`search.rs` | `search_tags`<br>`SearchFacade` | `SearchTags` |
| `QuerySearchStatus` | `QuerySearchStatus` | `execute_query_search_status`<br>`search.rs` | `search_status`<br>`SearchFacade` | `SearchStatus` |
| `RebuildSearchIndex` | `RebuildSearchIndex` | `execute_rebuild_search_index`<br>`search.rs` | `request_search_rebuild`<br>`SearchFacade` | `SearchRebuildAccepted` |
| `ListHistoryEntries`(ListHistoryEntriesInput) | `ListHistoryEntries` | `execute_list_history_entries`<br>`history.rs` | `list_history_entries`<br>`ClipboardHistoryFacade` | `HistoryEntries` |
| `GetHistoryEntry`(HistoryEntryInput) | `GetHistoryEntry` | `execute_get_history_entry`<br>`history.rs` | `get_history_entry`<br>`ClipboardHistoryFacade` | `HistoryEntry` |
| `DeleteHistoryEntry`(HistoryEntryInput) | `DeleteHistoryEntry` | `execute_delete_history_entry`<br>`history.rs` | `delete_history_entry`<br>`ClipboardHistoryFacade` | `HistoryEntryDeleted` |
| `SetHistoryEntryFavorite`(SetHistoryEntryFavoriteInput) | `SetHistoryEntryFavorite` | `execute_set_history_entry_favorite`<br>`history.rs` | `set_history_entry_favorite`<br>`ClipboardHistoryFacade` | `HistoryEntryFavoriteSet` |
| `ListHistoryTags` | `ListHistoryTags` | `execute_list_history_tags`<br>`tags.rs` | `list_history_tags`<br>`AppFacade` | `HistoryTags` |
| `CreateHistoryTag`(CreateHistoryTagInput) | `CreateHistoryTag` | `execute_create_history_tag`<br>`tags.rs` | `create_history_tag`<br>`AppFacade` | `HistoryTagCreated` |
| `RenameHistoryTag`(RenameHistoryTagInput) | `RenameHistoryTag` | `execute_rename_history_tag`<br>`tags.rs` | `rename_history_tag`<br>`AppFacade` | `HistoryTagRenamed` |
| `AddHistoryTagToEntries`(HistoryTagEntriesInput) | `AddHistoryTagToEntries` | `execute_add_history_tag_to_entries`<br>`tags.rs` | `add_history_tag_to_entries`<br>`AppFacade` | `HistoryTagEntriesChanged` |
| `RemoveHistoryTagFromEntries`(HistoryTagEntriesInput) | `RemoveHistoryTagFromEntries` | `execute_remove_history_tag_from_entries`<br>`tags.rs` | `remove_history_tag_from_entries`<br>`AppFacade` | `HistoryTagEntriesChanged` |
| `SummarizeHistoryEntryTags`(HistoryEntryTagsInput) | `SummarizeHistoryEntryTags` | `execute_summarize_history_entry_tags`<br>`tags.rs` | `summarize_history_entry_tags`<br>`AppFacade` | `HistoryEntryTags` |
| `MergeHistoryTags`(MergeHistoryTagsInput) | `MergeHistoryTags` | `execute_merge_history_tags`<br>`tags.rs` | `merge_history_tags`<br>`AppFacade` | `HistoryTagsMerged` |
| `DeleteHistoryTag`(HistoryTagInput) | `DeleteHistoryTag` | `execute_delete_history_tag`<br>`tags.rs` | `delete_history_tag`<br>`AppFacade` | `HistoryTagDeleted` |
| `QueryHistoryStats` | `QueryHistoryStats` | `execute_query_history_stats`<br>`history.rs` | `history_stats`<br>`ClipboardHistoryFacade` | `HistoryStats` |
| `GetHistoryEntryResource`(HistoryEntryInput) | `GetHistoryEntryResource` | `execute_get_history_entry_resource`<br>`history.rs` | `get_history_entry_resource`<br>`ClipboardHistoryFacade` | `HistoryEntryResource` |
| `ReadBlob`(BlobResourceInput) | `ReadBlob` | `execute_read_blob`<br>`resource.rs` | `read_blob_resource`<br>`ResourceFacade` | `BlobRead` |
| `ReadEntryThumbnail`(HistoryEntryInput) | `ReadEntryThumbnail` | `execute_read_entry_thumbnail`<br>`resource.rs` | `read_entry_thumbnail_resource`<br>`AppFacade` | `ThumbnailRead` |
| `ReadEntryFile`(HistoryEntryInput) | `ReadEntryFile` | `execute_read_entry_file`<br>`resource.rs` | `read_entry_file_resource`<br>`ResourceFacade` | `EntryFileRead` |
| `QueryEntryDelivery`(HistoryEntryInput) | `QueryEntryDelivery` | `execute_query_entry_delivery`<br>`delivery.rs` | `get_entry_delivery_view`<br>`ClipboardSyncFacade` | `EntryDelivery` |
| `ClearHistory` | `ClearHistory` | `execute_clear_history`<br>`history.rs` | `clear_history`<br>`ClipboardHistoryFacade` | `HistoryCleared` |
| `QueryOutboundProgress`(OutboundProgressInput) | `QueryOutboundProgress` | `execute_query_outbound_progress`<br>`transfer_progress.rs` | `get_transfer_progress`<br>`ProgressViewOwner` | `OutboundProgress` |
| `ListOutboundProgress` | `ListOutboundProgress` | `execute_list_outbound_progress`<br>`transfer_progress.rs` | `list_transfer_progress`<br>`ProgressViewOwner` | `OutboundProgressList` |
| `ReconcileOutboundProgress`(ReconcileOutboundProgressInput) | `ReconcileOutboundProgress` | `execute_reconcile_outbound_progress`<br>`transfer_progress.rs` | `reconcile_transfer_progress`<br>`AppFacade` | `OutboundProgressReconciliation` |
| `QueryEntryReceiveProgress`(EntryReceiveProgressInput) | `QueryEntryReceiveProgress` | `execute_query_entry_receive_progress`<br>`receive.rs` | `get_entry_receive_progress`<br>`AppFacade` | `EntryReceiveProgress` |
| `ListEntryReceiveProgress` | `ListEntryReceiveProgress` | `execute_list_entry_receive_progress`<br>`receive.rs` | `list_entry_receive_progress`<br>`ClipboardSyncFacade` | `EntryReceiveProgressList` |
| `CancelEntryReceive`(CancelEntryReceiveInput) | `CancelEntryReceive` | `execute_cancel_entry_receive`<br>`receive.rs` | `cancel_entry_receive`<br>`AppFacade` | `EntryReceiveCancellation` |
| `RetryEntryReceive`(RetryEntryReceiveInput) | `RetryEntryReceive` | `execute_retry_entry_receive`<br>`receive.rs` | `retry_entry_receive`<br>`AppFacade` | `EntryReceiveRetry` |
| `CancelInboundTransfer`(CancelInboundTransferInput) | `CancelInboundTransfer` | `execute_cancel_inbound_transfer`<br>`receive.rs` | `cancel_inbound_transfer`<br>`AppFacade` | `InboundTransferCancellation` |
| `ResendEntry`(ResendEntryInput) | `ResendEntry` | `execute_resend_entry`<br>`resend.rs` | `resend_entry`<br>`ClipboardOutboundFacade` | `EntryResent` |

## 领域：clipboard（3）

| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |
| --- | --- | --- | --- | --- |
| `CaptureCurrentClipboard` | `CaptureCurrentClipboard` | `execute_capture_current_clipboard`<br>`active.rs` | `capture_current_clipboard`<br>`ClipboardCaptureFacade` | `ClipboardCaptured` |
| `QueryActiveClipboard` | `QueryActiveClipboard` | `execute_query_active_clipboard`<br>`active.rs` | `current_active_clipboard`<br>`ActiveClipboardFacade` | `ActiveClipboard` |
| `RestoreClipboard`(RestoreClipboardInput) | `RestoreClipboard` | `execute_restore_clipboard`<br>`restore.rs` | `restore_clipboard`<br>`ClipboardRestoreFacade` | `ClipboardRestored` |

## 生成时发现的缺口

- 全部变体都出现在 `Operation::kind()` 中。
- 全部变体都有路由分支。
- 1 个适配函数没有可见的转调方法（按调用名识别，可能漏判）：`execute_export_entry`

## 登记点

新增一个操作必须同时改动下列位置，漏改通常只有集成测试能发现：

1. `crates/uc-engine/src/contract/operation.rs`：枚举变体与 `kind()`
2. `crates/uc-engine/src/contract/result.rs`：结果变体
3. `crates/uc-engine/src/operations/<领域>/`：适配函数
4. `crates/uc-engine/src/runtime/dispatch.rs`：路由分支
5. `bindings/uc-engine-uniffi/src/runtime.rs`：Swift / Kotlin 入口
6. `bindings/uc-ohos-napi/src/runtime.rs`：HarmonyOS 入口
