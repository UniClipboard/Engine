//! 启动请求的计时不能由持续运行的发现actor延长。
use super::{tests::identity_store, IrohNodeBuilder, IrohNodeConfig};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use tracing::{
    dispatcher,
    field::{Field, Visit},
    span::{Attributes, Id},
    Instrument, Subscriber,
};
use tracing_subscriber::{
    layer::{Context, SubscriberExt},
    registry::LookupSpan,
    Layer,
};

#[derive(Clone, Default)]
struct ClosedSpans(Arc<Mutex<SpanState>>);
#[derive(Default)]
struct SpanState {
    closed: HashSet<u64>,
    created: HashMap<u64, (String, Option<u64>)>,
}
#[derive(Default)]
struct ActorLabel(String);
impl Visit for ActorLabel {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "actor" {
            self.0 = format!("{value:?}");
        }
    }
}
impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for ClosedSpans {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let parent = ctx
            .span(id)
            .and_then(|span| span.parent())
            .map(|span| span.id().into_u64());
        let mut actor = ActorLabel::default();
        attrs.record(&mut actor);
        self.0.lock().unwrap().created.insert(
            id.into_u64(),
            (
                format!(
                    "{} {} {}",
                    attrs.metadata().target(),
                    attrs.metadata().name(),
                    actor.0
                ),
                parent,
            ),
        );
    }
    fn on_close(&self, id: Id, _: Context<'_, S>) {
        self.0.lock().unwrap().closed.insert(id.into_u64());
    }
}

#[tokio::test]
async fn startup_trace_finishes_before_background_discovery_stops() {
    let closed = ClosedSpans::default();
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(closed.clone()));
    let _scope = dispatcher::set_default(&dispatch);
    let startup = tracing::info_span!(parent: None, "test_session_start");
    let startup_id = startup.id().unwrap().into_u64();
    let node = async {
        let identity = identity_store();
        let builder = IrohNodeBuilder::bind(
            &identity,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .await
        .expect("node bound");
        let session = builder.prepare_session();
        let mut node = builder.spawn();
        node.activate_session(session.finish())
            .await
            .expect("session activated");
        node
    }
    .instrument(startup.clone())
    .await;
    drop(startup);
    let (finished_before_stop, open_before_stop) = {
        let state = closed.0.lock().unwrap();
        (
            state.closed.contains(&startup_id),
            state
                .created
                .iter()
                .filter(|(id, _)| !state.closed.contains(id))
                .map(|(id, info)| (*id, info.clone()))
                .collect::<Vec<_>>(),
        )
    };
    node.shutdown().await.expect("discovery stopped");
    assert!(
        finished_before_stop,
        "长期发现actor不得延长已完成启动请求的计时: {open_before_stop:?}"
    );
}
