//! 原发送意图拥有全局投影；网络帧只能查询已授权、仍保留的任务。
mod handle;
mod intent;
pub use intent::ProgressIntent;
mod model;
mod receive;
mod refresh;
mod unattributed;
pub use refresh::TransferProgressReconciler;
mod worker;
pub use handle::ProgressProjectionRegistry;
pub use model::{ProgressProjectionAdmission, ProgressProjectionEvent, ProgressProjectionKey};
pub use worker::ProgressProjectionWorker;
#[cfg(test)]
mod tests;
