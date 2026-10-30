use std::sync::Arc;

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
    pub gate: Arc<Semaphore>,
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
            gate: Arc::new(Semaphore::new(1)),
        }
    }
}

impl ResourceFacade {
    /// 根据条目的当前预览选择读取缩略图；缺失时在容量限制内生成。
    pub async fn entry_thumbnail(
        &self,
        entry_id: &str,
    ) -> Result<BinaryResourceView, ResourceFacadeError> {
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
            // 在读取原图前限制并发，禁止等待队列持有多份原图。
            let _permit = Arc::clone(&self.entry_thumbnails.gate)
                .try_acquire_owned()
                // discarded-source[business-outcome]: 容量耗尽直接表达可重试的繁忙结果。
                .map_err(|_| ResourceFacadeError::Busy)?;
            if let Some(cached) = self.entry_thumbnails.cache.get(preview_id) {
                cached
            } else {
                let resource = if self
                    .deps
                    .thumbnail_repo
                    .get_by_representation_id(preview_id)
                    .await
                    .map_err(ResourceFacadeError::Internal)?
                    .is_some()
                {
                    self.thumbnail(preview_id.inner()).await?
                } else {
                    if representation.size_bytes < 0 || representation.size_bytes > MAX_SOURCE_BYTES
                    {
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
                    BinaryResourceView {
                        mime_type: Some(generated.thumbnail_mime_type.as_str().to_owned()),
                        bytes: generated.thumbnail_bytes,
                    }
                };
                self.entry_thumbnails
                    .cache
                    .insert(preview_id.clone(), resource.clone());
                self.entry_thumbnails.cache.run_pending_tasks();
                resource
            }
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
