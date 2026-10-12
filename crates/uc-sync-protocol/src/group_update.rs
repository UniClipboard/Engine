//! 组更新 /2 与 /1 的既有请求格式及单字节确认。
//! 上下文由调用方提供；本模块不注入 span，也不决定授权、应用或重试。

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::WireTraceContext;

pub const GROUP_UPDATE_ALPN: &[u8] = b"uniclipboard/group-update/2";
pub const LEGACY_GROUP_UPDATE_ALPN: &[u8] = b"uniclipboard/group-update/1";
pub const MAX_UPDATE_SIZE: usize = 4 * 1024 * 1024;
pub const MAX_WIRE_SIZE: usize = MAX_UPDATE_SIZE + 1024;
const WIRE_LAYOUT_MARKER: &[u8; 4] = b"UCT1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupUpdateVersion {
    Legacy,
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupUpdateAck {
    Applied,
    Rejected,
    Unavailable,
}

impl GroupUpdateAck {
    /// /1 不能表达暂不可用；调用方应结束交换而非发送拒绝。
    pub fn encode(self, version: GroupUpdateVersion) -> Option<u8> {
        match (self, version) {
            (Self::Applied, _) => Some(1),
            (Self::Rejected, _) => Some(2),
            (Self::Unavailable, GroupUpdateVersion::Current) => Some(3),
            (Self::Unavailable, GroupUpdateVersion::Legacy) => None,
        }
    }

    pub fn decode(byte: u8, version: GroupUpdateVersion) -> Result<Self, GroupUpdateWireError> {
        match (byte, version) {
            (1, _) => Ok(Self::Applied),
            (2, _) => Ok(Self::Rejected),
            (3, GroupUpdateVersion::Current) => Ok(Self::Unavailable),
            _ => Err(GroupUpdateWireError::InvalidAck),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct GroupUpdateRequest {
    pub trace_context: Option<WireTraceContext>,
    pub payload: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum GroupUpdateWireError {
    #[error("invalid group update length")]
    InvalidLength,
    #[error("invalid group update layout")]
    InvalidLayout,
    #[error("invalid group update acknowledgement")]
    InvalidAck,
    #[error("group update encoding failed")]
    Encoding(#[from] postcard::Error),
    #[error("group update stream failed")]
    Io(#[from] std::io::Error),
}

pub fn encode_request(
    payload: &[u8],
    trace_context: Option<WireTraceContext>,
) -> Result<Vec<u8>, GroupUpdateWireError> {
    if payload.is_empty() || payload.len() > MAX_UPDATE_SIZE {
        return Err(GroupUpdateWireError::InvalidLength);
    }
    let mut encoded = WIRE_LAYOUT_MARKER.to_vec();
    encoded.extend(postcard::to_stdvec(&GroupUpdateRequest {
        trace_context,
        payload: payload.to_vec(),
    })?);
    if encoded.len() > MAX_WIRE_SIZE {
        return Err(GroupUpdateWireError::InvalidLength);
    }
    Ok(encoded)
}

pub fn decode_request(encoded: &[u8]) -> Result<GroupUpdateRequest, GroupUpdateWireError> {
    if encoded.is_empty() || encoded.len() > MAX_WIRE_SIZE {
        return Err(GroupUpdateWireError::InvalidLength);
    }
    let body = encoded
        .strip_prefix(WIRE_LAYOUT_MARKER)
        .ok_or(GroupUpdateWireError::InvalidLayout)?;
    let (request, trailing): (GroupUpdateRequest, &[u8]) = postcard::take_from_bytes(body)?;
    if !trailing.is_empty() {
        return Err(GroupUpdateWireError::InvalidLayout);
    }
    if request.payload.is_empty() || request.payload.len() > MAX_UPDATE_SIZE {
        return Err(GroupUpdateWireError::InvalidLength);
    }
    Ok(request)
}

pub async fn write_request<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    payload: &[u8],
    trace_context: Option<WireTraceContext>,
) -> Result<(), GroupUpdateWireError> {
    let encoded = encode_request(payload, trace_context)?;
    writer
        .write_all(&(encoded.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&encoded).await?;
    Ok(())
}

pub async fn read_request<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
) -> Result<GroupUpdateRequest, GroupUpdateWireError> {
    let length = reader.read_u32().await? as usize;
    if length == 0 || length > MAX_WIRE_SIZE {
        return Err(GroupUpdateWireError::InvalidLength);
    }
    let mut encoded = vec![0; length];
    reader.read_exact(&mut encoded).await?;
    decode_request(&encoded)
}
