//! 双端复用同一份原始逻辑项映射；不由网络到达顺序或provider推断源身份。
use crate::clipboard::sync::payload_codec::V3BlobRef;
use uc_core::file_transfer::{
    valid_progress_plan, ProgressIdentity, ProgressItem, ProgressItemKind, MAX_PROGRESS_ITEMS,
};
use uc_core::ids::EntryId;
use uuid::Uuid;

pub(crate) struct ProgressOriginalPlan {
    pub source: ProgressIdentity,
    pub items: Vec<ProgressItem>,
}
impl ProgressOriginalPlan {
    pub(crate) fn from_refs(
        source_hint: Option<&EntryId>,
        blobs: &[V3BlobRef],
        directory: bool,
    ) -> Option<Self> {
        if blobs.len() > MAX_PROGRESS_ITEMS {
            return None;
        }
        let source = match source_hint {
            Some(source) => source,
            None => {
                let source = &blobs.first()?.entry_id;
                if blobs.iter().any(|blob| &blob.entry_id != source) {
                    return None;
                }
                source
            }
        };
        let source = Uuid::parse_str(source.as_ref()).ok()?;
        let items = if directory {
            vec![ProgressItem {
                kind: ProgressItemKind::DirectoryUnit,
                index: 0,
            }]
        } else {
            let mut file_index = 0_u16;
            let mut items = Vec::with_capacity(blobs.len());
            for blob in blobs {
                let (kind, index) = match blob.representation_index {
                    Some(index) => (ProgressItemKind::Representation, u16::try_from(index).ok()?),
                    None => {
                        let index = file_index;
                        file_index += 1;
                        (ProgressItemKind::File, index)
                    }
                };
                items.push(ProgressItem { kind, index });
            }
            items
        };
        valid_progress_plan(&items).then_some(Self {
            source: ProgressIdentity::new(*source.as_bytes()),
            items,
        })
    }
}
