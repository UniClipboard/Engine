use tokio::io::AsyncWriteExt;
use uc_core::membership::{MembershipHistoryAckV3, MembershipHistoryMessage};
use uc_sync_protocol::membership_history::{
    decode_message, decode_request, encode_message, encode_request, read_frame,
    MembershipHistoryStatus,
};

#[test]
fn published_v4_invalid_ack_bytes_remain_unchanged() {
    // 原 edf0a296 编码器：AckV3 判别值 3，Invalid 判别值 5；请求上下文为空。
    let message = MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid);
    let reply = [4, 3, 5];
    let request = [4, b'U', b'C', b'T', b'1', 0, 3, 5];
    assert_eq!(encode_message(&message).unwrap(), reply);
    assert_eq!(encode_request(message.clone(), None).unwrap(), request);
    assert_eq!(decode_message(&reply).unwrap(), message);
    assert_eq!(decode_request(&request).unwrap().message, message);
    assert!(decode_request(&reply).is_err());
    assert_eq!(
        MembershipHistoryStatus::decode(3).unwrap(),
        MembershipHistoryStatus::Busy
    );
    assert!(MembershipHistoryStatus::decode(255).is_err());
}

#[tokio::test]
async fn oversized_frame_is_rejected_without_reading_its_body() {
    let (mut send, mut receive) = tokio::io::duplex(4);
    send.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
    assert!(read_frame(&mut receive).await.is_err());
}
