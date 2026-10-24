//! 反向进度的完整业务负责人；原始发送意图授权，登记及终态采用共享同一投影。
mod error;
mod model;
mod ports;
mod receiver;
pub use error::ProgressReceiveError;
pub use model::{
    ProgressBound, ProgressProjectionSnapshot, ProgressTaskDescriptor, ProgressTerminalNotice,
};
pub use ports::{CurrentProgressTaskPort, ProgressReceiverPort};
pub use receiver::TransferProgressReceiver;

#[cfg(test)]
mod tests;
