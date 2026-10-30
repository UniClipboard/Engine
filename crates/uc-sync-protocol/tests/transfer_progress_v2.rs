//! 新版进度合同先由固定字节及拒收边界约束，不接入拨号或业务流程。
use std::error::Error;
use tokio::io::AsyncWriteExt;
use uc_core::error_class::ErrorClass;
use uc_sync_protocol::transfer_progress::v2::{
    self, ActiveProgress, CancelReason, Envelope, FailureReason, FrameRefusal, ItemKind, ItemState,
    LogicalItem, Message, Phase, ProducerDescriptor, QueryResult, RejectionReason, SnapshotPage,
    TaskScope, TerminalOutcome, TerminalResult, WireError, WireId, MAX_FRAME_BYTES, MAX_ITEMS,
    MAX_PAGE_ITEMS, TRANSFER_PROGRESS_ALPN,
};
use uc_sync_protocol::{WireTraceContext, TRACEPARENT_MAX_BYTES};

// 基础向量显式不带观测载体；载体的边界与固定字节由独立用例验证。
fn message_bytes(message: &Message) -> Result<Vec<u8>, WireError> {
    v2::encode_frame(&Envelope {
        message: message.clone(),
        trace_context: None,
    })
}

fn decode_message(bytes: &[u8]) -> Result<Message, WireError> {
    let envelope = v2::decode_frame(bytes)?;
    assert!(envelope.trace_context.is_none());
    Ok(envelope.message)
}

async fn write_message(
    writer: &mut tokio::io::DuplexStream,
    message: &Message,
) -> Result<(), WireError> {
    v2::write_frame(
        writer,
        &Envelope {
            message: message.clone(),
            trace_context: None,
        },
    )
    .await
}

async fn read_message(reader: &mut tokio::io::DuplexStream) -> Result<Message, WireError> {
    let envelope = v2::read_frame(reader).await?;
    assert!(envelope.trace_context.is_none());
    Ok(envelope.message)
}

fn scope() -> TaskScope {
    TaskScope {
        source_entry: WireId([2; 16]),
        attempt: WireId([3; 16]),
    }
}

fn descriptor() -> ProducerDescriptor {
    ProducerDescriptor {
        producer: WireId([1; 16]),
        owner_revision: 7,
        scope: scope(),
        item_count: 2,
    }
}

fn active() -> ActiveProgress {
    ActiveProgress {
        item: LogicalItem {
            kind: ItemKind::File,
            index: 0,
        },
        round: 1,
        sequence: 9,
        bytes: 10,
        total: Some(20),
        phase: Phase::Transferring,
    }
}

fn terminal() -> TerminalResult {
    TerminalResult {
        item: LogicalItem {
            kind: ItemKind::File,
            index: 0,
        },
        round: 1,
        sequence: 10,
        result_revision: 5,
        bytes: 20,
        total: Some(20),
        outcome: TerminalOutcome::Completed,
    }
}

fn page() -> SnapshotPage {
    let mut other = active();
    other.item.index = 1;
    other.item.kind = ItemKind::Representation;
    SnapshotPage {
        snapshot_revision: 12,
        offset: 0,
        total: 2,
        items: vec![ItemState::Terminal(terminal()), ItemState::Active(other)],
    }
}

