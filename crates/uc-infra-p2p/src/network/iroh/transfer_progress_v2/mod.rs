//! 进度协议的有界交换；新版在Application采用后响应，旧版只提交无关联通知。
mod io;
mod legacy;
mod mapping;
mod outbound;
mod query;
mod receive;
mod rejection;
mod snapshot;
pub use outbound::TransferProgressWire;
pub use receive::TransferProgressInbound;

mod inbound;
pub use inbound::TransferProgressInboundAdapter;
