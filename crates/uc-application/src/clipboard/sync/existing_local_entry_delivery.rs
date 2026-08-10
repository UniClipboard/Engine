//! Recreates and sends an existing local clipboard entry to chosen devices.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;
use uc_core::blob::ports::BlobReaderPort;
use uc_core::clipboard::ClipboardContentCategorySet;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::clipboard::{
    ClipboardPayloadResolverPort, EntryFileSetRepositoryPort, GetClipboardEntryPort,
    GetRepresentationPort, UpdateRepresentationProcessingResultPort,
};
use uc_core::ports::{
    ClipboardEventRepositoryPort, ClipboardSelectionRepositoryPort, DeviceIdentityPort,
    SettingsPort,
};

use crate::clipboard::outbound::{
    assemble_outbound_payload, OutboundBlobPublishGateway, OutboundPayload, OutboundPayloadError,
};
use crate::clipboard::sync::dispatch_entry::{
    DispatchClipboardEntryInput, DispatchEntryRunner, DispatchSyncError,
};
use crate::clipboard::sync::payload_codec::{
    encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes,
    encode_snapshot_with_blob_refs_to_v3_bytes,
};
use crate::clipboard::sync::resend_entry::{NotResendableReason, ResendEntryError, ResendReport};
use crate::clipboard::sync::snapshot_from_entry::{
    reconstruct_snapshot_from_entry, BuildSnapshotError,
};

