use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;
use uc_core::app_dirs::{AppDirs, AppPaths};
use uc_core::clipboard::{
    normalize_wire_mime, FileDisplayMetadata, FileDisplayMetadataEntry,
    ObservedClipboardRepresentation, SystemClipboardSnapshot, FILE_DISPLAY_METADATA_FORMAT,
    FILE_DISPLAY_METADATA_MIME,
};
use uc_core::ids::{FormatId, RepresentationId};
use uc_core::ports::{
    ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, EmitError, HostEvent,
    HostEventEmitterPort, PlatformClipboardPort, SecureStorageError, SecureStoragePort,
    SystemClipboardPort, TransferHostEvent,
};
use uc_observability_contract::analytics::DefaultAnalyticsFacade;

use crate::assembly::deps::{BackgroundRuntimeDeps, WiredDependencies, WiringError, WiringResult};
use crate::assembly::platform::SystemClipboardLayer;
use crate::assembly::wire::{wire_dependencies_from_inputs, CoreWiringInputs};
use crate::engine::event_stream::EventSender;
use crate::{
    EngineConfig, EngineEvent, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardChangeStream, HostClipboardRepresentation, HostDirectories,
    HostFileAccess, HostSecureStorage, TransferProgress,
};

struct HostSecureStorageAdapter {
    host: Box<dyn HostSecureStorage>,
}

impl SecureStoragePort for HostSecureStorageAdapter {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        self.host.get(key).map_err(map_secure_storage_error)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.host.set(key, value).map_err(map_secure_storage_error)
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.host.delete(key).map_err(map_secure_storage_error)
    }
}

fn map_secure_storage_error(error: HostCapabilityError) -> SecureStorageError {
    let message = error.to_string();
    match error.category() {
        HostCapabilityErrorCategory::Unavailable => SecureStorageError::Unavailable(message),
        HostCapabilityErrorCategory::PermissionDenied => {
            SecureStorageError::PermissionDenied(message)
        }
        HostCapabilityErrorCategory::InvalidHandle | HostCapabilityErrorCategory::Io => {
            SecureStorageError::Other(message)
        }
    }
}

pub fn adapt_secure_storage(host: Box<dyn HostSecureStorage>) -> Arc<dyn SecureStoragePort> {
    Arc::new(HostSecureStorageAdapter { host })
}

struct HostClipboardAdapter {
    host: Box<dyn HostClipboard>,
    files: Arc<dyn HostFileAccess>,
    import_root: PathBuf,
}

impl SystemClipboardPort for HostClipboardAdapter {
    fn read_snapshot(&self) -> anyhow::Result<SystemClipboardSnapshot> {
        let snapshot = self
            .host
            .read()
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        let mut representations = Vec::with_capacity(snapshot.representations.len());
        let mut file_metadata = Vec::new();
        let operation_dir = self
            .import_file_representations(
                snapshot.representations,
                &mut representations,
                &mut file_metadata,
            )
            .map_err(|error| anyhow::anyhow!("host clipboard file import failed: {error}"))?;

        if !file_metadata.is_empty() {
            let encoded = FileDisplayMetadata {
                files: file_metadata,
            }
            .encode()
            .map_err(|_| anyhow::anyhow!("host clipboard metadata encoding failed"));
            match encoded {
                Ok(bytes) => representations.push(ObservedClipboardRepresentation::new(
                    RepresentationId::new(),
                    FormatId::from(FILE_DISPLAY_METADATA_FORMAT),
                    normalize_wire_mime(Some(FILE_DISPLAY_METADATA_MIME.to_string())),
                    bytes,
                )),
                Err(error) => {
                    cleanup_import_directory(operation_dir.as_deref());
                    return Err(error);
                }
            }
        }

        Ok(SystemClipboardSnapshot {
            ts_ms: snapshot.observed_at_ms,
            representations,
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        })
    }

