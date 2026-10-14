//! 迁移前固定输入：保留空值、截断和尾随输入的实际解释。
use tokio::io::AsyncReadExt;
use uc_sync_protocol::active_clipboard_pull::{
    read_request, read_response, PullResponse, PullWireError,
};
use uc_sync_protocol::active_clipboard_state::{decode_body, read_frame, ActiveWireDecodeError};

#[tokio::test]
async fn pull_empty_and_trailing_inputs_keep_original_semantics() {
    let mut request = &b"\xc2\x00\x00\xff"[..];
    assert_eq!(read_request(&mut request).await.unwrap(), "");
    assert_eq!(request.read_u8().await.unwrap(), 255);
    let mut response = &b"\x00\x00\x00\x00\x00\xff"[..];
    assert_eq!(
        read_response(&mut response).await.unwrap(),
        PullResponse::Envelope(vec![])
    );
    assert_eq!(response.read_u8().await.unwrap(), 255);
    for (byte, expected) in [
        (1, PullResponse::NotAvailable),
        (2, PullResponse::Locked),
        (3, PullResponse::Internal),
    ] {
        let bytes = [byte, 255];
        let mut response = &bytes[..];
        assert_eq!(read_response(&mut response).await.unwrap(), expected);
        assert_eq!(response.read_u8().await.unwrap(), 255);
    }
}

#[tokio::test]
async fn pull_invalid_inputs_keep_original_classification() {
    assert!(matches!(
        read_request(&mut &b"\xc1"[..]).await,
        Err(PullWireError::BadMagic { .. })
    ));
    assert!(matches!(
        read_request(&mut &b"\xc2\x04\x01"[..]).await,
        Err(PullWireError::SnapshotHashTooLong { .. })
    ));
    assert!(matches!(
        read_request(&mut &b"\xc2\x00"[..]).await,
        Err(PullWireError::Io(_))
    ));
    assert!(matches!(
        read_response(&mut &b"\xff"[..]).await,
        Err(PullWireError::UnknownStatus { .. })
    ));
    assert!(matches!(
        read_response(&mut &b"\x00\x01\x00\x00\x01"[..]).await,
        Err(PullWireError::EnvelopeTooLong { .. })
    ));
    assert!(matches!(
        read_response(&mut &b"\x00\x00\x00\x00\x02\x01"[..]).await,
        Err(PullWireError::Io(_))
    ));
}

#[tokio::test]
async fn state_empty_truncated_and_trailing_inputs_keep_original_semantics() {
    assert!(matches!(
        decode_body(&[]),
        Err(ActiveWireDecodeError::Postcard(_))
    ));
    // V1 的四个空/零字段均合法；postcard 原实现不拒绝尾随 body 字节。
    let state = decode_body(&[1, 0, 0, 0, 0, 255]).unwrap();
    assert_eq!(state.snapshot_hash, "");
    assert_eq!(state.entry_id, "");
    assert_eq!(state.activated_at_ms, 0);
    assert_eq!(state.activated_by, "");
    assert!(matches!(
        read_frame(&mut &b"\xc2"[..]).await,
        Err(ActiveWireDecodeError::BadMagic { .. })
    ));
    assert!(matches!(
        read_frame(&mut &b"\xc3\x00\x00\x10\x01"[..]).await,
        Err(ActiveWireDecodeError::BodyTooLarge { .. })
    ));
    assert!(matches!(
        read_frame(&mut &b"\xc3\x00\x00\x00\x05\x01"[..]).await,
        Err(ActiveWireDecodeError::Io(_))
    ));
}
