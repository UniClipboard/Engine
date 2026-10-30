use moka::sync::Cache;
use tokio::sync::Semaphore;
use uc_core::clipboard::{MimeType, PayloadAvailability};
use uc_core::ids::{EntryId, RepresentationId};

use super::{BinaryResourceView, ResourceFacade, ResourceFacadeError};
pub(super) use crate::clipboard::history::resource_snapshot::ReadEntryResourceSnapshotPort;
#[cfg(test)]
pub(super) use crate::clipboard::history::resource_snapshot::{
    EntryResourceReadError, EntryResourceSnapshot,
};

const MAX_SOURCE_BYTES: i64 = 32 * 1024 * 1024;

pub(super) struct EntryThumbnailCache {
    pub cache: Cache<RepresentationId, BinaryResourceView>,
    pub gate: Semaphore,
}

impl Default for EntryThumbnailCache {
    fn default() -> Self {
        Self {
            cache: Cache::builder()
                .max_capacity(4 * 1024 * 1024)
                .weigher(|_: &RepresentationId, value: &BinaryResourceView| {
                    u32::try_from(value.bytes.len()).unwrap_or(u32::MAX)
                })
                .build(),
            gate: Semaphore::new(1),
        }
    }
}

impl ResourceFacade {
    pub(crate) fn discard_thumbnail_cache(&self) {
        self.entry_thumbnails.cache.invalidate_all();
        self.entry_thumbnails.cache.run_pending_tasks();
    }

    /// 从条目的权威预览按需生成缩略图；不读取历史派生数据。
    pub async fn entry_thumbnail(
        &self,
        entry_id: &str,
    ) -> Result<BinaryResourceView, ResourceFacadeError> {
        // 在快照解密前限制全部读取，最终归属验证完成前持续持有许可。
        let _permit = self
            .entry_thumbnails
            .gate
            .try_acquire()
            // discarded-source[business-outcome]: 容量耗尽直接表达可重试的繁忙结果。
            .map_err(|_| ResourceFacadeError::Busy)?;
        let entry_id = EntryId::from(entry_id);
        let snapshot = self
            .deps
            .snapshot
            .read_entry_resource_snapshot(&entry_id)
            .await
            .map_err(|error| ResourceFacadeError::Internal(error.into()))?
            .ok_or(ResourceFacadeError::NotFound)?;
        let preview_id = snapshot
            .preview_representation
            .as_ref()
            .ok_or(ResourceFacadeError::Unavailable)?;
        let representation = snapshot
            .representations
            .iter()
            .find(|representation| &representation.id == preview_id)
            .ok_or(ResourceFacadeError::Unavailable)?;
        if !representation
            .mime_type
            .as_ref()
            .is_some_and(MimeType::is_image)
        {
            return Err(ResourceFacadeError::Unavailable);
        }
        let result = if let Some(cached) = self.entry_thumbnails.cache.get(preview_id) {
            cached
        } else {
            if representation.size_bytes < 0 || representation.size_bytes > MAX_SOURCE_BYTES {
                return Err(ResourceFacadeError::Unavailable);
            }
            let bytes = if let Some(bytes) = representation.inline_data.as_ref() {
                bytes.clone()
            } else if let Some(blob) = representation.blob_id.as_ref() {
                self.deps
                    .blob_store
                    .get(blob)
                    .await
                    .map_err(ResourceFacadeError::Internal)?
            } else {
                return Err(match representation.payload_state {
                    PayloadAvailability::Staged | PayloadAvailability::Processing => {
                        ResourceFacadeError::Busy
                    }
                    _ => ResourceFacadeError::Unavailable,
                });
            };
            if bytes.len() > MAX_SOURCE_BYTES as usize {
                return Err(ResourceFacadeError::Unavailable);
            }
            let generated = self
                .deps
                .generator
                .generate_thumbnail(&bytes)
                .await
                .map_err(ResourceFacadeError::Internal)?;
            let resource = BinaryResourceView {
                mime_type: Some(generated.thumbnail_mime_type.as_str().to_owned()),
                bytes: generated.thumbnail_bytes,
            };
            self.entry_thumbnails
                .cache
                .insert(preview_id.clone(), resource.clone());
            self.entry_thumbnails.cache.run_pending_tasks();
            resource
        };
        let current = self
            .deps
            .snapshot
            .read_entry_resource_snapshot(&entry_id)
            .await
            .map_err(|error| ResourceFacadeError::Internal(error.into()))?
            .ok_or(ResourceFacadeError::NotFound)?;
        if current.event_id != snapshot.event_id
            || current.preview_representation != snapshot.preview_representation
        {
            return Err(ResourceFacadeError::Unavailable);
        }
        Ok(result)
    }
}