#[async_trait]
pub(crate) trait ExistingLocalEntryDeliveryRunner: Send + Sync {
    async fn deliver(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError>;
}

pub(super) struct ExistingLocalEntryDelivery {
    pub(super) entry_repo: Arc<dyn GetClipboardEntryPort>,
    pub(super) event_repo: Arc<dyn ClipboardEventRepositoryPort>,
    pub(super) selection_repo: Arc<dyn ClipboardSelectionRepositoryPort>,
    pub(super) representation_repo: Arc<dyn GetRepresentationPort>,
    pub(super) rep_processing_repo: Arc<dyn UpdateRepresentationProcessingResultPort>,
    pub(super) payload_resolver: Arc<dyn ClipboardPayloadResolverPort>,
    pub(super) blob_store: Arc<dyn BlobReaderPort>,
    pub(super) device_identity: Arc<dyn DeviceIdentityPort>,
    pub(super) settings: Arc<dyn SettingsPort>,
    pub(super) entry_file_set_repo: Arc<dyn EntryFileSetRepositoryPort>,
    pub(super) blob_publisher: Arc<dyn OutboundBlobPublishGateway>,
    pub(super) dispatch_runner: Arc<dyn DispatchEntryRunner>,
}

#[async_trait]
impl ExistingLocalEntryDeliveryRunner for ExistingLocalEntryDelivery {
    async fn deliver(
        &self,
        entry_id: EntryId,
        targets: Vec<DeviceId>,
    ) -> Result<ResendReport, ResendEntryError> {
        let entry = self
            .entry_repo
            .get_entry(&entry_id)
            .await
            .map_err(|err| ResendEntryError::Storage(format!("get_entry: {err}")))?
            .ok_or_else(|| ResendEntryError::EntryNotFound(entry_id.clone()))?;
        let source_device = self
            .event_repo
            .get_source_device(&entry.event_id)
            .await
            .map_err(|err| ResendEntryError::Storage(format!("get_source_device: {err}")))?
            .ok_or_else(|| ResendEntryError::EntryNotResendable {
                entry_id: entry_id.clone(),
                reason: NotResendableReason::RemoteOrigin,
            })?;
        if source_device != self.device_identity.current_device_id() {
            return Err(ResendEntryError::EntryNotResendable {
                entry_id,
                reason: NotResendableReason::RemoteOrigin,
            });
        }
        let snapshot = reconstruct_snapshot_from_entry(
            self.entry_repo.as_ref(),
            self.selection_repo.as_ref(),
            self.representation_repo.as_ref(),
            self.rep_processing_repo.as_ref(),
            self.payload_resolver.as_ref(),
            self.blob_store.as_ref(),
            &entry_id,
        )
        .await
        .map_err(|err| map_build_snapshot_error(err, &entry_id))?;

        let OutboundPayload {
            snapshot,
            blob_refs,
            file_set_manifest,
        } = assemble_outbound_payload(
            self.entry_file_set_repo.as_ref(),
            self.blob_publisher.as_ref(),
            Arc::clone(&self.settings),
            &entry_id,
            snapshot,
        )
        .await
        .map_err(|err| map_outbound_payload_error(err, &entry_id))?;

        let categories = ClipboardContentCategorySet::from_snapshot(&snapshot);
        let (plaintext, snapshot_hash, wire_version) = match file_set_manifest {
            Some(manifest) => {
                let (plaintext, snapshot_hash) =
                    encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes(
                        &snapshot, &blob_refs, &manifest,
                    )
                    .map_err(|err| ResendEntryError::Dispatch(format!("payload encode: {err}")))?;
                (
                    plaintext,
                    snapshot_hash,
                    uc_core::ports::ClipboardHeader::DIRECTORY_VERSION,
                )
            }
            None => {
                let (plaintext, snapshot_hash) = encode_snapshot_with_blob_refs_to_v3_bytes(
                    &snapshot, &blob_refs,
                )
                .map_err(|err| ResendEntryError::Dispatch(format!("payload encode: {err}")))?;
                (
                    plaintext,
                    snapshot_hash,
                    uc_core::ports::ClipboardHeader::CURRENT_VERSION,
                )
            }
        };

        let outcome = self
            .dispatch_runner
            .execute(DispatchClipboardEntryInput {
                plaintext,
                snapshot_hash,
                payload_version: 3,
                wire_version,
                categories,
                entry_id: Some(entry_id.clone()),
                target_filter: Some(targets),
                source_started_at: None,
            })
            .await
            .map_err(map_dispatch_sync_error)?;

        info!(
            entry_id = %entry_id,
            accepted = outcome.total_accepted,
            duplicate = outcome.total_duplicate,
            offline = outcome.total_offline,
            errored = outcome.total_errored,
            pending = outcome.total_pending,
            "existing local entry delivery completed"
        );

        Ok(ResendReport {
            accepted: outcome.total_accepted,
            duplicate: outcome.total_duplicate,
            offline: outcome.total_offline,
            errored: outcome.total_errored,
            pending: outcome.total_pending,
        })
    }
}

fn map_build_snapshot_error(err: BuildSnapshotError, entry_id: &EntryId) -> ResendEntryError {
    match err {
        BuildSnapshotError::EntryNotFound { entry_id } => ResendEntryError::EntryNotFound(entry_id),
        BuildSnapshotError::SelectionNotFound { .. }
        | BuildSnapshotError::PasteRepNotFound { .. }
        | BuildSnapshotError::PasteRepUnavailable(_)
        | BuildSnapshotError::PasteRepBlobFetchFailed { .. }
        | BuildSnapshotError::NoRestorableRepresentations { .. } => {
            ResendEntryError::EntryNotResendable {
                entry_id: entry_id.clone(),
                reason: NotResendableReason::PayloadLost,
            }
        }
        BuildSnapshotError::Repository(inner) => ResendEntryError::Storage(inner.to_string()),
    }
}

fn map_outbound_payload_error(err: OutboundPayloadError, entry_id: &EntryId) -> ResendEntryError {
    match err {
        OutboundPayloadError::Unavailable => ResendEntryError::EntryNotResendable {
            entry_id: entry_id.clone(),
            reason: NotResendableReason::PayloadLost,
        },
        OutboundPayloadError::Publish(err) => ResendEntryError::Dispatch(err.to_string()),
        OutboundPayloadError::Internal(message) => ResendEntryError::Dispatch(message),
    }
}

fn map_dispatch_sync_error(err: DispatchSyncError) -> ResendEntryError {
    match err {
        DispatchSyncError::LockedSpace => {
            ResendEntryError::Dispatch("encryption session locked".to_string())
        }
        DispatchSyncError::CipherFailure(message) => {
            ResendEntryError::Dispatch(format!("cipher: {message}"))
        }
        DispatchSyncError::Repository(message) => ResendEntryError::Storage(message),
    }
}
