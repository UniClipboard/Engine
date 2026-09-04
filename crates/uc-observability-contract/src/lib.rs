//! Portable observability contracts shared by application logic and host adapters.

pub mod analytics;
pub mod diagnostics;
pub mod task_supervision;

pub use task_supervision::spawn_supervised;
