//! 新进度协议的有界交换；Application 成功采用后才发送业务响应。
mod mapping;
mod receive;
pub use receive::TransferProgressInbound;
