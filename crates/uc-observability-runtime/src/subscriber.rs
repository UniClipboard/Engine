use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tracing_subscriber::filter::dynamic_filter_fn;
use tracing_subscriber::registry::Registry;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::HEALTH_TARGET;

use crate::filter::local_sink_enabled;
use crate::local_file::LocalFileRuntime;

pub(crate) type RuntimeLayer = Box<dyn Layer<Registry> + Send + Sync>;

pub(crate) fn local_file_layer(
    directory: &Path,
    telemetry_accepting: Arc<AtomicBool>,
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
            local_record_enabled(metadata, &telemetry_accepting, &health_accepting)
        }));
    Ok((Box::new(layer), local_file))
}

#[cfg(target_vendor = "apple")]
pub(crate) fn system_layer() -> RuntimeLayer {
    Box::new(tracing_oslog::OsLogger::new("app.uniclipboard", "engine"))
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

fn local_record_enabled(
    metadata: &tracing::Metadata<'_>,
    telemetry_accepting: &AtomicBool,
    health_accepting: &AtomicBool,
) -> bool {
    let accepting = if metadata.target() == HEALTH_TARGET {
        health_accepting
    } else {
        telemetry_accepting
    };
    accepting.load(Ordering::Acquire) && local_sink_enabled(metadata)
}
