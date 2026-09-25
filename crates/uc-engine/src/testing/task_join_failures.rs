//! 捕获任务异常退出的健康记录，供测试断言固定的任务分类。

use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::subscriber::DefaultGuard;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

const TASK_JOIN_FAILED_EVENT: &str = "uc.task.join_failed";

/// 记录当前线程产生的 `uc.task.join_failed` 事件中的 `task.kind`。
#[derive(Clone, Default)]
pub(crate) struct TaskJoinFailures(Arc<Mutex<Vec<String>>>);

impl TaskJoinFailures {
    /// 安装为当前线程的订阅者；`#[tokio::test]` 默认单线程运行时下，派生任务也在本线程执行。
    pub(crate) fn install(&self) -> DefaultGuard {
        tracing::subscriber::set_default(tracing_subscriber::registry().with(self.clone()))
    }

    pub(crate) fn kinds(&self) -> Vec<String> {
        self.0.lock().map(|kinds| kinds.clone()).unwrap_or_default()
    }
}

impl<S: Subscriber> Layer<S> for TaskJoinFailures {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut fields = TaskFields::default();
        event.record(&mut fields);
        if fields.name.as_deref() != Some(TASK_JOIN_FAILED_EVENT) {
            return;
        }
        if let (Some(kind), Ok(mut kinds)) = (fields.kind, self.0.lock()) {
            kinds.push(kind);
        }
    }
}

#[derive(Default)]
struct TaskFields {
    name: Option<String>,
    kind: Option<String>,
}

impl Visit for TaskFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "event.name" => self.name = Some(value.to_owned()),
            "task.kind" => self.kind = Some(value.to_owned()),
            _ => {}
        }
    }

    fn record_debug(&mut self, _field: &Field, _value: &dyn fmt::Debug) {}
}
