//! 独立网络日志层：按已审定调用点把第三方网络事件编码为脱敏摘要，写入自己的队列。

use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;
use uc_observability_contract::diagnostics::NETWORK_SCOPE_TARGET;

use super::rules::{callsite_path, rule_for, rule_index, CaptureTier, Kind, Library, Rule, RULES};
use super::status::NetworkCounters;
use crate::file_statistics::increment;
use crate::local_file::{lock, AsyncFileWriter};

/// 读取字段原始文本的长度上限；超出部分截断，规则只做分类与计数。
const RAW_VALUE_LIMIT: usize = 4_096;
/// 引用表容量。表满时淘汰最旧的映射，之后同一对象会得到新的引用。
const REFERENCE_CAPACITY: usize = 1_024;
/// 未审查调用点记录的令牌桶。
const UNREVIEWED_BURST: u32 = 4;
const UNREVIEWED_REFILL_MS: u32 = 10_000;
/// 详细采集窗口的合法范围。
pub(crate) const MAX_DETAILED_WINDOW: Duration = Duration::from_secs(900);

/// 静态的调用点准入：已审定的调用点，加上未审查的 WARN/ERROR（只计数并留下无字段的静态编号）。
///
/// 这是 Layer 级别的静态过滤，结果由 tracing 按调用点缓存，所以热路径上未审定的 TRACE/DEBUG
/// 调用点不会产生任何运行期开销。
pub(crate) fn callsite_enabled(metadata: &tracing::Metadata<'_>) -> bool {
    if metadata.is_span() {
        return metadata.target() == NETWORK_SCOPE_TARGET;
    }
    if Library::of(metadata).is_none() {
        return false;
    }
    rule_for(metadata).is_some() || *metadata.level() <= Level::WARN
}

pub(crate) struct NetworkLogLayer {
    shared: Arc<Shared>,
}

pub(crate) struct Shared {
    writer: AsyncFileWriter,
    counters: Arc<NetworkCounters>,
    run_id: String,
    known_alpns: Vec<String>,
    record_bytes: usize,
    started: Instant,
    sequence: AtomicU64,
    /// 序号分配与入队在同一临界区内完成，文件里的序号因此严格递增，缺口只可能来自丢弃。
    emit: Mutex<()>,
    /// 详细采集窗口的结束时刻，单位为 `started` 之后的毫秒加一；零表示未开启。
    detailed_until: AtomicU64,
    state: Mutex<State>,
}

struct State {
    buckets: Vec<Bucket>,
    unreviewed: Bucket,
    peers: References,
    connections: References,
    capture: Option<Capture>,
}

pub(crate) struct Capture {
    pub(crate) id: String,
    pub(crate) started_at_utc: String,
    pub(crate) duration: Duration,
}

struct Bucket {
    tokens: u32,
    last_refill: Instant,
}

impl Bucket {
    fn full(burst: u32, now: Instant) -> Self {
        Self {
            tokens: burst,
            last_refill: now,
        }
    }

