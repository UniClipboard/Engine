//! 当前剪贴板完整网络交换；业务应用仍由原串行负责人承担。
mod error;
mod model;
mod ports;
mod preparation;
mod pull;
mod pull_receiver;
mod receiver;
mod sender;
#[cfg(test)]
mod tests;

pub use error::ActiveClipboardTransportError;
pub use model::{ActivePullResponse, ActiveStateMessage};
pub use ports::{
    ActiveClipboardWirePort, ActivePullLocalWorkPort, ActivePullReceiverPort, ActivePullWork,
    ActiveStateReceiverPort,
};
pub use receiver::ActiveStateReceiver;
pub use sender::ActiveStateSender;

use std::time::Duration;
pub const ACTIVE_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const CLOSE_BARRIER_TIMEOUT: Duration = Duration::from_secs(3);

pub use preparation::{
    ActivePullPreparationCancelled, ActivePullPreparationError, ActivePullPreparationPort,
    ActivePullPreparationStop,
};

pub use pull::ActivePullClient;
pub use pull_receiver::ActivePullReceiver;
