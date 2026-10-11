//! 原生连接能力及会话唯一准入登记的桥接。
mod admission;
mod backend;
mod connection;
mod stream;
pub use admission::IrohConnectionAdmission;
pub use backend::IrohConnectivityBackend;
pub(crate) use connection::IrohPeerConnection;

mod clipboard_handler;
pub use clipboard_handler::IrohClipboardHandler;
