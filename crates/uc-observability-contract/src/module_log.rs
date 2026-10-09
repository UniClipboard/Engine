//! 模块日志的公共约定：敏感值包装与错误链渲染。
//!
//! 模块日志只写入本地文件与诊断导出，不进入远程遥测。`error =` 字段按 source chain 逐层渲染，
//! 只输出 `io::Error` 与 `serde_json::Error` 的固定摘要；其余层（第三方、anyhow context、仓库自有类型）
//! 一律记为固定占位，不回退到 `Display`。仓库自有错误的原因通过 `uc_core::error_class::ErrorClass`
//! 的固定分类在调用点写入 `error_class` 字段（[ADR-030](../../../docs/design-docs/decisions/030-typed-log-events-and-enforcement.md)）。

use std::error::Error;
use std::fmt;
use std::io;

/// 错误链最多渲染的层数；更深的层折叠为一个固定占位。
pub const MAX_CHAIN_LAYERS: usize = 16;
/// 单层文本的字符上限。
/// 未识别层的固定占位。
pub const OPAQUE_LAYER: &str = "<opaque>";
/// 超出层数上限后的固定占位。
pub const TRUNCATED_LAYERS: &str = "<more layers omitted>";

/// 可能进入日志字段、span 字段或错误文本的敏感值；`Debug` 与 `Display` 均只输出固定占位。
///
/// 适用于设备名、路径、地址、节点标识、邀请、令牌、密钥、剪贴板内容和文件名。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sensitive<T>(pub T);

impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Sensitive<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

/// 渲染后的错误链，由外到内。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorChain {
    pub layers: Vec<String>,
    /// 未识别的层数，用于评估登记覆盖。
    pub opaque_layers: usize,
}

impl ErrorChain {
    pub fn root(&self) -> Option<&str> {
        self.layers.last().map(String::as_str)
    }
}

/// 沿 `source()` 逐层渲染；不整体格式化错误，避免 `Debug` 递归输出整条链。
pub fn render_error_chain(error: &(dyn Error + 'static)) -> ErrorChain {
    let mut layers = Vec::new();
    let mut opaque_layers = 0;
    let mut current = Some(error);
    while let Some(layer) = current {
        if layers.len() == MAX_CHAIN_LAYERS {
            layers.push(TRUNCATED_LAYERS.to_owned());
            break;
        }
        match render_layer(layer) {
            Some(text) => layers.push(text),
            None => {
                opaque_layers += 1;
                layers.push(OPAQUE_LAYER.to_owned());
            }
        }
        current = layer.source();
    }
    ErrorChain {
        layers,
        opaque_layers,
    }
}

fn render_layer(layer: &(dyn Error + 'static)) -> Option<String> {
    if let Some(error) = layer.downcast_ref::<io::Error>() {
        return Some(match error.raw_os_error() {
            Some(code) => format!("io error kind={:?} os_code={code}", error.kind()),
            None => format!("io error kind={:?}", error.kind()),
        });
    }
    if let Some(error) = layer.downcast_ref::<serde_json::Error>() {
        return Some(format!(
            "json error category={:?} line={} column={}",
            error.classify(),
            error.line(),
            error.column()
        ));
    }
    None
}
