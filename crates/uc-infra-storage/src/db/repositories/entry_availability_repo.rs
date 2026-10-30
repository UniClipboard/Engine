//! 从SQLite表示状态、已认证文件清单和实际文件系统派生内容可用性。
//! 每次实时核对，不保存会随物化／外部文件修改而失效的可用标志。

mod files;

use anyhow::Error as SourceError;
use files::local_path_is_available;
use std::path::PathBuf;
use std::sync::Arc;

use diesel::prelude::*;
use tracing::instrument;
use uc_core::clipboard::{
    is_file_mime_or_format, ClipboardRepositoryError, EntryFileSet, MimeType,
};
use uc_core::crypto::{
    aad,
    domain::{Aad, Ciphertext},
};
use uc_core::ids::{EntryId, EventId, FormatId, RepresentationId};
use uc_core::ports::clipboard::{CheckEntryAvailabilityPort, EntryFileSetRepositoryPort};
use uc_core::ports::security::BlobCipherPort;

use crate::db::ports::DbExecutor;
use crate::db::schema::{clipboard_entry, clipboard_snapshot_representation};

/// Core统一分类后的文件列表标记、解密路径和表示状态。
type RepRow = (bool, Option<Vec<u8>>, String);
type StoredRepRow = (String, String, Option<String>, Option<Vec<u8>>, String);

pub struct DieselEntryAvailabilityRepository<E> {
    executor: E,
    cipher: Arc<dyn BlobCipherPort>,
    file_sets: Arc<dyn EntryFileSetRepositoryPort>,
}

impl<E> DieselEntryAvailabilityRepository<E> {
    pub fn new(
        executor: E,
        cipher: Arc<dyn BlobCipherPort>,
        file_sets: Arc<dyn EntryFileSetRepositoryPort>,
    ) -> Self {
        Self {
            executor,
            cipher,
            file_sets,
        }
    }
}

#[async_trait::async_trait]
impl<E> CheckEntryAvailabilityPort for DieselEntryAvailabilityRepository<E>
where
    E: DbExecutor,
{
    #[instrument(
        name = "infra.sqlite.is_entry_available",
        skip_all,
        fields(operation = "is_entry_available", entry_id = %entry_id)
    )]
    async fn is_entry_available(
        &self,
        entry_id: &EntryId,
    ) -> Result<bool, ClipboardRepositoryError> {
        let entry_id_str = entry_id.to_string();

        // 读取表示后立即释放连接，文件系统核对不占用SQLite连接。
        let saved: Option<(String, Vec<StoredRepRow>)> = self
            .executor
            .run(move |conn| {
                let event_id: Option<String> = clipboard_entry::table
                    .filter(clipboard_entry::entry_id.eq(&entry_id_str))
                    .select(clipboard_entry::event_id)
                    .first::<String>(conn)
                    .optional()?;
                let Some(event_id) = event_id else {
                    return Ok(None);
                };
                let reps = clipboard_snapshot_representation::table
                    .filter(clipboard_snapshot_representation::event_id.eq(&event_id))
                    .select((
                        clipboard_snapshot_representation::id,
                        clipboard_snapshot_representation::format_id,
                        clipboard_snapshot_representation::mime_type,
                        clipboard_snapshot_representation::inline_data,
                        clipboard_snapshot_representation::payload_state,
                    ))
                    .load::<StoredRepRow>(conn)?;
                Ok(Some((event_id, reps)))
            })
            .map_err(|e| {
                ClipboardRepositoryError::Storage(e.context("check entry availability").into())
            })?;

        // 不存在的条目不可用。
        let Some((event_id, stored)) = saved else {
            return Ok(false);
        };
        if stored
            .iter()
            .any(|(_, _, _, _, state)| state == "Failed" || state == "Lost")
        {
            return Ok(false);
        }
        let event_id = EventId::from(event_id);
        let mut reps = Vec::with_capacity(stored.len());
        for (id, format, mime, inline, state) in stored {
            let file_list =
                is_file_mime_or_format(mime.map(MimeType).as_ref(), &FormatId::from(format));
            // 文件路径必须先通过原表示身份认证；密文不能当URI或不可用占位读取。
            let inline = if file_list {
                match inline {
                    Some(bytes) => {
                        let context = aad::for_inline(&event_id, &RepresentationId::from(id));
                        Some(
                            self.cipher
                                .decrypt(&Ciphertext::new(bytes), &Aad::from(context.as_slice()))
                                .await
                                .map_err(|source| {
                                    ClipboardRepositoryError::Storage(
                                        SourceError::new(source)
                                            .context("authenticate entry file availability")
                                            .into(),
                                    )
                                })?
                                .into_bytes(),
                        )
                    }
                    None => None,
                }
            } else {
                None
            };
            reps.push((file_list, inline, state));
        }
        let file_set = if reps.iter().any(|(file_list, _, _)| *file_list) {
            self.file_sets.load(entry_id).await.map_err(|source| {
                ClipboardRepositoryError::Storage(
                    SourceError::new(source)
                        .context("load entry availability manifest")
                        .into(),
                )
            })?
        } else {
            None
        };
        // 分块异步读回随本次调用取消，不分离无法回收的整目录校验任务。
        Ok(reps_indicate_available(&reps, file_set.as_ref()).await)
    }
}

/// 所有表示就绪且无缺失占位；文件列表中的本地内容通过实际完整性检查。
async fn reps_indicate_available(reps: &[RepRow], file_set: Option<&EntryFileSet>) -> bool {
    if reps.is_empty()
        || file_set.is_some_and(|files| {
            files.has_directory_structure() && files.has_ambiguous_root_identity()
        })
    {
        return false;
    }
    for (file_list, inline, payload_state) in reps {
        if payload_state == "Failed" || payload_state == "Lost" {
            return false;
        }
        if *file_list {
            // 缺少文件列表正文时不能确认本地内容完整。
            let Some(bytes) = inline.as_deref() else {
                return false;
            };
            if !file_list_is_fully_present(bytes, file_set).await {
                return false;
            }
        }
    }
    true
}

/// 普通文件须可读；目录须有已认证清单且所有成员与实际内容一致。
async fn file_list_is_fully_present(bytes: &[u8], file_set: Option<&EntryFileSet>) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("uniclip-missing:") {
            return false;
        }
        if let Some(path) = file_uri_to_path(line) {
            if !local_path_is_available(&path, file_set).await {
                return false;
            }
        } else if line.starts_with("file://") {
            // 无法解析本地文件URI时保持不可用，避免传播过期内容。
            return false;
        }
        // http/data等非文件URI不要求本地文件。
    }
    true
}

fn file_uri_to_path(line: &str) -> Option<PathBuf> {
    if line.starts_with("file://") {
        url::Url::parse(line)
            .ok()
            .and_then(|u| u.to_file_path().ok())
    } else if line.starts_with('/') || line.contains(":\\") {
        // 部分宿主直接保存绝对路径。
        Some(PathBuf::from(line))
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