    fn take(&mut self, burst: u32, refill_ms: u32, now: Instant) -> bool {
        let refill = u64::from(refill_ms.max(1));
        let elapsed = u64::try_from(now.saturating_duration_since(self.last_refill).as_millis())
            .unwrap_or(u64::MAX);
        let gained = u32::try_from(elapsed / refill).unwrap_or(u32::MAX);
        if gained > 0 {
            self.tokens = self.tokens.saturating_add(gained).min(burst);
            self.last_refill += Duration::from_millis(u64::from(gained) * refill);
            if self.tokens == burst {
                self.last_refill = now;
            }
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

/// 把原始标识映射为本次运行内的随机引用。映射只在内存里，不导出，也不跨运行。
struct References {
    next: u32,
    by_value: HashMap<String, u32>,
    order: VecDeque<String>,
}

impl References {
    fn new() -> Self {
        Self {
            next: 1,
            by_value: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn reference(&mut self, value: &str) -> u32 {
        if let Some(reference) = self.by_value.get(value) {
            return *reference;
        }
        if self.order.len() >= REFERENCE_CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.by_value.remove(&oldest);
            }
        }
        let reference = self.next;
        self.next = self.next.wrapping_add(1).max(1);
        self.by_value.insert(value.to_owned(), reference);
        self.order.push_back(value.to_owned());
        reference
    }
}

impl NetworkLogLayer {
    pub(crate) fn new(
        writer: AsyncFileWriter,
        counters: Arc<NetworkCounters>,
        run_id: String,
        known_alpns: Vec<String>,
        record_bytes: usize,
    ) -> (Self, Arc<Shared>) {
        let now = Instant::now();
        let shared = Arc::new(Shared {
            writer,
            counters,
            run_id,
            known_alpns,
            record_bytes,
            started: now,
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
            detailed_until: AtomicU64::new(0),
            state: Mutex::new(State {
                buckets: RULES
                    .iter()
                    .map(|rule| Bucket::full(rule.burst, now))
                    .collect(),
                unreviewed: Bucket::full(UNREVIEWED_BURST, now),
                peers: References::new(),
                connections: References::new(),
                capture: None,
            }),
        });
        (
            Self {
                shared: Arc::clone(&shared),
            },
            shared,
        )
    }
}

impl Shared {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn detailed_active(&self) -> bool {
        let until = self.detailed_until.load(Ordering::Acquire);
        until != 0 && self.now_ms().saturating_add(1) < until
    }

    /// 开启限时详细采集。窗口只在本进程内存里，重启不恢复；重复开启不延长。
    pub(crate) fn start_capture(&self, duration: Duration) -> (Capture, bool) {
        let mut state = lock(&self.state);
        if self.detailed_active() {
            if let Some(capture) = &state.capture {
                return (
                    Capture {
                        id: capture.id.clone(),
                        started_at_utc: capture.started_at_utc.clone(),
                        duration: capture.duration,
                    },
                    true,
                );
            }
        }
        let capture = Capture {
            id: uuid::Uuid::new_v4().simple().to_string(),
            started_at_utc: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            duration,
        };
        let until = self
            .now_ms()
            .saturating_add(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .saturating_add(1);
        self.detailed_until.store(until, Ordering::Release);
        let result = Capture {
            id: capture.id.clone(),
            started_at_utc: capture.started_at_utc.clone(),
            duration,
        };
        state.capture = Some(capture);
        (result, false)
    }

    pub(crate) fn stop_capture(&self, capture_id: &str) -> bool {
        let mut state = lock(&self.state);
        match &state.capture {
            Some(capture) if capture.id == capture_id => {
                self.detailed_until.store(0, Ordering::Release);
                state.capture = None;
                true
            }
            _ => false,
        }
    }

    /// 详细窗口的剩余毫秒；已到期或未开启为 `None`。
    pub(crate) fn detailed_remaining_ms(&self) -> Option<u64> {
        let until = self.detailed_until.load(Ordering::Acquire);
        if until == 0 {
            return None;
        }
        let remaining = until.saturating_sub(1).saturating_sub(self.now_ms());
        (remaining > 0).then_some(remaining)
    }

    fn emit_reviewed(&self, rule: &'static Rule, event: &tracing::Event<'_>) {
        if rule.tier == CaptureTier::Detailed && !self.detailed_active() {
            increment(&self.counters.policy_filtered);
            return;
        }
        let allowed = lock(&self.state).buckets[rule_index(rule)].take(
            rule.burst,
            rule.refill_ms,
            Instant::now(),
        );
        if !allowed {
            increment(&self.counters.rate_limited);
            return;
        }
        // 第三方类型的 Debug 实现在持锁外运行，避免它们自己产生事件时重入本层。
        let mut collector = Collector::new(rule);
        event.record(&mut collector);
        let mut fields = Map::new();
        {
            let mut state = lock(&self.state);
            for (field_rule, raw) in rule.fields.iter().zip(collector.values) {
                let Some(raw) = raw else { continue };
                if let Some(value) = self.render(field_rule.kind, &raw, &mut state) {
                    fields.insert(field_rule.output.to_owned(), value);
                }
            }
        }
        self.counters.by_library[rule.library as usize].fetch_add(1, Ordering::Relaxed);
        self.emit(
            *event.metadata().level(),
            rule.library,
            Some(rule.version_dir),
            rule.event,
            match rule.tier {
                CaptureTier::Standard => "standard",
                CaptureTier::Detailed => "detailed",
            },
            Value::Object(fields),
        );
    }

    /// 未审查的 WARN/ERROR 调用点：只留库、版本目录和库内静态位置，不读取任何字段。
    fn emit_unreviewed(&self, metadata: &tracing::Metadata<'_>, library: Library) {
        increment(&self.counters.unreviewed_callsites);
        increment(&self.counters.policy_filtered);
        let allowed = lock(&self.state).unreviewed.take(
            UNREVIEWED_BURST,
            UNREVIEWED_REFILL_MS,
            Instant::now(),
        );
        if !allowed {
            increment(&self.counters.rate_limited);
            return;
        }
        let path = callsite_path(metadata.file());
        let callsite = match (path.in_crate, metadata.line()) {
            (Some(file), Some(line)) => Some(format!("{file}:{line}")),
            _ => None,
        };
        self.emit(
            *metadata.level(),
            library,
            path.version_dir,
            "unreviewed_callsite",
            "standard",
            json!({ "callsite": callsite }),
        );
    }

    fn emit(
        &self,
        level: Level,
        library: Library,
        version_dir: Option<&str>,
        event: &str,
        tier: &str,
        fields: Value,
    ) {
        let _ordered = lock(&self.emit);
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let mut line = serde_json::to_vec(&json!({
            "v": super::status::NETWORK_LOG_SCHEMA_VERSION,
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "mono_ms": self.now_ms(),
            "seq": sequence,
            "run": self.run_id,
            "level": level.as_str(),
            "lib": library.as_str(),
            "lib_dir": version_dir,
            "event": event,
            "tier": tier,
            "f": fields,
        }))
        .unwrap_or_default();
        line.push(b'\n');
        if line.len() > self.record_bytes {
            increment(&self.counters.oversized);
            return;
        }
        self.writer.write_indexed(&line, 0);
    }

    fn render(&self, kind: Kind, raw: &str, state: &mut State) -> Option<Value> {
        match kind {
            Kind::ConnRef => Some(Value::from(state.connections.reference(raw))),
            Kind::PeerRef => Some(Value::from(state.peers.reference(raw))),
            Kind::Alpn => Some(Value::from(
                self.known_alpns
                    .iter()
                    .find(|known| known.as_str() == raw)
                    .map_or("unrecognized", String::as_str),
            )),
            Kind::Variant(variants) => Some(Value::from(
                variants
                    .iter()
                    .find(|variant| starts_with_variant(raw, variant))
                    .copied()
                    .unwrap_or("other"),
            )),
            Kind::Contains(table) => {
                let lowered = raw.to_ascii_lowercase();
                Some(Value::from(
                    table
                        .iter()
                        .find(|(needle, _)| lowered.contains(needle))
                        .map_or("other", |(_, label)| *label),
                ))
            }
            Kind::Transport => Some(Value::from(path_kind(raw))),
            Kind::CountSeparated(separator) => Some(Value::from(count_separated(raw, separator))),
            Kind::CountToken(token) => Some(Value::from(raw.matches(token).count())),
            Kind::Bool => match raw {
                "true" => Some(Value::from(true)),
                "false" => Some(Value::from(false)),
                _ => None,
            },
            Kind::Number(max) => raw
                .parse::<u64>()
                .ok()
                .filter(|value| *value <= max)
                .map(Value::from),
        }
    }
}

/// 值是 Debug 形式的变体名，后面不能紧跟标识符字符（`Reset` 不匹配 `ResetByPeer`）。
fn starts_with_variant(raw: &str, variant: &str) -> bool {
    raw.strip_prefix(variant)
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'))
}

fn path_kind(raw: &str) -> &'static str {
    let raw = raw.trim();
    if raw == "None" {
        return "none";
    }
    if let Some(rest) = raw.strip_prefix("Ip(") {
        let remote = rest.rsplit("->").next().unwrap_or(rest);
        return if remote.starts_with('[') && !remote.contains("::ffff:") {
            "ip_v6"
        } else {
            "ip_v4"
        };
    }
    if raw.starts_with("Relay(") {
        "relay"
    } else if raw.starts_with("Custom") {
        "custom"
    } else {
        "other"
    }
}

fn count_separated(raw: &str, separator: &str) -> usize {
    let inner = raw
        .trim()
        .trim_start_matches(['[', '{'])
        .trim_end_matches([']', '}'])
        .trim();
    if inner.is_empty() {
        0
    } else {
        inner.matches(separator).count() + 1
    }
}

impl<S: Subscriber> Layer<S> for NetworkLogLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let Some(library) = Library::of(metadata) else {
            return;
        };
        match rule_for(metadata) {
            Some(rule) => self.shared.emit_reviewed(rule, event),
            None => self.shared.emit_unreviewed(metadata, library),
        }
    }
}

/// 只读取规则声明过的字段；其余字段的值从不被格式化。
struct Collector {
    wanted: &'static [super::rules::FieldRule],
    values: Vec<Option<String>>,
}

impl Collector {
    fn new(rule: &'static Rule) -> Self {
        Self {
            wanted: rule.fields,
            values: vec![None; rule.fields.len()],
        }
    }

    fn slot(&mut self, field: &Field) -> Option<&mut Option<String>> {
        let index = self
            .wanted
            .iter()
            .position(|wanted| wanted.source == field.name())?;
        self.values.get_mut(index)
    }
}

impl Visit for Collector {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if let Some(slot) = self.slot(field) {
            let mut text = Capped::default();
            let _ = write!(text, "{value:?}");
            *slot = Some(text.0);
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if let Some(slot) = self.slot(field) {
            *slot = Some(value.chars().take(RAW_VALUE_LIMIT).collect());
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if let Some(slot) = self.slot(field) {
            *slot = Some(value.to_string());
        }
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if let Some(slot) = self.slot(field) {
            *slot = Some(value.to_string());
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        if let Some(slot) = self.slot(field) {
            *slot = Some(value.to_string());
        }
    }
}

#[derive(Default)]
struct Capped(String);

impl fmt::Write for Capped {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let room = RAW_VALUE_LIMIT.saturating_sub(self.0.len());
        if text.len() <= room {
            self.0.push_str(text);
            Ok(())
        } else {
            let mut end = room;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&text[..end]);
            Err(fmt::Error)
        }
    }
}
