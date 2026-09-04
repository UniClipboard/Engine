use std::path::Path;
use std::sync::OnceLock;

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::Layer;

/// 文件层 writer 与其 worker guard 常驻进程生命周期。guard 被释放时滚动
/// 线程会停止接收后续日志，因此 writer 与 guard 都必须保存在进程级单例中；
/// 后续调用复用首次建立的 writer，重复构建只会被丢弃且不破坏已有写入。
static FILE_LOG_WRITER: OnceLock<NonBlocking> = OnceLock::new();
static FILE_LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// 构建按天滚动的文件层。日志目录创建或滚动器初始化失败时返回 `None`，
/// 调用方应降级为仅系统日志层，不影响 Engine 启动。
///
/// 文件名按天滚动，格式为 `engine.2026-08-08.txt`；文件层只接收
/// `info` 及以上级别，系统日志层不加过滤。
pub(crate) fn file_layer<S>(logs_dir: &Path) -> Option<Box<dyn Layer<S> + Send + Sync>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let writer = file_layer_writer(logs_dir)?;
    Some(Box::new(
        tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(false)
            .with_filter(filter_fn(persistent_sink_enabled)),
    ))
}

/// 持久与平台系统输出默认拒绝普通模块记录，只允许已经逐字段审核的
/// 稳定目标。远程输出还会在共同运行时中进一步收窄到 `uc.telemetry`。
pub(crate) fn persistent_sink_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    matches!(
        *metadata.level(),
        tracing::Level::ERROR | tracing::Level::WARN | tracing::Level::INFO
    ) && audited_persistent_target(metadata.target())
}

fn audited_persistent_target(target: &str) -> bool {
    matches!(
        target,
        "uc.telemetry"
            | "observability.health"
            | "admission.performance"
            | "membership.performance"
            | "storage.performance"
    )
}

fn file_layer_writer(logs_dir: &Path) -> Option<NonBlocking> {
    if let Some(writer) = FILE_LOG_WRITER.get() {
        return Some(writer.clone());
    }
    std::fs::create_dir_all(logs_dir).ok()?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("engine")
        .filename_suffix("txt")
        .build(logs_dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    match FILE_LOG_WRITER.set(writer) {
        Ok(()) => {
            let _ = FILE_LOG_GUARD.set(guard);
            FILE_LOG_WRITER.get().cloned()
        }
        Err(existing) => Some(existing.clone()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use tempfile::tempdir;
    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

    #[test]
    fn persistent_sinks_accept_only_audited_targets() {
        assert!(audited_persistent_target("uc.telemetry"));
        assert!(audited_persistent_target("admission.performance"));
        assert!(!audited_persistent_target("uc_application::clipboard"));
    }

    fn read_written_log(directory: &Path, expected: &str) -> String {
        for _ in 0..200 {
            if let Ok(entries) = std::fs::read_dir(directory) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let name = path.file_name().and_then(|name| name.to_str());
                    if name.is_some_and(|name| name.starts_with("engine.")) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if content.contains(expected) {
                                return content;
                            }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        String::new()
    }

    #[test]
    fn file_layer_degrades_on_unwritable_directory_and_writes_info_logs() {
        let directory = tempdir().expect("temp dir");

        let blocked = directory.path().join("blocked");
        std::fs::write(&blocked, b"file in the way").expect("write blocker");
        assert!(
            file_layer::<tracing_subscriber::registry::Registry>(&blocked).is_none(),
            "unwritable directory must degrade to system layers only"
        );

        let layer = file_layer(directory.path()).expect("file layer");
        let subscriber = tracing_subscriber::registry().with(layer);
        let _default_guard = tracing::subscriber::set_default(subscriber);

        tracing::info!(target: "uc.telemetry", outcome = "ok", "mobile file log line");
        tracing::warn!(
            target: "uc_application::clipboard",
            path = "/private/sensitive.txt",
            "must not reach a persistent sink"
        );
        tracing::debug!("must not reach the file");

        let content = read_written_log(directory.path(), "mobile file log line");
        assert!(
            content.contains("mobile file log line"),
            "info log must be written to the daily file"
        );
        assert!(
            content.contains("outcome=\"ok\""),
            "structured field must be written"
        );
        assert!(
            !content.contains("/private/sensitive.txt"),
            "unapproved targets must not reach the file"
        );
        assert!(
            !content.contains("must not reach the file"),
            "debug logs must be filtered out"
        );
    }
}
