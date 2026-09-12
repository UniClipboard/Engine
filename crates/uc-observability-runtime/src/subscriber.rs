use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::Layer;

use crate::filter::health_log_enabled;
use crate::local_file::LocalFileRuntime;

/// 宿主自身的日志输出层；共同运行时在首次安装时限定其接收范围。
pub type HostLogLayer = Box<dyn Layer<Registry> + Send + Sync>;
pub(crate) type RuntimeLayer = HostLogLayer;

pub(crate) fn local_health_layer(
    directory: &Path,
    health_accepting: Arc<AtomicBool>,
) -> Result<(RuntimeLayer, Arc<LocalFileRuntime>), ()> {
    let local_file = Arc::new(LocalFileRuntime::new(directory).map_err(|_| ())?);
    let layer = tracing_subscriber::fmt::layer()
        .json()
        .with_ansi(false)
        .with_current_span(false)
        .with_span_list(false)
        .with_writer(local_file.writer())
        .with_filter(dynamic_filter_fn(move |metadata, _| {
            health_accepting.load(Ordering::Acquire) && health_log_enabled(metadata)
        }));
    Ok((Box::new(layer), local_file))
}

#[cfg(target_vendor = "apple")]
pub(crate) fn system_layer() -> RuntimeLayer {
    Box::new(AppleSystemEvents {
        output: tracing_oslog::OsLogger::new("app.uniclipboard", "engine")
            .with_subscriber(Registry::default()),
    })
}

// 系统输出只消费已批准的事件，不跟随宿主或被过滤的 span。
// OsLogger 会直接解引用当前 span；独立的事件输出对象不注册 span，避免其
// 在分层过滤下读取不存在的活动。它不安装为全局 subscriber，也不重新发射事件。
#[cfg(target_vendor = "apple")]
struct AppleSystemEvents {
    output: tracing_subscriber::layer::Layered<tracing_oslog::OsLogger, Registry>,
}

#[cfg(target_vendor = "apple")]
impl Layer<Registry> for AppleSystemEvents {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, Registry>,
    ) {
        tracing::Subscriber::event(&self.output, event);
    }
}

#[cfg(target_os = "android")]
pub(crate) fn system_layer() -> RuntimeLayer {
    match tracing_android::layer("UcEngine") {
        Ok(layer) => Box::new(layer),
        Err(_) => Box::new(tracing_subscriber::layer::Identity::new()),
    }
}

#[cfg(not(any(target_vendor = "apple", target_os = "android")))]
pub(crate) fn system_layer() -> RuntimeLayer {
    Box::new(
        tracing_subscriber::fmt::layer()
            .json()
            .with_ansi(false)
            .with_current_span(false)
            .with_span_list(false),
    )
}
