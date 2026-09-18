//! Small shared capabilities without business ownership.
//!
//! Event delivery, in-memory caches and similar cross-domain helpers live
//! here; business sequencing, retry policy and persistence recovery stay in
//! their owning domains (ADR-018).

pub(crate) mod host_event_bus;
pub(crate) mod host_event_publisher;
pub(crate) mod outbound_entry_cache;