fn vectors() -> Vec<(&'static str, Message)> {
    let nonce = WireId([4; 16]);
    let binding = WireId([5; 16]);
    let mut values = vec![
        (
            "register",
            Message::Register {
                source_entry: scope().source_entry,
            },
        ),
        ("challenge", Message::Challenge { nonce }),
        (
            "descriptor",
            Message::CurrentDescriptor {
                challenge: nonce,
                descriptor: descriptor(),
            },
        ),
        (
            "bound",
            Message::Bound {
                challenge: nonce,
                binding,
                descriptor: descriptor(),
            },
        ),
        (
            "update",
            Message::Update {
                binding,
                scope: scope(),
                owner_revision: 7,
                progress: active(),
            },
        ),
        (
            "contact",
            Message::Contact {
                binding,
                scope: scope(),
                owner_revision: 7,
                sequence: 11,
            },
        ),
        (
            "terminal",
            Message::Terminal {
                binding,
                scope: scope(),
                owner_revision: 7,
                result: terminal(),
            },
        ),
        (
            "ack",
            Message::TerminalAck {
                binding,
                scope: scope(),
                owner_revision: 7,
                result: terminal(),
            },
        ),
        (
            "query",
            Message::Query {
                challenge: nonce,
                source_entry: scope().source_entry,
                expected_attempt: Some(scope().attempt),
                offset: 0,
                snapshot_revision: None,
            },
        ),
        (
            "current",
            Message::QueryResult {
                challenge: nonce,
                result: QueryResult::Current {
                    descriptor: descriptor(),
                    page: page(),
                },
            },
        ),
        (
            "superseded",
            Message::QueryResult {
                challenge: nonce,
                result: QueryResult::Superseded {
                    descriptor: descriptor(),
                    page: page(),
                },
            },
        ),
        (
            "not_retained",
            Message::QueryResult {
                challenge: nonce,
                result: QueryResult::NotRetained,
            },
        ),
        (
            "unsupported",
            Message::QueryResult {
                challenge: nonce,
                result: QueryResult::Unsupported,
            },
        ),
        (
            "denied",
            Message::QueryResult {
                challenge: nonce,
                result: QueryResult::AccessDenied,
            },
        ),
        (
            "rejected",
            Message::Rejected {
                challenge: nonce,
                reason: RejectionReason::Stale,
            },
        ),
    ];
    for (label, phase, total, kind) in [
        (
            "directory_waiting",
            Phase::Waiting,
            Some(20),
            ItemKind::DirectoryUnit,
        ),
        (
            "processing_unknown_total",
            Phase::Processing,
            None,
            ItemKind::File,
        ),
        (
            "retrying_zero_total",
            Phase::Retrying,
            Some(0),
            ItemKind::File,
        ),
    ] {
        let mut progress = active();
        progress.phase = phase;
        progress.total = total;
        progress.item.kind = kind;
        if total == Some(0) || phase == Phase::Waiting {
            progress.bytes = 0;
        }
        values.push((
            label,
            Message::Update {
                binding,
                scope: scope(),
                owner_revision: 7,
                progress,
            },
        ));
    }
    for (label, reason) in [
        ("failure_network", FailureReason::NetworkUnavailable),
        ("failure_timeout", FailureReason::TimedOut),
        ("failure_denied", FailureReason::AccessDenied),
        ("failure_storage", FailureReason::StorageUnavailable),
        ("failure_integrity", FailureReason::IntegrityCheckFailed),
        ("failure_interrupted", FailureReason::Interrupted),
        ("failure_unknown", FailureReason::Unknown),
    ] {
        let mut result = terminal();
        result.outcome = TerminalOutcome::Failed(reason);
        values.push((
            label,
            Message::Terminal {
                binding,
                scope: scope(),
                owner_revision: 7,
                result,
            },
        ));
    }
    for (label, reason) in [
        ("cancel_local", CancelReason::LocalUser),
        ("cancel_remote", CancelReason::RemotePeer),
        ("cancel_replaced", CancelReason::Replaced),
        ("cancel_timeout", CancelReason::Timeout),
        ("cancel_recovery", CancelReason::ConnectivityRecovery),
        ("cancel_unknown", CancelReason::Unknown),
    ] {
        let mut result = terminal();
        result.outcome = TerminalOutcome::Cancelled(reason);
        values.push((
            label,
            Message::Terminal {
                binding,
                scope: scope(),
                owner_revision: 7,
                result,
            },
        ));
    }
    for (label, reason) in [
        ("reject_denied", RejectionReason::AccessDenied),
        ("reject_busy", RejectionReason::Busy),
        ("reject_unknown", RejectionReason::UnknownTask),
        ("reject_scope", RejectionReason::InvalidScope),
    ] {
        values.push((
            label,
            Message::Rejected {
                challenge: nonce,
                reason,
            },
        ));
    }
    values.push((
        "query_continuation",
        Message::Query {
            challenge: nonce,
            source_entry: scope().source_entry,
            expected_attempt: None,
            offset: 1,
            snapshot_revision: Some(12),
        },
    ));
    values
}

fn unhex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn fixed_v2_vectors_cover_every_message_and_query_result() {
    let fixture = include_str!("fixtures/transfer-progress-v2-vectors.txt");
    let expected = fixture
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect::<Vec<_>>();
    let messages = vectors();
    assert_eq!(messages.len(), expected.len());
    assert_eq!(TRANSFER_PROGRESS_ALPN, b"uniclipboard/transfer-progress/2");
    for ((name, message), line) in messages.into_iter().zip(expected) {
        let (label, value) = line.split_once(' ').unwrap();
        assert_eq!(name, label);
        let bytes = unhex(value);
        assert_eq!(message_bytes(&message).unwrap(), bytes, "{name}");
        assert_eq!(decode_message(&bytes).unwrap(), message, "{name}");
    }
}

