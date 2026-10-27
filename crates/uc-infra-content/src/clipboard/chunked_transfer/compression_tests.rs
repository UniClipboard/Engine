use super::{ready_session, ChunkedEncoder, TransferCipherAdapter};
use uc_core::ports::TransferCipherPort;
use uuid::Uuid;

#[tokio::test]
async fn multi_megabyte_compressible_content_remains_compact_and_readable() {
    let (session, _) = ready_session();
    let adapter = TransferCipherAdapter::new(session);
    let plaintext = b"large encrypted clipboard content\n".repeat(100_000);

    let encrypted = adapter.encrypt(&plaintext).await.expect("encrypt large text");

    assert_eq!(&encrypted[..4], b"UC4\0");
    assert!(encrypted.len() < plaintext.len() / 10);
    assert_eq!(adapter.decrypt(&encrypted).await.expect("decrypt large text"), plaintext);
}

#[tokio::test]
async fn incompressible_content_spanning_many_chunks_remains_readable() {
    let (session, _) = ready_session();
    let adapter = TransferCipherAdapter::new(session);
    let mut state = 0x7b21_459d_02fa_c361_u64;
    let plaintext: Vec<u8> = (0..3 * 1024 * 1024).map(|_| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    }).collect();

    let encrypted = adapter.encrypt(&plaintext).await.expect("encrypt binary data");

    assert_eq!(&encrypted[..4], b"UC4\0");
    assert_eq!(adapter.decrypt(&encrypted).await.expect("decrypt binary data"), plaintext);
}

#[tokio::test]
async fn compressed_legacy_v3_content_remains_readable() {
    let (session, root) = ready_session();
    let adapter = TransferCipherAdapter::new(session);
    let plaintext = b"legacy compressed clipboard content\n".repeat(40_000);
    // 旧锁图实际生成的固定压缩字节，防止以当前编码器自证旧格式兼容。
    let compressed = include_bytes!("fixtures/zstd-0.13.3-legacy-v3.zst");
    let mut legacy = Vec::new();
    ChunkedEncoder::encode_to(
        &mut legacy, &root, Uuid::nil().as_bytes(), compressed, 1,
        u32::try_from(plaintext.len()).expect("bounded fixture"),
    ).expect("encode legacy transfer");

    assert_eq!(adapter.decrypt(&legacy).await.expect("decode legacy transfer"), plaintext);
}
