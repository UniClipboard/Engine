//! Network protocol types.

pub mod protocol;
pub mod session;
pub mod trusted_networks;

pub use protocol::{BinaryRepresentation, ClipboardBinaryPayload};
pub use protocol::{
    FileTransferMapping, MIME_IMAGE_PREFIX, MIME_TEXT_HTML, MIME_TEXT_PLAIN, MIME_TEXT_RTF,
};
pub use session::SessionId;
pub use trusted_networks::{
    is_private_address, IpNetwork, LenientTrustedNetworks, TrustedNetworkEntryError,
    TrustedNetworkRejection, TrustedNetworks,
};
