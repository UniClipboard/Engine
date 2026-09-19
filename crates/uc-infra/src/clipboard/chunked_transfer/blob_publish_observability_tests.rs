use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tracing::{field::Visit, Event, Subscriber};
use tracing_subscriber::{layer::Context, layer::SubscriberExt, Layer};
use uc_core::ports::TransferCipherPort;
use uc_observability_contract::diagnostics::connectivity::{
    decode_local_record, scope_blob_publish,
};

use super::{ready_session, TransferCipherAdapter, COMPRESSION_THRESHOLD};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Value>>>);

#[derive(Default)]
struct Fields {
    name: String,
    payload: String,
}

impl Visit for Fields {
    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn Debug) {}

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        match field.name() {
            "event.name" => self.name = value.into(),
            "payload" => self.payload = value.into(),
            _ => {}
        }
    }
}

impl<S: Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        if event.metadata().target() != "uc.connectivity" {
            return;
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        let record = decode_local_record(
            &fields.name,
            &fields.payload,
            event.metadata().level().as_str(),
        )
        .expect("blob work record should decode");
        self.0.lock().expect("capture").push(Value::Object(record));
    }
}

#[test]
fn blob_publish_scope_emits_compression_and_encryption_work_only_for_blob_publication() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    tracing::subscriber::with_default(subscriber, || {
        executor.block_on(async {
            let (session, _root) = ready_session();
            let adapter = TransferCipherAdapter::new(session);
            let input = vec![0x5a; COMPRESSION_THRESHOLD + 1];
            adapter.encrypt(&input).await.expect("ordinary encryption");
            scope_blob_publish(adapter.encrypt(&input))
                .await
                .expect("blob encryption");
        });
    });

    let records = capture.0.lock().expect("capture");
    let finished: Vec<_> = records
        .iter()
        .filter(|record| record["event.name"] == "runtime.work.finished")
        .collect();
    assert_eq!(finished.len(), 2, "ordinary encryption must stay quiet");
    assert!(finished
        .iter()
        .any(|record| { record["step"] == "blob_compress" && record["uc.outcome"] == "ok" }));
    assert!(finished
        .iter()
        .any(|record| { record["step"] == "blob_encrypt" && record["uc.outcome"] == "ok" }));
}