#[test]
fn every_truncated_prefix_and_extra_byte_is_rejected() {
    for (name, message) in vectors() {
        let encoded = message_bytes(&message).unwrap();
        for length in 0..encoded.len() {
            assert!(
                decode_message(&encoded[..length]).is_err(),
                "{name} prefix {length}"
            );
        }
        let mut extra = encoded;
        extra.push(0);
        assert!(matches!(
            decode_message(&extra),
            Err(WireError::Frame(FrameRefusal::TrailingBytes))
        ));
    }
}

#[test]
fn bad_magic_version_tag_and_length_fail_closed() {
    let encoded = message_bytes(&Message::Challenge {
        nonce: WireId([4; 16]),
    })
    .unwrap();
    let mut bad_magic = encoded.clone();
    bad_magic[0] = 0;
    assert!(matches!(
        decode_message(&bad_magic),
        Err(WireError::Frame(FrameRefusal::Magic))
    ));
    let mut version = encoded.clone();
    version[3] = 3;
    assert!(matches!(
        decode_message(&version),
        Err(WireError::Frame(FrameRefusal::Version))
    ));
    let mut tag = encoded;
    tag[9] = 255;
    let error = decode_message(&tag).unwrap_err();
    assert!(matches!(&error, WireError::Value(_)));
    assert!(error.source().is_some());
    for size in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let mut header = b"UCP\x02".to_vec();
        header.extend_from_slice(&size.to_be_bytes());
        assert!(matches!(
            decode_message(&header),
            Err(WireError::Frame(FrameRefusal::Length))
        ));
    }
}

#[test]
fn terminal_reasons_and_unknown_total_round_trip_without_conflating_zero() {
    let failures = [
        FailureReason::NetworkUnavailable,
        FailureReason::TimedOut,
        FailureReason::AccessDenied,
        FailureReason::StorageUnavailable,
        FailureReason::IntegrityCheckFailed,
        FailureReason::Interrupted,
        FailureReason::Unknown,
    ];
    let cancellations = [
        CancelReason::LocalUser,
        CancelReason::RemotePeer,
        CancelReason::Replaced,
        CancelReason::Timeout,
        CancelReason::ConnectivityRecovery,
        CancelReason::Unknown,
    ];
    let outcomes = failures
        .into_iter()
        .map(TerminalOutcome::Failed)
        .chain(cancellations.into_iter().map(TerminalOutcome::Cancelled));
    for outcome in outcomes {
        for total in [None, Some(0), Some(u64::MAX)] {
            let mut result = terminal();
            result.outcome = outcome;
            result.bytes = 0;
            result.total = total;
            let message = Message::Terminal {
                binding: WireId([5; 16]),
                scope: scope(),
                owner_revision: 7,
                result,
            };
            assert_eq!(
                decode_message(&message_bytes(&message).unwrap()).unwrap(),
                message
            );
        }
    }
}

#[test]
fn maximum_page_is_valid_and_over_limit_or_mismatched_pages_are_rejected() {
    let mut full = page();
    full.total = MAX_ITEMS as u16;
    full.offset = (MAX_ITEMS - MAX_PAGE_ITEMS) as u16;
    full.items = (0..MAX_PAGE_ITEMS)
        .map(|index| {
            let mut value = active();
            value.item.index = (index + MAX_ITEMS - MAX_PAGE_ITEMS) as u16;
            ItemState::Active(value)
        })
        .collect();
    let mut owner = descriptor();
    owner.item_count = MAX_ITEMS as u16;
    let make = |owner: ProducerDescriptor, page| Message::QueryResult {
        challenge: WireId([4; 16]),
        result: QueryResult::Current {
            descriptor: owner,
            page,
        },
    };
    let valid = make(owner.clone(), full.clone());
    assert_eq!(
        decode_message(&message_bytes(&valid).unwrap()).unwrap(),
        valid
    );
    let mut excessive = full.clone();
    excessive.items.push(ItemState::Active(active()));
    assert!(message_bytes(&make(owner.clone(), excessive)).is_err());
    let mut mismatch = full.clone();
    mismatch.total -= 1;
    assert!(message_bytes(&make(owner.clone(), mismatch)).is_err());
    let mut empty = full.clone();
    empty.items.clear();
    assert!(message_bytes(&make(owner.clone(), empty)).is_err());
    let mut duplicate = full;
    duplicate.items[1] = duplicate.items[0].clone();
    assert!(message_bytes(&make(owner, duplicate)).is_err());
}

