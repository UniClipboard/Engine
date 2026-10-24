//! 反向进度的完整业务负责人；原始发送意图授权，登记及终态采用共享同一投影。
mod error;
mod model;
mod ports;
mod producer;
mod receiver;
pub use producer::TransferProgressProducer;
mod source;
pub use error::ProgressReceiveError;
pub use model::{
    ProgressBound, ProgressProjectionSnapshot, ProgressSourceTask, ProgressTaskDescriptor,
    ProgressTerminalNotice,
};
pub use ports::{
    CurrentProgressTaskPort, ProgressReceiverPort, ProgressRegistrationWirePort,
    ProgressTaskAuthorityPort,
};
pub use receiver::TransferProgressReceiver;
pub(crate) use source::ProgressSourceCatalog;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod source_tests;
