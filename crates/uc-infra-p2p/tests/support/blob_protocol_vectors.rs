//! 只用固定 SDK 公开编码与存储 API 生成合成向量；同一源码也在冻结旧端编译。
use anyhow::Result;
use futures_util::StreamExt;
use iroh::{EndpointAddr, SecretKey};
use iroh_blobs::{
    api::Store,
    protocol::{GetRequest, Request},
    store::mem::MemStore,
    ticket::BlobTicket,
    BlobFormat, Hash,
};
use iroh_tickets::Ticket;
use serde_json::{json, Value};

async fn one(store: &Store, hash: Hash) -> Result<Value> {
    let addr = EndpointAddr::new(SecretKey::from_bytes(&[7; 32]).public());
    let ticket = BlobTicket::new(addr, hash, BlobFormat::Raw).encode_bytes();
    let request = GetRequest::blob(hash);
    let wire_request = postcard::to_stdvec(&Request::Get(request.clone()))?;
    let ranges = request.ranges.iter().next().expect("root ranges").clone();
    let stream = store.blobs().export_bao(hash, ranges).into_byte_stream();
    tokio::pin!(stream);
    let mut response = Vec::new();
    while let Some(bytes) = stream.next().await {
        response.extend_from_slice(&bytes?);
    }
    Ok(json!({ "ticket": ticket, "request": wire_request, "response": response }))
}
pub async fn generate() -> Result<Value> {
    let store = MemStore::new();
    let raw = store
        .blobs()
        .add_bytes(b"synthetic-blob-wire-v1".as_slice())
        .await?;
    let empty = store.blobs().add_bytes(Vec::new()).await?;
    let vectors = json!({ "alpn": iroh_blobs::ALPN, "raw": one(&store, raw.hash).await?, "empty": one(&store, empty.hash).await? });
    store.shutdown().await?;
    Ok(vectors)
}