#[test]
fn malformed_page_count_is_rejected_before_reading_records() {
    let message = Message::QueryResult {
        challenge: WireId([4; 16]),
        result: QueryResult::Current {
            descriptor: descriptor(),
            page: page(),
        },
    };
    let mut encoded = message_bytes(&message).unwrap();
    // header8＋空观测载体1＋kind1＋challenge16＋reply1＋descriptor58＋revision8＋offset2＋total2。
    let count = 8 + 1 + 1 + 16 + 1 + 58 + 8 + 2 + 2;
    encoded[count..count + 2].copy_from_slice(&u16::MAX.to_be_bytes());
    assert!(matches!(
        decode_message(&encoded),
        Err(WireError::Frame(FrameRefusal::Page))
    ));
}

#[tokio::test]
async fn framed_stream_supports_handshake_messages_and_preserves_io_source() {
    let (mut write, mut read) = tokio::io::duplex(1024);
    let first = Message::Register {
        source_entry: scope().source_entry,
    };
    let second = Message::Challenge {
        nonce: WireId([4; 16]),
    };
    write_message(&mut write, &first).await.unwrap();
    write_message(&mut write, &second).await.unwrap();
    assert_eq!(read_message(&mut read).await.unwrap(), first);
    assert_eq!(read_message(&mut read).await.unwrap(), second);
    write.write_all(b"UCP").await.unwrap();
    drop(write);
    let error = read_message(&mut read).await.unwrap_err();
    assert!(matches!(&error, WireError::Io(_)));
    assert!(error
        .source()
        .unwrap()
        .downcast_ref::<std::io::Error>()
        .is_some());
}

#[tokio::test]
async fn oversized_header_is_rejected_without_waiting_for_body() {
    let (mut write, mut read) = tokio::io::duplex(8);
    write.write_all(b"UCP\x02").await.unwrap();
    write.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
    // writer 仍然打开且没有正文；若先读取／分配正文，该 future 不会立即返回。
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        read_message(&mut read),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(WireError::Frame(FrameRefusal::Length))
    ));
}

#[test]
fn identities_are_redacted_in_debug_output() {
    let debug = format!("{:?}", WireId([0xab; 16]));
    assert_eq!(debug, "WireId(<opaque>)");
}

#[test]
fn invalid_item_and_continuation_shapes_are_rejected() {
    for (kind, index) in [
        (ItemKind::File, MAX_ITEMS as u16),
        (ItemKind::Representation, u16::MAX),
        (ItemKind::DirectoryUnit, 1),
    ] {
        let mut progress = active();
        progress.item = LogicalItem { kind, index };
        let message = Message::Update {
            binding: WireId([5; 16]),
            scope: scope(),
            owner_revision: 7,
            progress,
        };
        assert!(matches!(
            message_bytes(&message),
            Err(WireError::Frame(FrameRefusal::Item))
        ));
    }
    let query = Message::Query {
        challenge: WireId([4; 16]),
        source_entry: scope().source_entry,
        expected_attempt: None,
        offset: 1,
        snapshot_revision: None,
    };
    assert!(matches!(
        message_bytes(&query),
        Err(WireError::Frame(FrameRefusal::Page))
    ));
    for count in [0, MAX_ITEMS as u16 + 1] {
        let mut owner = descriptor();
        owner.item_count = count;
        let message = Message::CurrentDescriptor {
            challenge: WireId([4; 16]),
            descriptor: owner,
        };
        assert!(matches!(
            message_bytes(&message),
            Err(WireError::Frame(FrameRefusal::Page))
        ));
    }
}

#[test]
fn duplicate_decoded_items_and_non_boolean_options_are_rejected() {
    let message = Message::QueryResult {
        challenge: WireId([4; 16]),
        result: QueryResult::Current {
            descriptor: descriptor(),
            page: page(),
        },
    };
    let mut bytes = message_bytes(&message).unwrap();
    let second_item = 8 + 1 + 1 + 16 + 1 + 58 + 14 + 1 + 41 + 1;
    bytes[second_item..second_item + 3].copy_from_slice(&[1, 0, 0]);
    assert!(matches!(
        decode_message(&bytes),
        Err(WireError::Frame(FrameRefusal::Page))
    ));
    let message = Message::Update {
        binding: WireId([5; 16]),
        scope: scope(),
        owner_revision: 7,
        progress: active(),
    };
    let mut bytes = message_bytes(&message).unwrap();
    let total_presence = 8 + 1 + 1 + 16 + 32 + 8 + 3 + 4 + 8 + 8;
    bytes[total_presence] = 2;
    assert!(matches!(
        decode_message(&bytes),
        Err(WireError::Frame(FrameRefusal::Option))
    ));
}

