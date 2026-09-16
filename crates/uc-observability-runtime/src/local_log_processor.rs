//! SDK 记录到既有本地文件队列的适配；不拥有线程、刷新或关闭流程。

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use opentelemetry::logs::{AnyValue, Severity};
use opentelemetry::InstrumentationScope;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogProcessor, SdkLogRecord};
use serde_json::{json, Map, Value};

use crate::local_capture::source_for;
use crate::local_file::LocalFileRuntime;
use crate::local_recording::LocalRecordingState;
use crate::remote_health::log_is_approved;

pub(crate) struct LocalLogProcessor {
    file: Arc<LocalFileRuntime>,
    recording: Arc<LocalRecordingState>,
}

impl std::fmt::Debug for LocalLogProcessor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalLogProcessor")
            .finish_non_exhaustive()
    }
}

impl LocalLogProcessor {
    pub(crate) fn new(file: Arc<LocalFileRuntime>, recording: Arc<LocalRecordingState>) -> Self {
        Self { file, recording }
    }
}

impl LogProcessor for LocalLogProcessor {
    fn emit(&self, data: &mut SdkLogRecord, scope: &InstrumentationScope) {
        if data
            .target()
            .is_none_or(|target| !matches!(target.as_ref(), "uc.telemetry" | "uc.connectivity"))
        {
            return;
        }
        let Some(mut record) = decode_record(data, scope, &self.recording) else {
            self.recording.rejected();
            self.file.record_rejection();
            return;
        };
        if !self.recording.include(&mut record) {
            return;
        }
        let source = source_for(&record);
        let Ok(mut record) = serde_json::to_vec(&record) else {
            self.recording.rejected();
            self.file.record_rejection();
            return;
        };
        if record.len() > 4096 {
            self.recording.rejected();
            self.file.record_rejection();
            return;
        }
        record.push(b'\n');
        self.file.writer().write_record(&record, source);
    }

    // 文件队列由进程运行时在 SDK 刷新之后统一刷新，处理器自身没有缓存。
    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    // health 仍可能在 provider 收尾期间写入；不能在此关闭共同 writer。
    fn shutdown_with_timeout(&self, _timeout: Duration) -> OTelSdkResult {
        Ok(())
    }

    fn event_enabled(&self, _level: Severity, target: &str, _name: Option<&str>) -> bool {
        matches!(target, "uc.telemetry" | "uc.connectivity")
    }
}

fn decode_record(
    data: &SdkLogRecord,
    scope: &InstrumentationScope,
    recording: &LocalRecordingState,
) -> Option<Value> {
    if data.body().is_some()
        || !scope.name().is_empty()
        || scope.version().is_some()
        || scope.schema_url().is_some()
        || scope.attributes().next().is_some()
    {
        return None;
    }
    let level = match data.severity_text()? {
        "INFO" => "INFO",
        "WARN" => "WARN",
        "ERROR" => "ERROR",
        _ => return None,
    };
    let target = data.target()?.as_ref();
    if target == "uc.telemetry" && !log_is_approved(data) {
        return None;
    }
    let mut fields = Map::new();
    for (key, value) in data.attributes_iter() {
        let value = match value {
            AnyValue::String(value) => Value::String(value.to_string()),
            AnyValue::Int(value) => json!(value),
            _ => return None,
        };
        if fields.insert(key.to_string(), value).is_some() {
            return None;
        }
    }
    if target == "uc.connectivity" {
        if fields.len() != 2 {
            return None;
        }
        fields = uc_observability_contract::diagnostics::connectivity::decode_local_record(
            fields.get("event.name")?.as_str()?,
            fields.get("payload")?.as_str()?,
            level,
        )?;
    }
    if let Some(detail) =
        uc_observability_contract::diagnostics::connectivity::take_local_completion_detail(
            fields
                .get("uc.domain")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("uc.operation")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("uc.role")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("uc.outcome")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
    {
        let (phase, reason) = detail.local_fields();
        fields.insert("error.phase".into(), json!(phase));
        fields.insert("error.reason".into(), json!(reason));
        if let Some(chain) = detail.source_chain() {
            fields.insert("error.chain".into(), json!(chain));
        }
    }
    let timestamp: DateTime<Utc> = data.timestamp().or(data.observed_timestamp())?.into();
    let mut record = json!({
        "timestamp": timestamp.to_rfc3339_opts(SecondsFormat::Micros, true),
        "level": level, "target": target, "fields": fields,
        "local_schema_version": 1,
    });
    if let Some(context) = data.trace_context() {
        if context.trace_id != opentelemetry::trace::TraceId::INVALID
            && context.span_id != opentelemetry::trace::SpanId::INVALID
        {
            record["trace_id"] = json!(context.trace_id.to_string());
            record["span_id"] = json!(context.span_id.to_string());
        }
    }
    recording.annotate(&mut record);
    Some(record)
}
