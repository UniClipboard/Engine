use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::Layer;

use crate::file_statistics::LocalDiagnosticSource;
use crate::filter::health_log_enabled;
use crate::local_file::LocalFileRuntime;
use crate::local_recording::LocalRecordingState;

/// 宿主自身的日志输出层；共同运行时在首次安装时限定其接收范围。
pub type HostLogLayer = Box<dyn Layer<Registry> + Send + Sync>;
pub(crate) type RuntimeLayer = HostLogLayer;

/// 健康记录的本地关联来源；记录状态晚于文件队列创建，所以由运行时装配完成后一次性补上。
pub(crate) type HealthRecording = Arc<OnceLock<Arc<LocalRecordingState>>>;

pub(crate) fn local_health_layer(
    directory: &Path,
    health_accepting: Arc<AtomicBool>,
    recording: HealthRecording,
) -> Result<(RuntimeLayer, Arc<LocalFileRuntime>), ()> {
    // discarded-source[observability-init]: `std::io::Error`: setup failure only degrades to a fixed status and the log channel is not built yet
    let local_file = Arc::new(LocalFileRuntime::new(directory).map_err(|_| ())?);
    let layer = HealthLayer {
        file: Arc::clone(&local_file),
        recording,
    }
    .with_filter(dynamic_filter_fn(move |metadata, _| {
        health_accepting.load(Ordering::Acquire) && health_log_enabled(metadata)
    }));
    Ok((Box::new(layer), local_file))
}

/// 健康记录与其他本地记录使用同一份运行信封（`run_id`、`local_schema_version`、`source_commit` 等）。
struct HealthLayer {
    file: Arc<LocalFileRuntime>,
    recording: HealthRecording,
}

impl Layer<Registry> for HealthLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, Registry>,
    ) {
        let mut fields = FieldCollector::default();
        event.record(&mut fields);
        let metadata = event.metadata();
        let mut record = serde_json::json!({
            "timestamp": chrono::Utc::now()
                .to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
            "level": metadata.level().as_str(),
            "target": metadata.target(),
            "fields": fields.0,
            "local_schema_version": 1,
        });
        if let Some(recording) = self.recording.get() {
            // 健康记录与当前连接无关，隔离外层上下文，避免被附加连接或对端引用。
            let _context = opentelemetry::Context::new().attach();
            recording.annotate(&mut record);
        }
        let Ok(mut bytes) = serde_json::to_vec(&record) else {
            self.file.record_rejection();
            return;
        };
        bytes.push(b'\n');
        self.file
            .writer()
            .write_record(&bytes, LocalDiagnosticSource::Runtime);
    }
}

#[derive(Default)]
struct FieldCollector(serde_json::Map<String, serde_json::Value>);

impl tracing::field::Visit for FieldCollector {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), format!("{value:?}").into());
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.into());
    }
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
