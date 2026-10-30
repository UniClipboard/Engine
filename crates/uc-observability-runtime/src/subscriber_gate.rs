//! 在订阅器入口持有已接受的追踪创建回调。
use std::any::TypeId;
use std::sync::Arc;

use tracing_core::span::{Attributes, Current, Id, Record};
use tracing_core::subscriber::{Interest, Subscriber};
use tracing_core::{Event, LevelFilter, Metadata};

use crate::filter::remote_span_enabled;
use crate::remote_health::TracingSpans;

pub(crate) struct SubscriberGate<S> {
    inner: S,
    activity: Option<Arc<TracingSpans>>,
}

impl<S> SubscriberGate<S> {
    pub(crate) fn new(inner: S, activity: Option<Arc<TracingSpans>>) -> Self {
        Self { inner, activity }
    }
}

impl<S: Subscriber> Subscriber for SubscriberGate<S> {
    fn register_callsite(&self, metadata: &'static Metadata<'static>) -> Interest {
        self.inner.register_callsite(metadata)
    }

    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.inner.enabled(metadata)
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        self.inner.max_level_hint()
    }

    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        let _callback = self
            .activity
            .as_ref()
            .filter(|_| remote_span_enabled(attrs.metadata()))
            .map(TracingSpans::callback);
        self.inner.new_span(attrs)
    }

    fn record(&self, id: &Id, values: &Record<'_>) {
        self.inner.record(id, values);
    }
    fn record_follows_from(&self, id: &Id, follows: &Id) {
        self.inner.record_follows_from(id, follows);
    }
    fn event_enabled(&self, event: &Event<'_>) -> bool {
        self.inner.event_enabled(event)
    }
    fn event(&self, event: &Event<'_>) {
        self.inner.event(event);
    }
    fn enter(&self, id: &Id) {
        self.inner.enter(id);
    }
    fn exit(&self, id: &Id) {
        self.inner.exit(id);
    }
    fn clone_span(&self, id: &Id) -> Id {
        self.inner.clone_span(id)
    }
    fn try_close(&self, id: Id) -> bool {
        self.inner.try_close(id)
    }
    fn current_span(&self) -> Current {
        self.inner.current_span()
    }

    // SAFETY: 返回的指针由内层订阅器提供，借用期不超过 self。
    unsafe fn downcast_raw(&self, id: TypeId) -> Option<*const ()> {
        if id == TypeId::of::<Self>() {
            Some(self as *const _ as *const ())
        } else {
            // SAFETY: 透明转交调用方的类型与生命周期约束。
            unsafe { self.inner.downcast_raw(id) }
        }
    }
}
