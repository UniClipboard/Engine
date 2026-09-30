//! Engine 各 crate 的模块日志：只写本地文件与诊断导出，不进入远程遥测与宿主日志层。
//!
//! 只接收 `uc_*` 目标的事件；合同目标仍走既有的合同/SDK 通道，不在此重复。标准模式记录 INFO 及以上，
//! Detailed 采集窗口内提升到 DEBUG。限额、限速与截断都计数并随导出报告给出，不承诺无限无损。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use opentelemetry::trace::TraceContextExt;
use serde::Serialize;
use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::{Event, Id, Level, Metadata, Subscriber};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::connectivity::{
    LocalDiagnosticSource, CONNECTIVITY_TARGET,
};
use uc_observability_contract::diagnostics::{
    HEALTH_TARGET, LOCAL_DIAGNOSTIC_TARGET, TELEMETRY_TARGET,
};
use uc_observability_contract::module_log::render_error_chain;

use crate::local_file::LocalFileRuntime;
use crate::local_recording::LocalRecordingState;
use crate::module_log_fields::text_field_allowed;

/// 模块日志记录的来源名。
const SOURCE_NAME: &str = "engine_module";
/// 单条记录的字节上限，与合同记录一致。
const MAX_RECORD_BYTES: usize = 4096;
/// 单个进程运行期内模块日志可写入的字节预算，避免挤占合同记录的总限额。
pub(crate) const DEFAULT_RUN_BUDGET_BYTES: u64 = 32 * 1024 * 1024;
/// 每个记录点允许的突发条数与每秒补充条数。
const RATE_BURST: f64 = 20.0;
const RATE_PER_SECOND: f64 = 2.0;
/// 记录点限速表的容量；超出后不再新增记录点表项，也不限速。
const MAX_TRACKED_CALLSITES: usize = 4096;
const MAX_FIELD_CHARS: usize = 256;
const MAX_FIELDS: usize = 24;
/// 未审定的文本字段的占位。
const OMITTED_FIELD: &str = "<omitted>";

/// 模块日志的可见计数，随诊断导出报告给出。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ModuleLogCounts {
    /// 已提交给本地文件队列的记录数。
    pub emitted: u64,
    /// 被记录点限速丢弃的记录数。
    pub rate_limited: u64,
    /// 超出本次运行字节预算而丢弃的记录数。
    pub budget_dropped: u64,
    /// 为满足单条上限而裁剪内容的记录数。
    pub truncated: u64,
    /// 裁剪后仍超限或无法序列化而丢弃的记录数。
    pub rejected: u64,
    /// 错误链中未识别、按占位输出的层数。
    pub opaque_error_layers: u64,
}

impl ModuleLogCounts {
    pub fn dropped_total(&self) -> u64 {
        self.rate_limited
            .saturating_add(self.budget_dropped)
            .saturating_add(self.rejected)
    }
}

#[derive(Default)]
pub(crate) struct ModuleLogStats {
    emitted: AtomicU64,
    rate_limited: AtomicU64,
    budget_dropped: AtomicU64,
    truncated: AtomicU64,
    rejected: AtomicU64,
    opaque_error_layers: AtomicU64,
    written_bytes: AtomicU64,
}

impl ModuleLogStats {
    pub(crate) fn snapshot(&self) -> ModuleLogCounts {
        ModuleLogCounts {
            emitted: self.emitted.load(Ordering::Relaxed),
            rate_limited: self.rate_limited.load(Ordering::Relaxed),
            budget_dropped: self.budget_dropped.load(Ordering::Relaxed),
            truncated: self.truncated.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            opaque_error_layers: self.opaque_error_layers.load(Ordering::Relaxed),
        }
    }
}

fn bump(counter: &AtomicU64, by: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(by))
    });
}

/// 目标属于 Engine 自有 crate。
pub(crate) fn is_engine_source(name: &str) -> bool {
    [
        "uc_core",
        "uc_application",
        "uc_infra",
        "uc_engine",
        "uc_observability_contract",
        "uc_observability_runtime",
        "uc_mobile",
        "uc_mobile_lan",
        "uc_mobile_proto",
    ]
    .iter()
    .any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    })
}

fn is_contract_target(target: &str) -> bool {
    matches!(
        target,
        TELEMETRY_TARGET | CONNECTIVITY_TARGET | LOCAL_DIAGNOSTIC_TARGET | HEALTH_TARGET
    )
}

