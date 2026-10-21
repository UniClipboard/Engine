//! 长度核对直接使用 SDK 同源的 BAO 遍历器，不编码线上数据。
use bao_tree::{BaoTree, BlockSize, ChunkNum, ChunkRanges};
use iroh_blobs::store::IROH_BLOCK_SIZE;
use std::mem::size_of;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{BlobExchangeError, BlobExchangeFailure};

pub(super) async fn expected_size(
    size: u64,
    ranges: &ChunkRanges,
    cancel: &CancellationToken,
) -> Result<u64, BlobExchangeError> {
    // 从最小 BLAKE3 树按 SDK 块层数合并完整范围，稀疏范围保留所需证明节点。
    let tree = BaoTree::new(size, BlockSize::from_chunk_log(0));
    // SDK 把触及／越过末块的范围视为开放尾部；避免末尾证明节点被重复计入。
    let last = ChunkNum(ChunkNum::chunks(size).0.saturating_sub(1));
    let tail = ChunkRanges::from(last..);
    let mut normalized = ranges & &ChunkRanges::from(..last);
    if !(ranges & &tail).is_empty() {
        normalized |= tail;
    }
    let mut bytes = size_of::<u64>() as u64;
    if normalized.is_empty() {
        return Ok(bytes);
    }
    for (index, item) in tree
        .ranges_pre_order_chunks_iter_ref(&normalized, IROH_BLOCK_SIZE.chunk_log())
        .enumerate()
    {
        if cancel.is_cancelled() {
            return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
        }
        bytes = bytes
            .checked_add(item.without_ranges().size() as u64)
            .ok_or_else(|| BlobExchangeError::refused(BlobExchangeFailure::InvalidRequest))?;
        if index % 4096 == 0 {
            tokio::task::yield_now().await;
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::expected_size;
    use bao_tree::{ChunkNum, ChunkRanges};
    use futures_util::StreamExt;
    use iroh_blobs::store::mem::MemStore;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn sizes_match_actual_sdk_bytes_including_empty_and_sparse_ranges() -> anyhow::Result<()>
    {
        let store = MemStore::new();
        for size in [0, 1, 1024, 16_384, 65_537, 1024 * 1024] {
            let tag = store.blobs().add_bytes(vec![7; size]).await?;
            for ranges in [
                ChunkRanges::all(),
                ChunkRanges::empty(),
                ChunkRanges::from(ChunkNum(1)..ChunkNum(2)),
                ChunkRanges::from(ChunkNum(3)..ChunkNum(10_000)),
                ChunkRanges::from(ChunkNum(1024)..ChunkNum(2048)),
            ] {
                let expected =
                    expected_size(size as u64, &ranges, &CancellationToken::new()).await?;
                let bytes = store
                    .blobs()
                    .export_bao(tag.hash, ranges)
                    .into_byte_stream();
                tokio::pin!(bytes);
                let mut actual = 0;
                while let Some(bytes) = bytes.next().await {
                    actual += bytes?.len() as u64;
                }
                assert_eq!(actual, expected, "SDK response size differs for {size}");
            }
        }
        store.shutdown().await?;
        Ok(())
    }
}
