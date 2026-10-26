//! 生产入站桥从已认证消息传递源身份；空目录不依赖测试直接注入apply输入。
use super::{deps, fixture, ClipboardInboundRuntime, FakeReceiver, RecordingEvents};
use crate::clipboard::inbound::{
    InboundClipboardApplyError, InboundClipboardApplyInput, InboundClipboardApplyOutcome,
    InboundClipboardApplyPort,
};
use crate::clipboard::sync::payload_codec::encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes;
use crate::transfer::receive::materialization::{InboundFileSetManifest, InboundFileSetMember};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::time::{timeout, Duration};
use uc_core::clipboard::FileSetMemberKind;
use uc_core::ids::EntryId;
use uc_core::ports::{ClipboardHeader, InboundClipboardDisposition};
use uc_core::SystemClipboardSnapshot;

#[derive(Default)]
struct Apply {
    inputs: Mutex<Vec<InboundClipboardApplyInput>>,
}
#[async_trait]
impl InboundClipboardApplyPort for Apply {
    async fn apply(
        &self,
        input: InboundClipboardApplyInput,
    ) -> Result<InboundClipboardApplyOutcome, InboundClipboardApplyError> {
        self.inputs.lock().unwrap().push(input);
        Ok(InboundClipboardApplyOutcome::Applied {
            entry_id: "receiver-entry".into(),
        })
    }
}

#[tokio::test]
async fn runtime_passes_authenticated_source_identity_for_an_empty_directory() {
    let receiver = Arc::new(FakeReceiver::new());
    let apply = Arc::new(Apply::default());
    let runtime = ClipboardInboundRuntime::start(deps(
        receiver.clone(),
        apply.clone(),
        Arc::new(RecordingEvents::default()),
    ));
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: Vec::new(),
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let manifest = InboundFileSetManifest {
        members: vec![InboundFileSetMember {
            root_index: 0,
            root_name: "directory".into(),
            root_is_file: false,
            relative_path: String::new(),
            kind: FileSetMemberKind::EmptyDirectory,
            blob_ref_index: None,
        }],
    };
    let (plaintext, hash) =
        encode_snapshot_with_blob_refs_and_file_set_to_v3_bytes(&snapshot, &[], &manifest).unwrap();
    let source = EntryId::from("42424242-4242-4242-4242-424242424242");
    let (mut inbound, result) = fixture("peer-1", &hash);
    inbound.header.version = ClipboardHeader::DIRECTORY_VERSION;
    inbound.header.source_entry = Some(source.clone());
    inbound.ciphertext = plaintext.clone();
    receiver.publish(inbound);
    assert_eq!(
        timeout(Duration::from_secs(1), result.wait())
            .await
            .unwrap(),
        Some(InboundClipboardDisposition::Applied)
    );
    {
        let inputs = apply.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].source_entry, Some(source));
        assert_eq!(inputs[0].from_device, "peer-1");
        assert_eq!(inputs[0].plaintext, plaintext);
    }
    runtime.shutdown().await.unwrap();
}
