//! SDK 编码与存储交换；只接受 Uni 租约，没有 Endpoint 或自拨能力。
mod adapter;
mod failure;
mod io;
mod provide;
mod receive;
mod response_size;

pub use adapter::IrohBlobExchangeAdapter;

mod handler;
pub use handler::IrohBlobHandler;

#[cfg(test)]
mod managed_tests;
