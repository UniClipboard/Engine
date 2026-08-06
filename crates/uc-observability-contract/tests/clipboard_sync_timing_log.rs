use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use uc_observability_contract::analytics::{
    Direction, PayloadSizeBucket, PayloadType, TransportType,
};
use uc_observability_contract::otlp::{
    log_clipboard_sync_stage, ClipboardSyncStage, ClipboardSyncTiming,
};
use uc_observability_contract::FlowId;

#[derive(Clone, Default)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

struct Writer(CapturedWriter);

impl Write for Writer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
             .0
            .lock()
            .expect("captured log writer lock")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for CapturedWriter {
    type Writer = Writer;

    fn make_writer(&'a self) -> Self::Writer {
        Writer(self.clone())
    }
}

impl CapturedWriter {
    fn output(&self) -> String {
        String::from_utf8(self.0.lock().expect("captured log writer lock").clone())
            .expect("UTF-8 log output")
    }
}

#[test]
fn logs_relay_stage_with_otlp_fields_and_no_clipboard_payload() {
    let writer = CapturedWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(writer.clone())
        .finish();
    let dispatch = tracing::Dispatch::new(subscriber);
    let flow_id = FlowId::generate();

    tracing::dispatcher::with_default(&dispatch, || {
        let parent = tracing::info_span!(
            "clipboard_inbound",
            "peer.device_id" = "peer-relay",
            clipboard_content = "clipboard-content-must-never-appear"
        );
        let _entered = parent.enter();
        log_clipboard_sync_stage(ClipboardSyncTiming {
            flow_id: &flow_id,
            flow_synthetic: false,
            direction: Direction::Outbound,
            payload_type: PayloadType::Text,
            payload_size_bucket: PayloadSizeBucket::Lt1Kb,
            transport_type: TransportType::Relay,
            stage: ClipboardSyncStage::ReceiverApplyWait,
            duration_ms: 55,
        });
    });

    let output = writer.output();
    assert!(output.contains("event.name=\"clipboard.sync.stage.completed\""));
    assert!(output.contains(&format!("flow_id={flow_id}")));
    assert!(output.contains("flow_synthetic=false"));
    assert!(output.contains("sync_direction=\"outbound\""));
    assert!(output.contains("sync_transport=\"relay\""));
    assert!(output.contains("sync_stage=\"receiver_apply_wait\""));
    assert!(output.contains("sync_duration_ms=55"));
    assert!(!output.contains("clipboard-content-must-never-appear"));
    assert!(!output.contains("peer-relay"));
}
