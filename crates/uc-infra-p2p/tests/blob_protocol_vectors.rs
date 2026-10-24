//! 由 rc.15 真实冻结 SDK 导出的合成协议向量，不使用当前实现计算期望值。
#[path = "support/blob_protocol_vectors.rs"]
mod vectors;

#[tokio::test]
async fn blob_wire_matches_frozen_rc15_raw_and_empty_vectors() -> anyhow::Result<()> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/blob-connectivity-rc15-vectors.json");
    let expected: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let current = vectors::generate().await?;
    assert!(
        current == expected,
        "blob wire changed from the frozen historical SDK"
    );
    Ok(())
}
