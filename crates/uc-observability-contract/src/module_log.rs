//! 模块日志的公共约定：敏感值包装与错误层识别登记。
//!
//! 模块日志只写入本地文件与诊断导出，不进入远程遥测。错误按 source chain 逐层渲染，
//! 每一层只有在类型被识别时才输出文本；无法识别的层记为固定占位，不回退到 `Display`。
//! 识别只依赖 `downcast`：稳定版 Rust 无法从 `&dyn Error` 读取类型名，anyhow 的 context 层也无法
//! `downcast`，因此这些层始终按未识别处理。

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::RwLock;

/// 错误链最多渲染的层数；更深的层折叠为一个固定占位。
pub const MAX_CHAIN_LAYERS: usize = 16;
/// 单层文本的字符上限。
const MAX_LAYER_CHARS: usize = 256;
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

/// 识别一层错误：返回 `Some(可输出文本)` 表示类型已登记且文本安全，`None` 表示未识别。
pub type ErrorLayerRenderer = fn(&(dyn Error + 'static)) -> Option<String>;

static RENDERERS: RwLock<Vec<ErrorLayerRenderer>> = RwLock::new(Vec::new());

/// 登记一组错误层识别函数；同一函数重复登记只保留一份。
///
/// 每个拥有错误类型的 crate 提供一个登记入口，由 Engine 装配时统一调用。
pub fn register_error_layer_renderers(renderers: &[ErrorLayerRenderer]) {
    let mut registered = RENDERERS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for renderer in renderers {
        if !registered
            .iter()
            .any(|known| std::ptr::fn_addr_eq(*known, *renderer))
        {
            registered.push(*renderer);
        }
    }
}

/// 生成识别函数：列出的类型按 `Display` 渲染。
///
/// 类型的 `#[error]` 文本必须只含固定文字、枚举变体名与 `Sensitive` 包装的值。
#[macro_export]
macro_rules! log_safe_errors {
    ($vis:vis fn $name:ident => [$($ty:ty),+ $(,)?]) => {
        $vis fn $name(layer: &(dyn ::std::error::Error + 'static)) -> Option<String> {
            // `#[source] Box<T>` 字段得到的错误对象是 `Box<T>` 而不是 `T`，两种都要识别；`Box` 的文本与 `T` 相同。
            $( if layer.is::<$ty>() || layer.is::<::std::boxed::Box<$ty>>() { return Some(layer.to_string()); } )+
            None
        }
    };
}

/// 失败时在调用点记录一条 WARN，携带完整错误链，并原样返回结果。
///
/// 用于替代 `#[instrument(err)]`：后者只记录最外层 `Display`。宏在调用处展开，事件的模块路径与源码位置属于调用方。
#[macro_export]
macro_rules! warn_on_error {
    ($result:expr, $message:literal) => {{
        let result = $result;
        if let ::core::result::Result::Err(error) = &result {
            ::tracing::warn!(error = error as &dyn ::std::error::Error, $message);
        }
        result
    }};
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
            Some(text) => layers.push(truncate(text)),
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
    let registered = RENDERERS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    registered.iter().find_map(|renderer| renderer(layer))
}

fn truncate(text: String) -> String {
    match text.char_indices().nth(MAX_LAYER_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}
