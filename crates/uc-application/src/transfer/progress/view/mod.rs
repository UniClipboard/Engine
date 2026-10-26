//! 完整本地状态读取与显式核对；成员授权和对端结果分类留在Application。
mod error;
mod intent;
pub(crate) use intent::ReconcileSentEntryPort;
pub use intent::SentEntryReconciliation;
mod mapping;
mod owner;
pub use error::TransferProgressQueryError;
pub(crate) use owner::ProgressViewOwner;
pub use owner::TransferProgressReconciliation;

#[cfg(test)]
mod tests;
