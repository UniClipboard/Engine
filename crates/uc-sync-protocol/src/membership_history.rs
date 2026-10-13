//! 成员历史 V4 封套；/5 只协商多流连接能力，不改变历史消息字节。
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uc_core::membership::{MembershipHistoryMessage, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE};

use crate::WireTraceContext;

pub const MEMBERSHIP_HISTORY_EXCHANGE_ALPN: &[u8] = b"uniclipboard/membership-history/4";
pub const MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN: &[u8] = b"uniclipboard/membership-history/5";
const WIRE_VERSION: u8 = 4;
const REQUEST_LAYOUT_MARKER: &[u8; 4] = b"UCT1";

#[derive(Serialize, Deserialize)]
pub struct MembershipHistoryRequest {
    pub trace_context: Option<WireTraceContext>,
    pub message: MembershipHistoryMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MembershipHistoryStatus {
    Accepted = 1,
    Rejected = 2,
    Busy = 3,
    /// 仅 /5：消息已接受，但本连接只完成本次交换。业务消息体仍为 V4。
    AcceptedClosing = 4,
}
impl MembershipHistoryStatus {
    pub fn decode(value: u8) -> Result<Self, MembershipHistoryWireError> {
        match value {
            1 => Ok(Self::Accepted),
            2 => Ok(Self::Rejected),
            3 => Ok(Self::Busy),
            4 => Ok(Self::AcceptedClosing),
            _ => Err(MembershipHistoryWireError::InvalidLayout),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MembershipHistoryWireError {
    #[error("invalid membership history frame length")]
    InvalidLength,
    #[error("invalid membership history frame layout")]
    InvalidLayout,
    #[error("membership history encoding failed")]
    Encoding(#[from] postcard::Error),
    #[error("membership history stream failed")]
    Io(#[from] std::io::Error),
}

pub fn checked_message_length(length: usize) -> Result<usize, MembershipHistoryWireError> {
    if length == 0 || length > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryWireError::InvalidLength);
    }
    Ok(length)
}

pub fn encode_message(
    message: &MembershipHistoryMessage,
) -> Result<Vec<u8>, MembershipHistoryWireError> {
    let mut payload = vec![WIRE_VERSION];
    payload.extend(postcard::to_stdvec(message)?);
    checked_message_length(payload.len())?;
    Ok(payload)
}

pub fn encode_request(
    message: MembershipHistoryMessage,
    trace_context: Option<WireTraceContext>,
) -> Result<Vec<u8>, MembershipHistoryWireError> {
    let mut payload = vec![WIRE_VERSION];
    payload.extend_from_slice(REQUEST_LAYOUT_MARKER);
    payload.extend(postcard::to_stdvec(&MembershipHistoryRequest {
        trace_context,
        message,
    })?);
    checked_message_length(payload.len())?;
    Ok(payload)
}

fn body(payload: &[u8]) -> Result<&[u8], MembershipHistoryWireError> {
    checked_message_length(payload.len())?;
    match payload.split_first() {
        Some((&WIRE_VERSION, body)) if !body.is_empty() => Ok(body),
        _ => Err(MembershipHistoryWireError::InvalidLayout),
    }
}

pub fn decode_message(
    payload: &[u8],
) -> Result<MembershipHistoryMessage, MembershipHistoryWireError> {
    Ok(postcard::from_bytes(body(payload)?)?)
}

pub fn decode_request(
    payload: &[u8],
) -> Result<MembershipHistoryRequest, MembershipHistoryWireError> {
    let body = body(payload)?
        .strip_prefix(REQUEST_LAYOUT_MARKER)
        .ok_or(MembershipHistoryWireError::InvalidLayout)?;
    Ok(postcard::from_bytes(body)?)
}

pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(
    receive: &mut R,
) -> Result<Vec<u8>, MembershipHistoryWireError> {
    let length = checked_message_length(receive.read_u32().await? as usize)?;
    let mut payload = vec![0; length];
    receive.read_exact(&mut payload).await?;
    Ok(payload)
}

pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    send: &mut W,
    payload: &[u8],
) -> Result<(), MembershipHistoryWireError> {
    checked_message_length(payload.len())?;
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await?;
    send.write_all(payload).await?;
    Ok(())
}
