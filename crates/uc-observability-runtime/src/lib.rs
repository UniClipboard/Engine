//! 宿主进程唯一拥有的运行诊断装配。

#[cfg(target_os = "android")]
mod android;
mod config;
mod filter;
mod local_file;
mod local_log_processor;
mod remote_health;
mod runtime;
mod status;
mod subscriber;
mod telemetry;

#[cfg(target_os = "android")]
pub use android::*;
pub use config::*;
pub use local_file::managed_log_files;
pub use runtime::*;
pub use status::*;
pub use subscriber::HostLogLayer;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