#[test]
fn exact_frame_limit_is_accepted_before_payload_validation() {
    let mut frame = message_bytes(&Message::Challenge {
        nonce: WireId([4; 16]),
    })
    .unwrap();
    frame.resize(MAX_FRAME_BYTES, 0);
    frame[4..8].copy_from_slice(&((MAX_FRAME_BYTES - 8) as u32).to_be_bytes());
    assert!(matches!(
        decode_message(&frame),
        Err(WireError::Frame(FrameRefusal::TrailingBytes))
    ));
    frame.push(0);
    frame[4..8].copy_from_slice(&((MAX_FRAME_BYTES - 7) as u32).to_be_bytes());
    assert!(matches!(
        decode_message(&frame),
        Err(WireError::Frame(FrameRefusal::Length))
    ));
}

#[test]
fn trace_envelope_has_fixed_bytes_and_bounded_carrier() {
    let envelope = Envelope {
        message: Message::Challenge {
            nonce: WireId([4; 16]),
        },
        trace_context: Some(WireTraceContext {
            traceparent: "00-11111111111111111111111111111111-2222222222222222-01".into(),
        }),
    };
    let expected = unhex(include_str!("fixtures/transfer-progress-v2-trace.hex").trim());
    assert_eq!(v2::encode_frame(&envelope).unwrap(), expected);
    assert_eq!(v2::decode_frame(&expected).unwrap(), envelope);
    assert!(!format!("{envelope:?}").contains("11111111111111111111111111111111"));
    for length in [0, TRACEPARENT_MAX_BYTES, TRACEPARENT_MAX_BYTES + 1] {
        let envelope = Envelope {
            message: Message::Challenge {
                nonce: WireId([4; 16]),
            },
            trace_context: Some(WireTraceContext {
                traceparent: "a".repeat(length),
            }),
        };
        let result = v2::encode_frame(&envelope);
        if length == TRACEPARENT_MAX_BYTES {
            assert_eq!(v2::decode_frame(&result.unwrap()).unwrap(), envelope);
        } else {
            assert!(matches!(
                result,
                Err(WireError::Frame(FrameRefusal::TraceContext))
            ));
        }
    }
}

#[test]
fn malformed_trace_length_and_utf8_preserve_fixed_classification() {
    for length in [0u16, TRACEPARENT_MAX_BYTES as u16 + 1] {
        let mut frame = b"UCP\x02".to_vec();
        frame.extend_from_slice(&4u32.to_be_bytes());
        frame.push(1);
        frame.extend_from_slice(&length.to_be_bytes());
        frame.push(2);
        assert!(matches!(
            v2::decode_frame(&frame),
            Err(WireError::Frame(FrameRefusal::TraceContext))
        ));
    }
    let mut frame = b"UCP\x02".to_vec();
    frame.extend_from_slice(&4u32.to_be_bytes());
    frame.extend_from_slice(&[1, 0, 1, 255]);
    let error = v2::decode_frame(&frame).unwrap_err();
    assert!(matches!(&error, WireError::Utf8(_)));
    assert!(error
        .source()
        .unwrap()
        .downcast_ref::<std::string::FromUtf8Error>()
        .is_some());
    assert_eq!(
        format!("{error:?}"),
        "transfer progress trace carrier is not utf8"
    );
}

#[test]
fn wire_failures_keep_variant_level_classes_and_source_chains() {
    for (reason, class) in [
        (FrameRefusal::Magic, "transfer_progress_magic"),
        (FrameRefusal::Version, "transfer_progress_version"),
        (FrameRefusal::Length, "transfer_progress_length"),
        (FrameRefusal::Truncated, "transfer_progress_truncated"),
        (
            FrameRefusal::TrailingBytes,
            "transfer_progress_trailing_bytes",
        ),
        (FrameRefusal::Page, "transfer_progress_page"),
        (FrameRefusal::Item, "transfer_progress_item"),
        (FrameRefusal::Option, "transfer_progress_option"),
        (
            FrameRefusal::TraceContext,
            "transfer_progress_trace_context",
        ),
    ] {
        let error = WireError::from(reason);
        assert_eq!(error.class(), class);
        assert!(error.source().unwrap().is::<FrameRefusal>());
    }
    let error = WireError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    assert_eq!(error.class(), "transfer_progress_io");
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied
    );
    let mut unknown = message_bytes(&Message::Challenge {
        nonce: WireId([4; 16]),
    })
    .unwrap();
    unknown[9] = 255;
    assert_eq!(
        decode_message(&unknown).unwrap_err().class(),
        "transfer_progress_unknown_value"
    );
}
