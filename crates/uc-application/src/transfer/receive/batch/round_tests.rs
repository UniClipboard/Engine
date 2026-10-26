//! 本轮位置允许真实回落；目录的任一子项重试都推进同一逻辑单元。
use super::super::ReceiveBatch;
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::transfer::receive::materialization::{InboundFileSetManifest, InboundFileSetMember};
use crate::transfer::receive::receive_item_ids;
use crate::transfer::receive::work::ReceiveSource;
use std::collections::HashSet;
use uc_core::clipboard::FileSetMemberKind;
use uc_core::file_transfer::{ProgressItem, ProgressItemKind, ProgressItemState, ProgressPhase};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::BlobTicket;
use uc_core::ports::AttemptState;

#[test]
fn current_round_bytes_and_terminal_keep_original_item_and_directory_rounds() {
    for directory in [false, true] {
        let entry = EntryId::from("local-entry");
        let source = ReceiveSource {
            source_entry: None,
            from_device: DeviceId::new("source"),
            expected_hash: "fixture".into(),
            blob_refs: (0..2)
                .map(|index| V3BlobRef {
                    ticket: BlobTicket::from_bytes(vec![index]),
                    entry_id: EntryId::from("source-entry"),
                    filename: None,
                    mime: None,
                    size_bytes: 100,
                    representation_index: None,
                })
                .collect(),
            manifest: directory.then(|| InboundFileSetManifest {
                members: (0..2)
                    .map(|index| InboundFileSetMember {
                        root_index: 0,
                        root_name: "folder".into(),
                        root_is_file: false,
                        relative_path: format!("child-{index}"),
                        kind: FileSetMemberKind::File,
                        blob_ref_index: Some(index),
                    })
                    .collect(),
            }),
            original_wire: None,
            representations: Vec::new(),
        };
        let planned = source.planned(&entry, "attempt");
        let ids = receive_item_ids(&entry, "attempt", &source.blob_refs);
        let logical = if directory {
            vec![ProgressItem {
                kind: ProgressItemKind::DirectoryUnit,
                index: 0,
            }]
        } else {
            (0..2)
                .map(|index| ProgressItem {
                    kind: ProgressItemKind::File,
                    index,
                })
                .collect()
        };
        let batch = ReceiveBatch::default();
        batch.plan(2);
        batch.record_round(&ids[0], 0, 80, None);
        batch.record_round(&ids[1], 0, 70, None);
        batch.record_retrying(&ids[0], 0);
        let recovering = batch
            .facts()
            .progress_states(&planned, &logical, &HashSet::new(), false)
            .unwrap();
        assert!(
            matches!(recovering[0], ProgressItemState::Active(value) if value.phase == ProgressPhase::Retrying)
        );
        batch.record_round(&ids[0], 1, 0, None);
        batch.record_round(&ids[0], 1, 20, None);
        batch.record_round(&ids[1], 1, 0, None);
        batch.record_round(&ids[1], 1, 30, None);
        // 迟到旧轮次与同轮更低位置都不能回退当前事实。
        batch.record_round(&ids[0], 0, 100, None);
        batch.record_round(&ids[1], 1, 10, None);
        let facts = batch.facts();
        assert_eq!(facts.items[&ids[0]].bytes, 80);
        assert_eq!(facts.items[&ids[1]].bytes, 70);
        let states = facts
            .progress_states(&planned, &logical, &HashSet::new(), false)
            .unwrap();
        for (index, state) in states.iter().enumerate() {
            let ProgressItemState::Active(state) = state else {
                panic!("unfinished local work")
            };
            assert_eq!(state.round, if directory { 2 } else { 1 });
            assert_eq!(state.bytes, if directory { 50 } else { [20, 30][index] });
            assert_eq!(state.total, Some(if directory { 200 } else { 100 }));
        }
        for id in &ids {
            batch.item_materialized(id, 100);
            batch.item_saved(id, 100);
        }
        batch.settled(AttemptState::Completed);
        batch.finish(true);
        let states = batch
            .facts()
            .progress_states(&planned, &logical, &HashSet::new(), true)
            .unwrap();
        for state in states {
            let ProgressItemState::Terminal(state) = state else {
                panic!("persisted result")
            };
            assert_eq!(state.round, if directory { 2 } else { 1 });
            assert_eq!(state.bytes, if directory { 200 } else { 100 });
        }
    }
}

#[test]
fn compressed_representation_uses_verified_wire_size_and_keeps_it_through_save() {
    let source = ReceiveSource {
        source_entry: None,
        from_device: DeviceId::new("source"),
        expected_hash: "fixture".into(),
        blob_refs: vec![V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![1]),
            entry_id: EntryId::from("source-entry"),
            filename: None,
            mime: None,
            size_bytes: 1024,
            representation_index: Some(0),
        }],
        manifest: None,
        original_wire: None,
        representations: Vec::new(),
    };
    let planned = source.planned(&EntryId::from("local"), "attempt");
    let item = ProgressItem {
        kind: ProgressItemKind::Representation,
        index: 0,
    };
    let batch = ReceiveBatch::default();
    batch.plan(1);
    let initial = batch
        .facts()
        .progress_states(&planned, &[item], &HashSet::new(), false)
        .unwrap();
    let ProgressItemState::Active(initial) = initial[0] else {
        panic!("waiting")
    };
    assert_eq!(
        initial.total, None,
        "plaintext size cannot freeze a compressed wire total"
    );
    batch.record_round(&planned[0].id, 0, 32, Some(32));
    batch.record_round(&planned[0].id, 0, 33, Some(64));
    batch.record_round(&planned[0].id, 0, 40, Some(32));
    let network = batch
        .facts()
        .progress_states(&planned, &[item], &HashSet::new(), false)
        .unwrap();
    let ProgressItemState::Active(network) = network[0] else {
        panic!("local save pending")
    };
    assert_eq!((network.bytes, network.total), (32, Some(32)));
    assert_eq!(network.phase, ProgressPhase::Processing);
    batch.item_materialized(&planned[0].id, 1024);
    batch.item_saved(&planned[0].id, 1024);
    batch.settled(AttemptState::Completed);
    batch.finish(true);
    let saved = batch
        .facts()
        .progress_states(&planned, &[item], &HashSet::new(), true)
        .unwrap();
    let ProgressItemState::Terminal(saved) = saved[0] else {
        panic!("actual save settled")
    };
    assert_eq!(
        (saved.bytes, saved.total),
        (32, Some(32)),
        "decryption must not replace wire positions with plaintext length"
    );
}