/// 模块日志层是否关心这个记录点（span 与事件），不含等级判断。
pub(crate) fn module_metadata(metadata: &Metadata<'_>) -> bool {
    !is_contract_target(metadata.target())
        && (is_engine_source(metadata.target())
            || metadata.module_path().is_some_and(is_engine_source))
}

struct Bucket {
    tokens: f64,
    updated: Instant,
    suppressed: u64,
}

/// 每个记录点的令牌桶；被限速的条数在下一条放行记录里以 `suppressed` 字段给出。
#[derive(Default)]
struct RateLimiter {
    buckets: Mutex<HashMap<tracing::callsite::Identifier, Bucket>>,
}

enum Admission {
    Admit { suppressed: u64 },
    Limited,
}

impl RateLimiter {
    fn admit(&self, metadata: &'static Metadata<'static>, now: Instant) -> Admission {
        let mut buckets = self
            .buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = metadata.callsite();
        if !buckets.contains_key(&key) && buckets.len() >= MAX_TRACKED_CALLSITES {
            return Admission::Admit { suppressed: 0 };
        }
        let bucket = buckets.entry(key).or_insert(Bucket {
            tokens: RATE_BURST,
            updated: now,
            suppressed: 0,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * RATE_PER_SECOND).min(RATE_BURST);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Admission::Admit {
                suppressed: std::mem::take(&mut bucket.suppressed),
            }
        } else {
            bucket.suppressed = bucket.suppressed.saturating_add(1);
            Admission::Limited
        }
    }
}

/// 模块 span 创建时记下最近的 OpenTelemetry 祖先上下文，供事件关联使用。
struct SpanTrace {
    trace_id: String,
    span_id: String,
}

pub(crate) struct ModuleLogLayer {
    file: Arc<LocalFileRuntime>,
    recording: Arc<LocalRecordingState>,
    stats: Arc<ModuleLogStats>,
    limiter: RateLimiter,
    budget_bytes: u64,
}

impl ModuleLogLayer {
    pub(crate) fn new(
        file: Arc<LocalFileRuntime>,
        recording: Arc<LocalRecordingState>,
        stats: Arc<ModuleLogStats>,
        budget_bytes: u64,
    ) -> Self {
        Self {
            file,
            recording,
            stats,
            limiter: RateLimiter::default(),
            budget_bytes,
        }
    }

    /// 等级门：INFO 及以上始终记录；DEBUG 只在 Detailed 采集窗口内记录；TRACE 不记录。
    pub(crate) fn level_enabled(recording: &LocalRecordingState, level: &Level) -> bool {
        match *level {
            Level::ERROR | Level::WARN | Level::INFO => true,
            Level::DEBUG => recording.detailed_active(),
            Level::TRACE => false,
        }
    }
}

fn current_trace() -> Option<SpanTrace> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    span_context.is_valid().then(|| SpanTrace {
        trace_id: span_context.trace_id().to_string(),
        span_id: span_context.span_id().to_string(),
    })
}

impl<S> Layer<S> for ModuleLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, _attributes: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let inherited = span.parent().and_then(|parent| trace_of(&parent));
        if let Some(trace) = inherited.or_else(current_trace) {
            span.extensions_mut().insert(trace);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let now = Instant::now();
        let suppressed = match self.limiter.admit(metadata, now) {
            Admission::Admit { suppressed } => suppressed,
            Admission::Limited => {
                bump(&self.stats.rate_limited, 1);
                return;
            }
        };
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);

        let scope: Vec<_> = ctx
            .event_scope(event)
            .map(|scope| scope.from_root().collect())
            .unwrap_or_default();
        let spans: Vec<&str> = scope.iter().map(|span| span.name()).collect();
        let inherited = scope.iter().rev().find_map(trace_of);
        let trace = current_trace().or(inherited);

        let mut record = Map::new();
        record.insert(
            "timestamp".into(),
            json!(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)),
        );
        record.insert("level".into(), json!(metadata.level().as_str()));
        record.insert("target".into(), json!(metadata.target()));
        record.insert("source".into(), json!(SOURCE_NAME));
        if let Some(trace) = trace {
            record.insert("trace_id".into(), json!(trace.trace_id));
            record.insert("span_id".into(), json!(trace.span_id));
        }
        if let Some(location) = location(metadata) {
            record.insert("location".into(), json!(location));
        }
        record.insert("spans".into(), json!(spans));
        record.insert("message".into(), json!(visitor.message.unwrap_or_default()));
        if suppressed > 0 {
            record.insert("suppressed".into(), json!(suppressed));
        }
        let mut record = Value::Object(record);
        let mut opaque = 0;
        if let Some(chain) = visitor.chain {
            opaque = chain.opaque_layers;
            record["error.root"] = json!(chain.root());
            if chain.opaque_layers > 0 {
                record["error.opaque_layers"] = json!(chain.opaque_layers);
            }
            record["error.chain"] = json!(chain.layers);
        }
        record["fields"] = Value::Object(visitor.fields);
        self.recording.module_metadata(&mut record);
        self.submit(record, opaque);
    }
}

