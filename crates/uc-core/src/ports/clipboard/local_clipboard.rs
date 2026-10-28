//! Clipboard port - abstracts local clipboard access
//!
//! This port defines the interface for clipboard operations including
//! reading, writing, and monitoring clipboard changes.

use crate::clipboard::SystemClipboardSnapshot;
use anyhow::Result;
use async_trait::async_trait;

/// Clipboard port - abstracts local clipboard access
///
/// This trait provides a platform-agnostic interface to clipboard functionality,
/// allowing use cases to interact with the clipboard without depending on
/// platform-specific implementations.
#[async_trait]
pub trait SystemClipboardPort: Send + Sync {
    /// 读取内容及与该次读取绑定的宿主身份。没有可靠身份时返回 None。
    fn read_observation(&self) -> Result<(SystemClipboardSnapshot, Option<Vec<u8>>)> {
        self.read_snapshot().map(|snapshot| (snapshot, None))
    }

    /// 写入完成后返回宿主能证明属于该次写入的身份。
    fn write_with_receipt(&self, snapshot: SystemClipboardSnapshot) -> Result<Option<Vec<u8>>> {
        self.write_snapshot(snapshot).map(|()| None)
    }

    /// Read current clipboard content
    ///
    /// Returns the current clipboard content as a Payload, which can contain
    /// text, images, files, or other supported content types.
    fn read_snapshot(&self) -> Result<SystemClipboardSnapshot>;

    fn write_snapshot(&self, snapshot: SystemClipboardSnapshot) -> Result<()>;
}
