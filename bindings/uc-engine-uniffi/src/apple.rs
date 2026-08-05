use std::sync::OnceLock;

use tracing_subscriber::layer::SubscriberExt;

static APPLE_TRACING_INSTALLED: OnceLock<()> = OnceLock::new();

pub(crate) fn install_apple_tracing() {
    APPLE_TRACING_INSTALLED.get_or_init(|| {
        let subscriber = tracing_subscriber::registry()
            .with(tracing_oslog::OsLogger::new("app.uniclipboard", "engine"));
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}
