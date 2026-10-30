//! Stable cross-platform interface owned by the UniClipboard engine.

mod assembly;
mod compatibility;
mod contract;
#[cfg(feature = "dev-tools")]
mod dev;
mod engine;
mod operations;
mod runtime;
mod subsystems;

#[cfg(test)]
mod testing;

pub use compatibility::mobile_lan::*;
pub use contract::*;
#[cfg(feature = "dev-tools")]
pub use dev::*;
#[cfg(feature = "dev-tools")]
pub use engine::probe_fail_next_orphan_retry_executor;
#[cfg(feature = "test-binding-worker-panic")]
pub use engine::StartupProbeOutcome;
#[cfg(feature = "test-binding-worker-panic")]
#[doc(hidden)]
pub use engine::{
    probe_release_startup_retry, probe_startup_retry_calls, probe_startup_retry_entered,
    probe_wrap_startup_host,
};
pub use engine::{Engine, EventStream};
pub use engine::{StartupLifecycle, StartupLifecycleInput, StartupReleaseOutcome};
pub use engine::{StartupProgress, StartupProgressInput};
