//! 宿主进程唯一拥有的运行诊断装配。

mod config;
mod local_file;
mod runtime;

pub use config::*;
pub use local_file::managed_log_files;
pub use runtime::*;

#[cfg(test)]
mod tests;
