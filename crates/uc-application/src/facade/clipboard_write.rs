//! ADR-018 stage 3 transitional re-exports: Engine still constructs the
//! clipboard write internals while the write/capture entry points are being
//! consolidated; stage 5 removes them from the whitelist.

pub use crate::clipboard::write::{
    restore_broadcast_channel, ClipboardWriteCoordinator, ClipboardWriteIntent,
    LocalActiveRegisterAdvancer, MobileConsumabilityProbe, MobileConsumableBackfill,
    MobileConsumableRefBackfill, RestoreBroadcastReceiver, RestoreBroadcastRequest,
    RestoreBroadcastTrigger,
};
