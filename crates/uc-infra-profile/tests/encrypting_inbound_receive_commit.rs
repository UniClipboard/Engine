use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::clipboard::{
    ClipboardEntry, ClipboardEvent, ClipboardSelection, ClipboardSelectionDecision, MimeType,
    PersistedClipboardRepresentation, SelectionPolicyVersion,
};
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::{DeviceId, EntryId, EventId, FormatId, RepresentationId};
use uc_core::ports::security::{BlobCipherError, BlobCipherPort};
use uc_core::ports::{
    CommitInboundReceivePort, CompletedReceiveArtifacts, InboundReceiveCommitError,
    InboundReceiveRecord, InboundReceiveSettlement,
};
use uc_core::SnapshotHash;
use uc_infra_profile::security::EncryptingInboundReceiveCommit;

struct RecordingCipher {
    aads: Mutex<Vec<Vec<u8>>>,
}

#[async_trait]
impl BlobCipherPort for RecordingCipher {
    async fn encrypt(
        &self,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError> {
        self.aads.lock().unwrap().push(aad.as_bytes().to_vec());
        let mut encrypted = b"encrypted:".to_vec();
        encrypted.extend_from_slice(plaintext.as_bytes());
        Ok(Ciphertext::new(encrypted))
    }

    async fn decrypt(
        &self,
        _ciphertext: &Ciphertext,
        _aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError> {
        unreachable!()
    }
}

#[derive(Default)]
struct RecordingCommit {
    inline_data: Mutex<Vec<Vec<u8>>>,
    deliveries: Mutex<Vec<String>>,
}

#[async_trait]
impl CommitInboundReceivePort for RecordingCommit {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        let record = match settlement {
            InboundReceiveSettlement::Progress {
                record,
                delivered_item_ids,
                ..
            } => {
                self.deliveries
                    .lock()
                    .unwrap()
                    .extend(delivered_item_ids.clone());
                record
            }
            InboundReceiveSettlement::Complete {
                record,
                delivered_item_ids,
                ..
            }
            | InboundReceiveSettlement::Partial {
                record,
                delivered_item_ids,
                ..
            } => {
                self.deliveries
                    .lock()
                    .unwrap()
                    .extend(delivered_item_ids.clone());
                record
            }
            InboundReceiveSettlement::NoEntry { .. }
            | InboundReceiveSettlement::FileFailed { .. }
            | InboundReceiveSettlement::FileCleaned { .. } => return Ok(()),
        };
        let representations = match record {
            InboundReceiveRecord::Create {
                representations, ..
            } => representations,
            InboundReceiveRecord::Replace {
                new_representations,
                ..
            } => new_representations,
        };
        self.inline_data.lock().unwrap().extend(
            representations
                .iter()
                .filter_map(|representation| representation.inline_data.clone()),
        );
        Ok(())
    }
}

fn record() -> InboundReceiveRecord {
    let event_id = EventId::from("event-1");
    let representation_id = RepresentationId::from("rep-1");
    let event = ClipboardEvent::new(
        event_id.clone(),
        1,
        DeviceId::new("peer"),
        SnapshotHash::parse(&format!("blake3v1:{}", "00".repeat(32))).unwrap(),
    );
    let representation = PersistedClipboardRepresentation::new(
        representation_id.clone(),
        FormatId::from("text"),
        Some(MimeType("text/plain".to_owned())),
        5,
        Some(b"hello".to_vec()),
        None,
    );
    let selection = ClipboardSelectionDecision::new(
        EntryId::from("entry-1"),
        ClipboardSelection {
            primary_rep_id: representation_id.clone(),
            secondary_rep_ids: Vec::new(),
            preview_rep_id: representation_id.clone(),
            paste_rep_id: representation_id,
            policy_version: SelectionPolicyVersion::V1,
        },
    );
    InboundReceiveRecord::Create {
        entry: ClipboardEntry::new(EntryId::from("entry-1"), event_id, 1, 5),
        event,
        representations: vec![representation],
        selection,
    }
}

#[tokio::test]
async fn atomic_inbound_commit_encrypts_inline_payload_before_storage() {
    let inner = Arc::new(RecordingCommit::default());
    let cipher = Arc::new(RecordingCipher {
        aads: Mutex::new(Vec::new()),
    });
    let commit = EncryptingInboundReceiveCommit::new(inner.clone(), cipher.clone());

    commit
        .commit_inbound_receive(&InboundReceiveSettlement::Complete {
            delivered_item_ids: vec!["saved-item".into()],
            record: record(),
            attempt_id: "attempt-1".to_owned(),
            file_set: None,
            artifacts: CompletedReceiveArtifacts::None,
            now_ms: 2,
        })
        .await
        .unwrap();

    assert_eq!(
        inner.inline_data.lock().unwrap().as_slice(),
        &[b"encrypted:hello".to_vec()]
    );
    assert_eq!(inner.deliveries.lock().unwrap().as_slice(), &["saved-item"]);
    assert_eq!(
        cipher.aads.lock().unwrap().as_slice(),
        &[uc_core::crypto::aad::for_inline(
            &EventId::from("event-1"),
            &RepresentationId::from("rep-1"),
        )]
    );
}

#[tokio::test]
async fn progress_commit_encrypts_inline_payload_and_preserves_delivered_artifact_identity() {
    use uc_core::ports::{ReceiveArtifact, ReceiveArtifactOwnership};
    let inner = Arc::new(RecordingCommit::default());
    let cipher = Arc::new(RecordingCipher {
        aads: Mutex::new(Vec::new()),
    });
    let commit = EncryptingInboundReceiveCommit::new(inner.clone(), cipher.clone());
    commit
        .commit_inbound_receive(&InboundReceiveSettlement::Progress {
            delivered_item_ids: vec!["saved-item".into()],
            record: record(),
            attempt_id: "attempt-1".into(),
            artifact: ReceiveArtifact {
                item_id: "saved-item".into(),
                staged_path: "PRIVATE_PATH".into(),
                final_path: "PRIVATE_PATH".into(),
                ownership: ReceiveArtifactOwnership::ManagedStaging,
            },
            now_ms: 2,
        })
        .await
        .unwrap();
    assert_eq!(
        inner.inline_data.lock().unwrap().as_slice(),
        &[b"encrypted:hello".to_vec()]
    );
    assert_eq!(inner.deliveries.lock().unwrap().as_slice(), &["saved-item"]);
    assert_eq!(
        cipher.aads.lock().unwrap().as_slice(),
        &[uc_core::crypto::aad::for_inline(
            &EventId::from("event-1"),
            &RepresentationId::from("rep-1")
        )]
    );
}
