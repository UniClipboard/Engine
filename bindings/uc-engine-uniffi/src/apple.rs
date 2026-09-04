use std::path::Path;
use std::sync::OnceLock;

use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

use crate::file_log;

static APPLE_TRACING_INSTALLED: OnceLock<()> = OnceLock::new();

pub(crate) fn install_apple_tracing(logs_dir: &Path) {
    APPLE_TRACING_INSTALLED.get_or_init(|| {
        let subscriber: Box<dyn tracing::Subscriber + Send + Sync> =
            match file_log::file_layer(logs_dir) {
                Some(file_layer) => Box::new(
                    tracing_subscriber::registry()
                        .with(
                            tracing_oslog::OsLogger::new("app.uniclipboard", "engine")
                                .with_filter(filter_fn(file_log::persistent_sink_enabled)),
                        )
                        .with(file_layer),
                ),
                None => Box::new(
                    tracing_subscriber::registry().with(
                        tracing_oslog::OsLogger::new("app.uniclipboard", "engine")
                            .with_filter(filter_fn(file_log::persistent_sink_enabled)),
                    ),
                ),
            };
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}
