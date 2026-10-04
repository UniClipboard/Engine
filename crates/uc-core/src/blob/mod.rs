//! Blob 领域模块。
//!
//! 只保存 blob 存储的读写 port 抽象。blob 值对象本身（`Blob`、`BlobStorageLocator`）
//! 与存储格式细节位于 `uc-infra-local`，这里只暴露跨层契约。

pub mod ports;
