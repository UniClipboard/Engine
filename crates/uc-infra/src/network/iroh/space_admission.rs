//! Iroh 准入通道：对外保留完整客户端与服务端能力，内部细节由各模块拥有。
mod client;
mod connection;
mod credential;
mod crypto;
mod diagnostics;
mod errors;
mod exchange;
mod route;
mod server;

pub use client::IrohSpaceAdmissionTransport;
pub use credential::{
    SpaceAdmissionChannelCredentialError, SpaceAdmissionChannelCredentialPort,
    SponsorOpaqueMaterial,
};
pub(crate) use route::decode_space_admission_route;
pub use route::encode_space_admission_route;
pub use server::IrohSpaceAdmissionHandler;

pub const SPACE_ADMISSION_ALPN: &[u8] = b"/uniclipboard/space-admission/1";

#[cfg(test)]
mod tests;
