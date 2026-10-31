//! Iroh-native pairing infrastructure.
//!
//! * [`invitation_resolver`] — 通过完整邀请、rendezvous 或 mDNS 解析新准入邀请。
//! * [`code_mint`] — sponsor-side local code generation (rendezvous no
//!   longer mints).
//! * [`discovery_constants`] — shared mDNS service name and TXT field
//!   keys; keeping these in one module prevents publisher/resolver drift.
//! * [`mdns_publisher`] / [`mdns_resolver`] — window-scoped LAN discovery
//!   channel for the invitation code. Cohabits with the cloud channel
//!   (`crate::rendezvous`) so first-pair-no-WAN can succeed without
//!   forcing the user to flip any setting.
//!
pub mod code_mint;
pub mod discovery_constants;
pub mod invitation_resolver;
pub mod mdns_publisher;
pub mod mdns_resolver;

pub use code_mint::mint_invitation_code;
pub use invitation_resolver::PairingInvitationResolverAdapter;
pub use mdns_publisher::{MdnsPairingPublisher, MdnsPublisherError, PublisherHandle};
pub use mdns_resolver::{MdnsPairingResolver, MdnsResolverError};
