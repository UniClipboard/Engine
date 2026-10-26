//! Blob publish and fetch use cases.
//!
//! In-memory non-file content is encrypted before entering the persistent
//! transfer store and decrypted after fetch. File payloads use the explicit
//! raw-byte exception; their names, paths, and other metadata remain encrypted
//! in the clipboard event.
//!
//! External callers enter through the facade.

pub(crate) mod facade;
mod failure;
mod fetch_blob;
mod output;
mod progress;
pub(super) use progress::BlobProgressSink;
mod publish_blob;
pub(crate) mod transport;

pub(crate) use failure::fetch_failure_reason;
pub(crate) use fetch_blob::{FetchBlobInput, FetchBlobPathInput, FetchBlobUseCase};
pub(crate) use output::BlobOutputPreparationError;
pub(crate) use publish_blob::{PublishBlobInput, PublishBlobUseCase};

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
