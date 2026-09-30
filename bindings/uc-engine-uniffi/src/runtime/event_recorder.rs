//! 测试用的最小 tracing 订阅者：把每个事件的字段记成 `名称=取值` 文本，可跨线程共享。

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};

/// 每个事件一行：`消息 名称=取值 名称=取值 ...`。
#[derive(Clone, Default)]
pub(super) struct EventRecorder(Arc<Mutex<Vec<String>>>);

struct Line(String);

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push_str(&format!(" {}={value}", field.name()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.push_str(&format!(" {}={value:?}", field.name()));
    }
}

impl EventRecorder {
    pub(super) fn lines(&self) -> Vec<String> {
        self.0.lock().map(|lines| lines.clone()).unwrap_or_default()
    }
}

impl Subscriber for EventRecorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line(String::new());
        event.record(&mut line);
        if let Ok(mut lines) = self.0.lock() {
            lines.push(line.0);
        }
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}
