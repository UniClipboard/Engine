mod blob_writer;
mod domain;
mod filesystem_store;
mod hashing;
mod lease;
mod reclaimer;
mod repository_port;
mod store_port;

pub use blob_writer::BlobWriter;
pub use domain::{Blob, BlobStorageLocator};
pub use filesystem_store::{FilesystemBlobStore, SwitchableFilesystemBlobStore};
pub use lease::{BlobLeaseRegistry, BLOB_RECLAIM_CLAIM_GRACE};
pub use reclaimer::BlobReclaimer;
pub use repository_port::{BlobRepositoryPort, UnreferencedBlobDeletion};
pub use store_port::{BlobFileRemoval, BlobStorePort, StoredPathBlob};
// Re-export uc-core's BlobWriterPort under the existing path to keep
// downstream `uc_infra_local::blob::BlobWriterPort` imports working during the
// transition. New code should import directly from `uc_core::blob::ports`.
pub use uc_core::blob::ports::{BlobReaderPort, BlobWriterPort};
