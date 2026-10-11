use std::error::Error;

use uc_sync_protocol::group_update::{
    decode_request, encode_request, read_request, write_request, GroupUpdateAck, GroupUpdateVersion,
};
use uc_sync_protocol::WireTraceContext;

// 在原 Iroh adapter 中先固定的线上字节，不从新 encoder 反推基准。
const PLAIN: &[u8] = b"UCT1\x00\x03MLS";
const TRACED: &[u8] = b"UCT1\x01\x3700-0123456789abcdef0123456789abcdef-0123456789abcdef-01\x03MLS";

#[test]
fn requests_preserve_the_original_plain_and_traced_bytes() {
    assert_eq!(encode_request(b"MLS", None).unwrap(), PLAIN);
    let trace = WireTraceContext {
        traceparent: "00-0123456789abcdef0123456789abcdef-0123456789abcdef-01".into(),
    };
    assert_eq!(encode_request(b"MLS", Some(trace.clone())).unwrap(), TRACED);
    assert_eq!(decode_request(PLAIN).unwrap().payload, b"MLS");
    assert_eq!(decode_request(TRACED).unwrap().trace_context, Some(trace));
}

#[test]
fn acknowledgements_keep_version_specific_meaning() {
    for version in [GroupUpdateVersion::Legacy, GroupUpdateVersion::Current] {
        assert_eq!(GroupUpdateAck::Applied.encode(version), Some(1));
        assert_eq!(GroupUpdateAck::Rejected.encode(version), Some(2));
    }
    assert_eq!(
        GroupUpdateAck::Unavailable.encode(GroupUpdateVersion::Current),
        Some(3)
    );
    assert_eq!(
        GroupUpdateAck::Unavailable.encode(GroupUpdateVersion::Legacy),
        None
    );
    assert!(GroupUpdateAck::decode(3, GroupUpdateVersion::Legacy).is_err());
    assert!(GroupUpdateAck::decode(0, GroupUpdateVersion::Current).is_err());
}

#[test]
fn malformed_and_unbounded_requests_are_rejected() {
    assert!(decode_request(b"MLS").is_err());
    assert!(decode_request(b"UCT1\x00\x00").is_err());
    for end in 0..PLAIN.len() {
        assert!(decode_request(&PLAIN[..end]).is_err());
    }
    assert!(encode_request(b"", None).is_err());
    assert!(encode_request(&vec![0; 4 * 1024 * 1024 + 1], None).is_err());
}

#[test]
fn trailing_bytes_are_not_another_valid_request() {
    let mut extended = PLAIN.to_vec();
    extended.push(0xff);
    assert!(decode_request(&extended).is_err());
}

#[tokio::test]
async fn stream_framing_matches_the_original_length_prefix() {
    let mut encoded = Vec::new();
    write_request(&mut encoded, b"MLS", None).await.unwrap();
    assert_eq!(encoded, b"\x00\x00\x00\x09UCT1\x00\x03MLS");
    let decoded = read_request(&mut encoded.as_slice()).await.unwrap();
    assert_eq!(decoded.payload, b"MLS");
    // 声明超限而不交付正文，验证分配/读取之前就返回长度拒绝。
    let oversize = (4u32 * 1024 * 1024 + 1025).to_be_bytes();
    assert!(read_request(&mut oversize.as_slice()).await.is_err());
}

#[test]
fn maximum_payload_roundtrips_and_decoder_errors_keep_their_source() {
    let payload = vec![0x41; 4 * 1024 * 1024];
    let encoded = encode_request(&payload, None).unwrap();
    assert_eq!(decode_request(&encoded).unwrap().payload, payload);
    let truncated = decode_request(b"UCT1\x00\x03M").err().unwrap();
    assert!(truncated.source().unwrap().is::<postcard::Error>());
}