    fn write_snapshot(&self, snapshot: SystemClipboardSnapshot) -> anyhow::Result<()> {
        let representations = snapshot
            .representations
            .into_iter()
            .map(|representation| {
                let bytes = representation
                    .inline_bytes()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "local file clipboard representations cannot reach the host"
                        )
                    })?
                    .to_vec();
                Ok(HostClipboardRepresentation::Inline {
                    format: representation.format_id.to_string(),
                    mime_type: representation.mime.map(|mime| mime.0),
                    bytes,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        self.host
            .write(crate::HostClipboardSnapshot {
                observed_at_ms: snapshot.ts_ms,
                representations,
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

impl HostClipboardAdapter {
    fn import_file_representations(
        &self,
        host_representations: Vec<HostClipboardRepresentation>,
        representations: &mut Vec<ObservedClipboardRepresentation>,
        file_metadata: &mut Vec<FileDisplayMetadataEntry>,
    ) -> anyhow::Result<Option<PathBuf>> {
        let mut operation_dir: Option<PathBuf> = None;
        for representation in host_representations {
            let result = match representation {
                HostClipboardRepresentation::Inline {
                    format,
                    mime_type,
                    bytes,
                } => {
                    representations.push(ObservedClipboardRepresentation::new(
                        RepresentationId::new(),
                        FormatId::from(format),
                        normalize_wire_mime(mime_type),
                        bytes,
                    ));
                    Ok(())
                }
                HostClipboardRepresentation::File {
                    format,
                    handle,
                    display_name,
                    mime_type,
                    size_bytes,
                } => {
                    let directory = match operation_dir.as_ref() {
                        Some(directory) => directory.clone(),
                        None => {
                            let directory =
                                self.import_root.join(RepresentationId::new().to_string());
                            std::fs::create_dir_all(&directory)?;
                            operation_dir = Some(directory.clone());
                            directory
                        }
                    };
                    let storage_name = RepresentationId::new().to_string();
                    let path = directory.join(&storage_name);
                    copy_host_clipboard_file(self.files.as_ref(), &handle, size_bytes, &path)?;
                    representations.push(ObservedClipboardRepresentation::new_local_file(
                        RepresentationId::new(),
                        FormatId::from(format),
                        normalize_wire_mime(mime_type),
                        path,
                        size_bytes,
                    ));
                    file_metadata.push(FileDisplayMetadataEntry {
                        storage_name,
                        display_name,
                    });
                    Ok(())
                }
            };
            if let Err(error) = result {
                cleanup_import_directory(operation_dir.as_deref());
                return Err(error);
            }
        }
        Ok(operation_dir)
    }
}

const HOST_CLIPBOARD_FILE_CHUNK_SIZE: u32 = 64 * 1024;

fn copy_host_clipboard_file(
    files: &dyn HostFileAccess,
    handle: &crate::HostFileHandle,
    size_bytes: u64,
    destination: &Path,
) -> anyhow::Result<()> {
    let metadata = files
        .metadata(handle)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if metadata.size_bytes != size_bytes {
        return Err(anyhow::anyhow!("host clipboard file size changed"));
    }
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    let mut offset = 0_u64;
    while offset < size_bytes {
        let requested = (size_bytes - offset).min(HOST_CLIPBOARD_FILE_CHUNK_SIZE as u64) as u32;
        let chunk = files
            .read_chunk(handle, offset, requested)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if chunk.is_empty() || chunk.len() > requested as usize {
            return Err(anyhow::anyhow!("host clipboard file read was incomplete"));
        }
        output.write_all(&chunk)?;
        offset = offset
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("host clipboard file offset overflow"))?;
    }
    output.sync_all()?;
    Ok(())
}

fn cleanup_import_directory(directory: Option<&Path>) {
    let Some(directory) = directory else {
        return;
    };
    if std::fs::remove_dir_all(directory).is_err() {
        warn!("failed to remove incomplete host clipboard import");
    }
}

#[cfg(test)]
pub fn adapt_system_clipboard(
    host: Box<dyn HostClipboard>,
    files: Arc<dyn HostFileAccess>,
    import_root: PathBuf,
) -> Arc<dyn SystemClipboardPort> {
    Arc::new(HostClipboardAdapter {
        host,
        files,
        import_root,
    })
}

pub fn derive_app_paths(directories: &HostDirectories) -> AppPaths {
    AppPaths::from_app_dirs(&AppDirs {
        app_data_root: directories.private_data().to_path_buf(),
        app_cache_root: directories.cache().to_path_buf(),
        app_log_dir: directories.logs().to_path_buf(),
    })
}

fn adopt_v019_profile_directories(app_data_root: &Path) -> WiringResult<()> {
    let directories = [("iroh-identity", "identity"), ("iroh-blobs", "blob store")];
    let mut removals = Vec::new();
    let mut moves = Vec::new();
    let entries = match std::fs::read_dir(app_data_root) {
        Ok(entries) => entries
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| WiringError::SettingsInit("failed to inspect v0.19 data".to_owned()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(WiringError::SettingsInit(
                "failed to inspect v0.19 data".to_owned(),
            ))
        }
    };

    for (name, description) in directories {
        let current = app_data_root.join(name);
        let mut legacy_directories = entries.iter().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.starts_with(&format!("{name}_")))
        });
        let legacy = legacy_directories.next();
        if legacy_directories.next().is_some() {
            return Err(WiringError::SettingsInit(format!(
                "multiple v0.19 {description} directories found"
            )));
        }
        let Some(legacy) = legacy else {
            continue;
        };
        if current.exists() {
            let legacy_is_empty =
                std::fs::read_dir(&legacy).is_ok_and(|mut entries| entries.next().is_none());
            if legacy_is_empty {
                removals.push((legacy, description));
                continue;
            }
            return Err(WiringError::SettingsInit(format!(
                "v0.19 {description} directory conflict"
            )));
        }
        moves.push((legacy, current, description));
    }

    for (legacy, description) in removals {
        std::fs::remove_dir(&legacy).map_err(|_| {
            WiringError::SettingsInit(format!(
                "failed to remove empty v0.19 {description} directory"
            ))
        })?;
    }

    for (legacy, current, description) in moves {
        std::fs::rename(&legacy, &current).map_err(|_| {
            WiringError::SettingsInit(format!("failed to adopt v0.19 {description} directory"))
        })?;
    }

    Ok(())
}

fn adapt_system_clipboard_layer(
    host: Box<dyn HostClipboard>,
    files: Arc<dyn HostFileAccess>,
    import_root: PathBuf,
) -> SystemClipboardLayer {
    let adapter = Arc::new(HostClipboardAdapter {
        host,
        files,
        import_root,
    });
    let clipboard: Arc<dyn PlatformClipboardPort> = adapter.clone();
    let system_clipboard: Arc<dyn SystemClipboardPort> = adapter;
    SystemClipboardLayer::new(clipboard, system_clipboard)
}

#[cfg(test)]
struct NoopHostEventEmitter;

#[cfg(test)]
impl HostEventEmitterPort for NoopHostEventEmitter {
    fn emit(&self, _event: HostEvent) -> Result<(), EmitError> {
        Ok(())
    }
}

pub(crate) struct EngineHostEventEmitter {
    events: EventSender,
}

impl EngineHostEventEmitter {
    pub(crate) fn new(events: EventSender) -> Self {
        Self { events }
    }
}

impl HostEventEmitterPort for EngineHostEventEmitter {
    fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
        let event = match event {
            HostEvent::Transfer(TransferHostEvent::Progress {
                transfer_id,
                entry_id,
                attempt_id,
                peer_id,
                direction,
                bytes_transferred,
                total_bytes,
            }) => EngineEvent::TransferProgress(TransferProgress {
                transfer_id,
                entry_id,
                attempt_id,
                peer_id,
                direction: match direction {
                    uc_core::file_transfer::FileTransferDirection::Sending => {
                        crate::TransferDirectionSummary::Sending
                    }
                    uc_core::file_transfer::FileTransferDirection::Receiving => {
                        crate::TransferDirectionSummary::Receiving
                    }
                },
                completed_bytes: bytes_transferred,
                total_bytes,
            }),
            HostEvent::Clipboard(ClipboardHostEvent::NewContent {
                entry_id,
                attempt_id,
                preview,
                origin,
            }) => EngineEvent::IncomingEntry(crate::IncomingEntryEvent {
                entry_id,
                attempt_id,
                preview,
                origin: match origin {
                    ClipboardOriginKind::Local => crate::ClipboardOriginSummary::Local,
                    ClipboardOriginKind::Remote => crate::ClipboardOriginSummary::Remote,
                },
            }),
            HostEvent::Transfer(TransferHostEvent::StatusChanged {
                transfer_id,
                entry_id,
                attempt_id,
                status,
                reason,
            }) => EngineEvent::TransferStatusChanged(crate::TransferStatusChanged {
                transfer_id,
                entry_id,
                attempt_id,
                status,
                reason,
            }),
            HostEvent::Delivery(DeliveryHostEvent::StatusChanged {
                entry_id,
                target_device_id,
            }) => EngineEvent::DeliveryStatusChanged(crate::DeliveryStatusChanged {
                entry_id,
                target_device_id,
            }),
            HostEvent::Clipboard(ClipboardHostEvent::IncomingPending {
                entry_id,
                attempt_id,
                from_device,
                total_bytes,
                filenames,
            }) => EngineEvent::IncomingPending(crate::IncomingPendingEvent {
                entry_id,
                attempt_id,
                from_device,
                total_bytes,
                filenames,
            }),
            HostEvent::Clipboard(ClipboardHostEvent::ReceiveAttemptStateChanged {
                entry_id,
                attempt_id,
                state,
            }) => EngineEvent::ReceiveAttemptStateChanged(crate::ReceiveAttemptStateChanged {
                entry_id,
                attempt_id,
                state,
            }),
        };
        self.events.send(event);
        Ok(())
    }
}

