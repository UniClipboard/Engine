//! 停写资料目录的本机原样归档；不解密文件、不访问钥匙串。
//! 调用方必须先停止所有写入者；本模块不打开 SQLite，也不执行迁移。

mod archive;
mod error;
mod model;
mod storage;
pub(super) mod tree;

pub use archive::ProfileBackupArchive;
pub use error::ProfileBackupArchiveError;
pub use model::{ProfileArchiveReceipt, ProfileBackupSource};
pub(super) use storage::{private_new_file, sync_directory};

#[cfg(test)]
mod tests;
