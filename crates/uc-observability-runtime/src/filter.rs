use uc_observability_contract::diagnostics::{HEALTH_TARGET, TELEMETRY_TARGET};

const DIAGNOSTIC_OWNER: &str = "uc_observability_contract::diagnostics";
const RUNTIME_HEALTH_OWNER: &str = "uc_observability_runtime::remote_health";

const SPAN_FIELDS: &[&str] = &[
    "uc.flow.id",
    "uc.domain",
    "uc.operation",
    "uc.role",
    "otel.name",
    "otel.kind",
    "otel.status_code",
];

const EVENT_FIELDS: &[&str] = &[
    "event.name",
    "uc.domain",
    "uc.operation",
    "uc.role",
    "uc.outcome",
    "error.type",
    "duration_ms",
];

const HEALTH_FIELDS: &[&str] = &[
    "event.name",
    "task.kind",
    "error.type",
    "task.completed.count",
    "task.timed_out.count",
    "task.join_error.count",
];

pub(crate) fn remote_span_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.is_span()
        && metadata.target() == TELEMETRY_TARGET
        && metadata.module_path() == Some(DIAGNOSTIC_OWNER)
        && fields_are_approved(metadata, SPAN_FIELDS)
}

pub(crate) fn remote_log_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.is_event()
        && metadata.target() == TELEMETRY_TARGET
        && metadata.module_path() == Some(DIAGNOSTIC_OWNER)
        && fields_are_approved(metadata, EVENT_FIELDS)
}

pub(crate) fn health_log_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    metadata.is_event()
        && metadata.target() == HEALTH_TARGET
        && matches!(
            metadata.module_path(),
            Some(DIAGNOSTIC_OWNER | RUNTIME_HEALTH_OWNER)
        )
        && fields_are_approved(metadata, HEALTH_FIELDS)
}

pub(crate) fn local_sink_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    matches!(
        *metadata.level(),
        tracing::Level::ERROR | tracing::Level::WARN | tracing::Level::INFO
    ) && match metadata.target() {
        TELEMETRY_TARGET if metadata.is_span() => remote_span_enabled(metadata),
        TELEMETRY_TARGET => remote_log_enabled(metadata),
        HEALTH_TARGET => health_log_enabled(metadata),
        _ => false,
    }
}

fn fields_are_approved(metadata: &tracing::Metadata<'_>, approved: &[&str]) -> bool {
    metadata
        .fields()
        .iter()
        .all(|field| approved.contains(&field.name()))
}