pub struct HostWiring {
    pub wired: WiredDependencies,
    pub background: BackgroundRuntimeDeps,
    pub paths: AppPaths,
    pub temporary_dir: std::path::PathBuf,
    pub clipboard_import_root: std::path::PathBuf,
    pub files: Arc<dyn HostFileAccess>,
    pub clipboard_changes: Option<Box<dyn HostClipboardChangeStream>>,
}

#[cfg(test)]
pub fn wire_host_capabilities(
    config: &EngineConfig,
    host: HostCapabilities,
) -> WiringResult<HostWiring> {
    wire_host_capabilities_with_emitter(config, host, Arc::new(NoopHostEventEmitter))
}

pub(crate) fn wire_host_capabilities_with_emitter(
    config: &EngineConfig,
    host: HostCapabilities,
    host_event_emitter: Arc<dyn HostEventEmitterPort>,
) -> WiringResult<HostWiring> {
    let (directories, secure_storage, mut clipboard, files, analytics) = host.into_parts();
    let clipboard_changes = clipboard.take_change_stream().map_err(|_| {
        WiringError::ClipboardInit("failed to open host clipboard change stream".into())
    })?;
    let paths = derive_app_paths(&directories);
    let secure_storage = adapt_secure_storage(secure_storage);
    let app_data_root = paths.app_data_root_dir.clone();
    adopt_v019_profile_directories(&app_data_root)?;
    uc_infra::config_migration::staging::apply_pending_import(
        &app_data_root,
        &paths.db_path,
        &paths.vault_dir,
        &paths.settings_path,
        &app_data_root.join("iroh-identity"),
        secure_storage.as_ref(),
    )
    .map_err(|error| WiringError::SettingsInit(error.to_string()))?;
    let temporary_dir = directories.temporary().to_path_buf();
    let clipboard_import_root = temporary_dir.join("clipboard-imports");
    if let Err(error) = std::fs::remove_dir_all(&clipboard_import_root) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(WiringError::ClipboardInit(
                "failed to clear stale host clipboard imports".into(),
            ));
        }
    }
    std::fs::create_dir_all(&clipboard_import_root).map_err(|_| {
        WiringError::ClipboardInit("failed to create host clipboard import directory".into())
    })?;
    let files: Arc<dyn HostFileAccess> = Arc::from(files);
    let (wired, background) = wire_dependencies_from_inputs(CoreWiringInputs {
        paths: paths.clone(),
        secure_storage,
        profile_id: uc_core::ids::ProfileId::from(config.profile_id()),
        app_version: config.app_version().to_string(),
        config_source_mode: if config.uses_portable_storage() {
            uc_core::ports::ConfigSourceMode::Portable
        } else {
            uc_core::ports::ConfigSourceMode::Installed
        },
        iroh_identity_dir: app_data_root.join("iroh-identity"),
        iroh_blob_store_dir: app_data_root.join("iroh-blobs"),
        system_clipboard: adapt_system_clipboard_layer(
            clipboard,
            Arc::clone(&files),
            clipboard_import_root.clone(),
        ),
        analytics_sink: Arc::clone(&analytics.sink),
        analytics_facade: Arc::new(DefaultAnalyticsFacade::new(
            analytics.sink,
            analytics.identity,
        )),
        host_event_emitter,
    })?;

    Ok(HostWiring {
        wired,
        background,
        paths,
        temporary_dir,
        clipboard_import_root,
        files,
        clipboard_changes,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use uc_core::file_transfer::FileTransferDirection;
    use uc_core::ids::SpaceId;
    use uc_core::membership::{CurrentMemberSignatureError, MembershipCandidateRepositoryError};
    use uc_core::ports::{
        ClipboardHostEvent, ClipboardOriginKind, DeliveryHostEvent, HostEvent,
        HostEventEmitterPort, TransferHostEvent,
    };

    use crate::engine::event_stream::event_channel;
    use crate::{
        ClipboardOriginSummary, DeliveryStatusChanged, EngineConfig, EngineEvent, HostCapabilities,
        HostCapabilityError, HostClipboard, HostClipboardSnapshot, HostDirectories, HostFileAccess,
        HostFileHandle, HostFileMetadata, HostSecureStorage, IncomingPendingEvent,
        ReceiveAttemptStateChanged, TransferDirectionSummary, TransferProgress,
        TransferStatusChanged,
    };

    use super::{adopt_v019_profile_directories, wire_host_capabilities, EngineHostEventEmitter};
    use crate::assembly::lifecycle::build_daemon_lifecycle;

    #[test]
    fn v019_profile_directories_are_adopted_before_engine_wiring() {
        let root = tempfile::tempdir().unwrap();
        let data_root = root.path();
        let legacy_identity = data_root.join("iroh-identity_mobile_primary");
        let legacy_blobs = data_root.join("iroh-blobs_mobile_primary");
        std::fs::create_dir_all(&legacy_identity).unwrap();
        std::fs::create_dir_all(&legacy_blobs).unwrap();
        std::fs::write(legacy_identity.join("identity.bin"), b"identity").unwrap();
        std::fs::write(legacy_blobs.join("blobs.db"), b"blobs").unwrap();

        adopt_v019_profile_directories(data_root).unwrap();

        assert_eq!(
            std::fs::read(data_root.join("iroh-identity/identity.bin")).unwrap(),
            b"identity"
        );
        assert_eq!(
            std::fs::read(data_root.join("iroh-blobs/blobs.db")).unwrap(),
            b"blobs"
        );
        assert!(!legacy_identity.exists());
        assert!(!legacy_blobs.exists());
    }

    #[test]
    fn absent_v019_profile_directories_leave_current_layout_untouched() {
        let root = tempfile::tempdir().unwrap();

        adopt_v019_profile_directories(root.path()).unwrap();

        assert!(!root.path().join("iroh-identity").exists());
        assert!(!root.path().join("iroh-blobs").exists());
    }

    #[test]
    fn missing_app_data_root_is_treated_as_a_fresh_installation() {
        let root = tempfile::tempdir().unwrap();
        let data_root = root.path().join("private");

        adopt_v019_profile_directories(&data_root).unwrap();

        assert!(!data_root.exists());
    }

    #[test]
    fn empty_v019_identity_directory_is_removed_when_current_identity_exists() {
        let root = tempfile::tempdir().unwrap();
        let current_identity = root.path().join("iroh-identity");
        let legacy_identity = root.path().join("iroh-identity_profile");
        std::fs::create_dir_all(&current_identity).unwrap();
        std::fs::create_dir_all(&legacy_identity).unwrap();
        std::fs::write(current_identity.join("identity.bin"), b"current identity").unwrap();

        adopt_v019_profile_directories(root.path()).unwrap();

        assert_eq!(
            std::fs::read(current_identity.join("identity.bin")).unwrap(),
            b"current identity"
        );
        assert!(!legacy_identity.exists());
    }

    #[test]
    fn nonempty_v019_and_current_blob_directories_stop_startup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_a")).unwrap();
        let legacy_blobs = root.path().join("iroh-blobs_a");
        std::fs::create_dir_all(&legacy_blobs).unwrap();
        std::fs::write(legacy_blobs.join("blobs.db"), b"legacy blobs").unwrap();
        std::fs::create_dir_all(root.path().join("iroh-blobs")).unwrap();

        let error = adopt_v019_profile_directories(root.path()).unwrap_err();

        assert!(error.to_string().contains("blob store directory conflict"));
        assert!(root.path().join("iroh-identity_a").exists());
        assert!(!root.path().join("iroh-identity").exists());
    }

    #[test]
    fn multiple_empty_v019_directories_stop_startup() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity")).unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_a")).unwrap();
        std::fs::create_dir_all(root.path().join("iroh-identity_b")).unwrap();

        let error = adopt_v019_profile_directories(root.path()).unwrap_err();

        assert!(error
            .to_string()
            .contains("multiple v0.19 identity directories found"));
        assert!(root.path().join("iroh-identity_a").exists());
        assert!(root.path().join("iroh-identity_b").exists());
    }

    #[derive(Default)]
    struct TestSecureStorage(Mutex<HashMap<String, Vec<u8>>>);

    impl HostSecureStorage for TestSecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct EmptyHostClipboard;

    impl HostClipboard for EmptyHostClipboard {
        fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
            Ok(HostClipboardSnapshot {
                observed_at_ms: 1,
                representations: Vec::new(),
            })
        }

        fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
            Ok(())
        }
    }

    struct EmptyHostFiles;

    impl HostFileAccess for EmptyHostFiles {
        fn metadata(
            &self,
            _handle: &HostFileHandle,
        ) -> Result<HostFileMetadata, HostCapabilityError> {
            Err(HostCapabilityError::new(
                crate::HostCapabilityErrorCategory::InvalidHandle,
                "test handle unavailable",
            ))
        }

        fn read_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _max_bytes: u32,
        ) -> Result<Vec<u8>, HostCapabilityError> {
            Ok(Vec::new())
        }

        fn write_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _bytes: &[u8],
        ) -> Result<(), HostCapabilityError> {
            Ok(())
        }

        fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn host_wiring_exposes_membership_runtime_dependencies() {
        let root = tempfile::tempdir().unwrap();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.path().join("private"),
                root.path().join("cache"),
                root.path().join("temporary"),
                root.path().join("logs"),
            ),
            Box::new(TestSecureStorage::default()),
            Box::new(EmptyHostClipboard),
            Box::new(EmptyHostFiles),
        );

        let wiring = wire_host_capabilities(&EngineConfig::new("test"), host).unwrap();

        assert_eq!(
            wiring
                .wired
                .sync_engine
                .membership_candidate_repo
                .list(&SpaceId::from("space-a"))
                .await,
            Err(MembershipCandidateRepositoryError::Locked)
        );
        assert_eq!(
            wiring
                .wired
                .sync_engine
                .current_member_signatures
                .current_member_epoch()
                .await,
            Err(CurrentMemberSignatureError::Unavailable)
        );
        assert!(!wiring.wired.sync_engine.membership_session.is_ready());
        assert_eq!(
            wiring
                .wired
                .sync_engine
                .workspace_convergence_repository
                .load_state()
                .await,
            Err(uc_core::membership::WorkspaceConvergenceRepositoryError::Locked)
        );
    }

    // 流程：启动真实生产组装，确认 1.1 成员核对与旧空间升级入口同时存在，
    // 再确认已经废弃的成员移除入口没有被重新带回。
    #[tokio::test]
    async fn production_engine_assembly_registers_membership_attestation_protocol() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("private")).unwrap();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.path().join("private"),
                root.path().join("cache"),
                root.path().join("temporary"),
                root.path().join("logs"),
            ),
            Box::new(TestSecureStorage::default()),
            Box::new(EmptyHostClipboard),
            Box::new(EmptyHostFiles),
        );
        let wiring = wire_host_capabilities(&EngineConfig::new("test"), host).unwrap();
        let mut settings = wiring.wired.deps.settings.load().await.unwrap();
        settings.network.allow_relay_fallback = false;
        wiring.wired.deps.settings.save(&settings).await.unwrap();

        let lifecycle = build_daemon_lifecycle(
            &wiring.wired.deps,
            &wiring.wired.sync_engine,
            &wiring.wired.shared,
            "test",
            #[cfg(feature = "lan-compat")]
            wiring.wired.mobile_sync_ports.clone(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let reachable = lifecycle
            .sync_engine_assembly
            .membership_attestation_is_reachable_for_test()
            .await;
        let membership_history_reachable = lifecycle
            .sync_engine_assembly
            .membership_history_exchange_is_reachable_for_test()
            .await;
        let admission_completion_recovery_reachable = lifecycle
            .sync_engine_assembly
            .admission_completion_recovery_is_reachable_for_test()
            .await;
        let deprecated_removal_protocols_reachable = lifecycle
            .sync_engine_assembly
            .deprecated_removal_protocols_are_reachable_for_test()
            .await;
        lifecycle
            .sync_engine_assembly
            .shutdown(uc_core::FileTransferCancellationReason::Unknown)
            .await;

        assert!(
            reachable,
            "membership attestation protocol was not installed"
        );
        assert!(
            membership_history_reachable,
            "membership history exchange was not installed"
        );
        assert!(
            admission_completion_recovery_reachable,
            "admission completion recovery was not installed"
        );
        assert!(
            !deprecated_removal_protocols_reachable,
            "superseded member-removal protocols were installed"
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_forwards_transfer_progress() {
        let (events, mut stream) = event_channel(8);
        let emitter: Arc<dyn HostEventEmitterPort> = Arc::new(EngineHostEventEmitter::new(events));

        emitter
            .emit(HostEvent::Transfer(TransferHostEvent::Progress {
                transfer_id: "transfer-1".into(),
                entry_id: Some("entry-1".into()),
                attempt_id: Some("attempt-1".into()),
                peer_id: "peer-1".into(),
                direction: FileTransferDirection::Receiving,
                bytes_transferred: 64,
                total_bytes: Some(128),
            }))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::TransferProgress(TransferProgress {
                transfer_id: "transfer-1".into(),
                entry_id: Some("entry-1".into()),
                attempt_id: Some("attempt-1".into()),
                peer_id: "peer-1".into(),
                direction: TransferDirectionSummary::Receiving,
                completed_bytes: 64,
                total_bytes: Some(128),
            }))
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_preserves_clipboard_change_details() {
        let (events, mut stream) = event_channel(8);
        let emitter = EngineHostEventEmitter::new(events);

        emitter
            .emit(HostEvent::Clipboard(ClipboardHostEvent::NewContent {
                entry_id: "entry-1".into(),
                attempt_id: None,
                preview: "placeholder".into(),
                origin: ClipboardOriginKind::Remote,
            }))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::IncomingEntry(crate::IncomingEntryEvent {
                entry_id: "entry-1".into(),
                attempt_id: None,
                preview: "placeholder".into(),
                origin: ClipboardOriginSummary::Remote,
            }))
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_preserves_transfer_status_details() {
        let (events, mut stream) = event_channel(8);
        let emitter = EngineHostEventEmitter::new(events);

        emitter
            .emit(HostEvent::Transfer(TransferHostEvent::StatusChanged {
                transfer_id: "transfer-1".into(),
                entry_id: "entry-1".into(),
                attempt_id: Some("attempt-1".into()),
                status: "failed".into(),
                reason: Some("cancelled".into()),
            }))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::TransferStatusChanged(TransferStatusChanged {
                transfer_id: "transfer-1".into(),
                entry_id: "entry-1".into(),
                attempt_id: Some("attempt-1".into()),
                status: "failed".into(),
                reason: Some("cancelled".into()),
            }))
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_preserves_delivery_change_details() {
        let (events, mut stream) = event_channel(8);
        let emitter = EngineHostEventEmitter::new(events);

        emitter
            .emit(HostEvent::Delivery(DeliveryHostEvent::StatusChanged {
                entry_id: "entry-1".into(),
                target_device_id: "peer-1".into(),
            }))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::DeliveryStatusChanged(DeliveryStatusChanged {
                entry_id: "entry-1".into(),
                target_device_id: "peer-1".into(),
            }))
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_preserves_incoming_pending_details() {
        let (events, mut stream) = event_channel(8);
        let emitter = EngineHostEventEmitter::new(events);

        emitter
            .emit(HostEvent::Clipboard(ClipboardHostEvent::IncomingPending {
                entry_id: "entry-1".into(),
                attempt_id: Some("attempt-1".into()),
                from_device: "peer-1".into(),
                total_bytes: Some(128),
                filenames: vec!["private.txt".into()],
            }))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::IncomingPending(IncomingPendingEvent {
                entry_id: "entry-1".into(),
                attempt_id: Some("attempt-1".into()),
                from_device: "peer-1".into(),
                total_bytes: Some(128),
                filenames: vec!["private.txt".into()],
            }))
        );
    }

    #[tokio::test]
    async fn engine_event_emitter_preserves_receive_attempt_state() {
        let (events, mut stream) = event_channel(8);
        let emitter = EngineHostEventEmitter::new(events);

        emitter
            .emit(HostEvent::Clipboard(
                ClipboardHostEvent::ReceiveAttemptStateChanged {
                    entry_id: "entry-1".into(),
                    attempt_id: "attempt-1".into(),
                    state: "failed".into(),
                },
            ))
            .unwrap();

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::ReceiveAttemptStateChanged(
                ReceiveAttemptStateChanged {
                    entry_id: "entry-1".into(),
                    attempt_id: "attempt-1".into(),
                    state: "failed".into(),
                }
            ))
        );
    }
}
