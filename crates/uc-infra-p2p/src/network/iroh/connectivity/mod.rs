//! 原生连接能力及会话唯一准入登记的桥接。
mod admission;
mod backend;
mod connection;
mod stream;
pub use admission::IrohConnectionAdmission;
pub use backend::{ConnectivityProfile, IrohConnectivityBackend};
pub(crate) use connection::IrohPeerConnection;

mod clipboard_handler;
pub use clipboard_handler::IrohClipboardHandler;

mod presence_wire;
pub use presence_wire::PeerReachabilityWire;

mod presence_environment;
pub(crate) use presence_environment::PresenceEnvironment;
mod presence_handler;
pub use presence_handler::IrohPeerReachabilityHandler;

mod progress_handler;
pub use progress_handler::IrohTransferProgressHandler;
