use std::sync::Arc;

use uc_core::blob::ports::BlobReaderPort;
use uc_core::ports::security::BlobCipherPort;

use super::{
    BlobCipherAdapter, ContentProtection, EncryptedBlobStore, V3EncryptedBlobStore,
    V3InlinePayloadCipher,
};
use crate::blob::BlobStorePort;
use crate::space::InMemorySession;

/// 同一 profile runtime 的 primary payload adapter family。
///
/// inline 与 UCBL 必须由同一个构造器共同选择 V2 或 V3。调用方只能取得
/// 已配对的 port，不能分别组合 V2 inline 与 V3 blob（或反向组合）。
pub struct ProfilePayloadAdapters {
    inline_cipher: Arc<dyn BlobCipherPort>,
    blob_store: Arc<dyn BlobStorePort>,
    blob_reader: Arc<dyn BlobReaderPort>,
}

impl ProfilePayloadAdapters {
    pub fn legacy(inner: Arc<dyn BlobStorePort>, session: Arc<InMemorySession>) -> Self {
        let encrypted = Arc::new(EncryptedBlobStore::new(inner, Arc::clone(&session)));
        Self {
            inline_cipher: Arc::new(BlobCipherAdapter::new(session)),
            blob_store: encrypted.clone(),
            blob_reader: encrypted,
        }
    }

    pub fn v3(inner: Arc<dyn BlobStorePort>, protection: Arc<ContentProtection>) -> Self {
        let encrypted = Arc::new(V3EncryptedBlobStore::new(inner, Arc::clone(&protection)));
        Self {
            inline_cipher: Arc::new(V3InlinePayloadCipher::new(protection)),
            blob_store: encrypted.clone(),
            blob_reader: encrypted,
        }
    }

    pub fn inline_cipher(&self) -> Arc<dyn BlobCipherPort> {
        Arc::clone(&self.inline_cipher)
    }

    pub fn blob_store(&self) -> Arc<dyn BlobStorePort> {
        Arc::clone(&self.blob_store)
    }

    pub fn blob_reader(&self) -> Arc<dyn BlobReaderPort> {
        Arc::clone(&self.blob_reader)
    }
}