fn trace_of<S>(span: &tracing_subscriber::registry::SpanRef<'_, S>) -> Option<SpanTrace>
where
    S: for<'a> LookupSpan<'a>,
{
    let extensions = span.extensions();
    let trace = extensions.get::<SpanTrace>()?;
    Some(SpanTrace {
        trace_id: trace.trace_id.clone(),
        span_id: trace.span_id.clone(),
    })
}

impl ModuleLogLayer {
    fn submit(&self, mut record: Value, opaque_layers: usize) {
        let mut bytes = match serde_json::to_vec(&record) {
            Ok(bytes) => bytes,
            Err(_) => {
                bump(&self.stats.rejected, 1);
                return;
            }
        };
        if bytes.len() >= MAX_RECORD_BYTES {
            bump(&self.stats.truncated, 1);
            record["truncated"] = json!(true);
            record["fields"] = json!({});
            bytes = serde_json::to_vec(&record).unwrap_or_default();
            if bytes.len() >= MAX_RECORD_BYTES {
                if let Some(chain) = record["error.chain"].as_array_mut() {
                    for layer in chain {
                        if let Some(text) = layer.as_str() {
                            *layer = json!(text.chars().take(64).collect::<String>());
                        }
                    }
                }
                record["spans"] = json!([]);
                bytes = serde_json::to_vec(&record).unwrap_or_default();
            }
            if bytes.is_empty() || bytes.len() >= MAX_RECORD_BYTES {
                bump(&self.stats.rejected, 1);
                return;
            }
        }
        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self
            .stats
            .written_bytes
            .fetch_add(length, Ordering::Relaxed)
            .saturating_add(length)
            > self.budget_bytes
        {
            bump(&self.stats.budget_dropped, 1);
            return;
        }
        bytes.push(b'\n');
        self.file
            .writer()
            .write_record(&bytes, LocalDiagnosticSource::Runtime);
        bump(&self.stats.emitted, 1);
        bump(
            &self.stats.opaque_error_layers,
            u64::try_from(opaque_layers).unwrap_or(u64::MAX),
        );
    }
}

/// 相对工作区的源码位置；绝对路径只保留 crate 内的相对部分，避免带出构建机目录。
fn location(metadata: &Metadata<'_>) -> Option<String> {
    let file = metadata.file()?.replace('\\', "/");
    let absolute =
        file.starts_with('/') || file.as_bytes().get(1) == Some(&b':') || file.starts_with("//");
    let relative = match file.rfind("crates/") {
        Some(start) => &file[start + "crates/".len()..],
        None if absolute => return None,
        None => &file,
    };
    Some(match metadata.line() {
        Some(line) => format!("{relative}:{line}"),
        None => relative.to_owned(),
    })
}

#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: Map<String, Value>,
    chain: Option<uc_observability_contract::module_log::ErrorChain>,
}

fn put(fields: &mut Map<String, Value>, field: &Field, value: Value) {
    if fields.len() < MAX_FIELDS {
        fields.insert(field.name().to_owned(), value);
    }
}

/// 文本字段默认拒绝：只有审定清单内的字段名才写出取值。
fn text_value(field: &Field, text: &str) -> Value {
    if text_field_allowed(field.name()) {
        json!(bounded(text.to_owned()))
    } else {
        json!(OMITTED_FIELD)
    }
}

fn bounded(text: String) -> String {
    match text.char_indices().nth(MAX_FIELD_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text,
    }
}

impl Visit for FieldVisitor {
    fn record_i64(&mut self, field: &Field, value: i64) {
        put(&mut self.fields, field, json!(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        put(&mut self.fields, field, json!(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        put(&mut self.fields, field, json!(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        put(&mut self.fields, field, json!(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(bounded(value.to_owned()));
        } else {
            put(&mut self.fields, field, text_value(field, value));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        let chain = render_error_chain(value);
        if self.chain.is_none() {
            self.chain = Some(chain);
        } else {
            put(&mut self.fields, field, json!(chain.layers));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let text = bounded(format!("{value:?}"));
        if field.name() == "message" {
            self.message = Some(text);
        } else {
            put(&mut self.fields, field, text_value(field, &text));
        }
    }
}
