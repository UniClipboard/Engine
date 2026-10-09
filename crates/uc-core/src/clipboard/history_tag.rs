//! 历史标签领域模型：用户手动维护的本机历史分类。
//!
//! 历史标签与内置规则标签（`link`、`image` 等）不同：它的成员关系只来自用户显式
//! 关联，不经任何规则求值；它的定义与关联是本机历史元数据，不进入跨设备同步。
//! 标签身份是与名称无关的不透明 id，名称只是可修改的属性。

use unicode_normalization::UnicodeNormalization;

/// 标签名称在规范化后允许的最大 Unicode 标量数。
pub const HISTORY_TAG_NAME_MAX_CHARS: usize = 64;
/// 单个 profile 最多保存的历史标签数。
pub const HISTORY_TAG_MAX_COUNT: usize = 1000;
/// 单次批量关联、移除或汇总最多包含的条目数。
pub const HISTORY_TAG_MAX_BATCH_ENTRIES: usize = 1000;
/// 单次合并最多包含的来源标签数。
pub const HISTORY_TAG_MAX_MERGE_SOURCES: usize = 100;

/// 名称不满足产品契约的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HistoryTagNameError {
    #[error("history tag name is empty")]
    Empty,
    #[error("history tag name contains control characters")]
    ControlCharacter,
    #[error("history tag name is too long")]
    TooLong,
}

/// 已规范化的历史标签名称。
///
/// 规范化顺序固定为：去掉首尾 Unicode 空白，再转为 NFC。展示保留用户输入的大小写；
/// 同名判断使用 [`HistoryTagName::identity_key`]（NFC 后的完整 Unicode 小写），因此
/// `"Work"`、`" work "` 与 `"WORK"` 视为同一标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagName {
    display: String,
    identity_key: String,
}

impl HistoryTagName {
    /// 按产品契约规范化并校验用户输入的名称。
    pub fn parse(raw: &str) -> Result<Self, HistoryTagNameError> {
        let display: String = raw.trim().nfc().collect();
        if display.is_empty() {
            return Err(HistoryTagNameError::Empty);
        }
        if display.chars().any(char::is_control) {
            return Err(HistoryTagNameError::ControlCharacter);
        }
        if display.chars().count() > HISTORY_TAG_NAME_MAX_CHARS {
            return Err(HistoryTagNameError::TooLong);
        }
        let identity_key = display.to_lowercase().nfc().collect();
        Ok(Self {
            display,
            identity_key,
        })
    }

    /// 展示用名称（保留用户输入的大小写）。
    pub fn as_str(&self) -> &str {
        &self.display
    }

    /// 同名判断使用的比较键。
    pub fn identity_key(&self) -> &str {
        &self.identity_key
    }

    /// 取出展示用名称。
    pub fn into_string(self) -> String {
        self.display
    }
}
