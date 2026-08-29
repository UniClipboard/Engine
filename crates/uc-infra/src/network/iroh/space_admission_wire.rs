use std::future::Future;
use std::io;
use std::time::Duration;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uc_core::membership::{SpaceAdmissionEnvelopeV1, SpaceAdmissionMessageKind};

const WIRE_MAGIC: [u8; 4] = *b"UCSA";
const WIRE_VERSION: u8 = 1;
const HEADER_LEN: usize = 10;
pub(super) const AUTH_FRAME_LIMIT: usize = 64 * 1024;
pub(super) const DURABLE_MESSAGE_LIMIT: usize = 256 * 1024;
pub(super) const LARGE_MESSAGE_LIMIT: usize = 4 * 1024 * 1024;
pub(super) const IO_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum FrameKind {
    InitialHello = 1,
    OpaqueResponse = 2,
    OpaqueFinish = 3,
    ContinuationHello = 4,
    Request = 5,
    Reply = 6,
}

impl FrameKind {
    fn from_u8(value: u8) -> Result<Self, WireError> {
        match value {
            1 => Ok(Self::InitialHello),
            2 => Ok(Self::OpaqueResponse),
            3 => Ok(Self::OpaqueFinish),
            4 => Ok(Self::ContinuationHello),
            5 => Ok(Self::Request),
            6 => Ok(Self::Reply),
            _ => Err(WireError::UnknownFrame),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct InitialHelloV1 {
    pub protocol_version: u16,
    pub admission_id: [u8; 32],
    pub invitation_id: [u8; 32],
    pub joiner_peer_id: [u8; 32],
    pub ke1: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct OpaqueResponseV1 {
    pub sponsor_peer_id: [u8; 32],
    pub ke2: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct OpaqueFinishV1 {
    pub ke3: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ContinuationHelloV1 {
    pub admission_id: [u8; 32],
    pub local_peer_id: [u8; 32],
    pub remote_peer_id: [u8; 32],
    pub nonce: [u8; 32],
    pub request_digest: [u8; 32],
    pub mac: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct AuthenticatedEnvelopeV1 {
    pub nonce: [u8; 32],
    pub canonical_envelope: Vec<u8>,
    pub mac: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum WireError {
    #[error("space admission wire I/O failed")]
    Io(#[source] io::Error),
    #[error("space admission wire operation timed out")]
    Timeout,
    #[error("space admission wire header is invalid")]
    InvalidHeader,
    #[error("space admission wire frame kind is unknown")]
    UnknownFrame,
    #[error("space admission wire frame length is invalid")]
    InvalidLength,
    #[error("space admission wire payload is invalid")]
    InvalidPayload,
}

pub(super) async fn write_typed<W, T>(
    writer: &mut W,
    kind: FrameKind,
    value: &T,
    limit: usize,
) -> Result<(), WireError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = postcard::to_stdvec(value).map_err(|_| WireError::InvalidPayload)?;
    write_raw(writer, kind, &payload, limit).await
}

pub(super) async fn read_typed<R, T>(
    reader: &mut R,
    expected: FrameKind,
    limit: usize,
) -> Result<T, WireError>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let payload = read_raw(reader, expected, limit).await?;
    postcard::from_bytes(&payload).map_err(|_| WireError::InvalidPayload)
}

pub(super) async fn write_envelope<W: AsyncWrite + Unpin>(
    writer: &mut W,
    kind: FrameKind,
    envelope: &AuthenticatedEnvelopeV1,
) -> Result<(), WireError> {
    let domain = SpaceAdmissionEnvelopeV1::decode_canonical_v1(&envelope.canonical_envelope)
        .map_err(|_| WireError::InvalidPayload)?;
    write_typed(writer, kind, envelope, envelope_limit(domain.kind())).await
}

pub(super) async fn read_envelope<R: AsyncRead + Unpin>(
    reader: &mut R,
    kind: FrameKind,
) -> Result<(AuthenticatedEnvelopeV1, SpaceAdmissionEnvelopeV1, [u8; 32]), WireError> {
    let (actual_kind, payload) = read_raw_with_limit(reader, LARGE_MESSAGE_LIMIT).await?;
    if actual_kind != kind {
        return Err(WireError::UnknownFrame);
    }
    let wire: AuthenticatedEnvelopeV1 =
        postcard::from_bytes(&payload).map_err(|_| WireError::InvalidPayload)?;
    let envelope = SpaceAdmissionEnvelopeV1::decode_canonical_v1(&wire.canonical_envelope)
        .map_err(|_| WireError::InvalidPayload)?;
    if payload.len() > envelope_limit(envelope.kind()) {
        return Err(WireError::InvalidLength);
    }
    let digest = Sha256::digest(&wire.canonical_envelope).into();
    Ok((wire, envelope, digest))
}

async fn write_raw<W: AsyncWrite + Unpin>(
    writer: &mut W,
    kind: FrameKind,
    payload: &[u8],
    limit: usize,
) -> Result<(), WireError> {
    if payload.is_empty() || payload.len() > limit || payload.len() > u32::MAX as usize {
        return Err(WireError::InvalidLength);
    }
    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(&WIRE_MAGIC);
    header[4] = WIRE_VERSION;
    header[5] = kind as u8;
    header[6..].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    run_io(writer.write_all(&header)).await?;
    run_io(writer.write_all(payload)).await
}

async fn read_raw<R: AsyncRead + Unpin>(
    reader: &mut R,
    expected: FrameKind,
    limit: usize,
) -> Result<Vec<u8>, WireError> {
    let (kind, payload) = read_raw_with_limit(reader, limit).await?;
    if kind != expected {
        return Err(WireError::UnknownFrame);
    }
    Ok(payload)
}

pub(super) async fn read_raw_with_limit<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<(FrameKind, Vec<u8>), WireError> {
    let mut header = [0u8; HEADER_LEN];
    run_io(reader.read_exact(&mut header)).await?;
    if header[..4] != WIRE_MAGIC || header[4] != WIRE_VERSION {
        return Err(WireError::InvalidHeader);
    }
    let kind = FrameKind::from_u8(header[5])?;
    let length = u32::from_be_bytes(
        header[6..]
            .try_into()
            .map_err(|_| WireError::InvalidHeader)?,
    ) as usize;
    if length == 0 || length > limit {
        return Err(WireError::InvalidLength);
    }
    let mut payload = vec![0u8; length];
    run_io(reader.read_exact(&mut payload)).await?;
    Ok((kind, payload))
}

async fn run_io<T>(future: impl Future<Output = io::Result<T>>) -> Result<T, WireError> {
    tokio::time::timeout(IO_DEADLINE, future)
        .await
        .map_err(|_| WireError::Timeout)?
        .map_err(WireError::Io)
}

fn envelope_limit(kind: SpaceAdmissionMessageKind) -> usize {
    match kind {
        SpaceAdmissionMessageKind::Candidate
        | SpaceAdmissionMessageKind::Commit
        | SpaceAdmissionMessageKind::Complete => LARGE_MESSAGE_LIMIT,
        _ => DURABLE_MESSAGE_LIMIT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_zero_oversize_unknown_and_truncated_frames_before_payload_decode() {
        let (mut writer, mut reader) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            let mut header = [0u8; HEADER_LEN];
            header[..4].copy_from_slice(&WIRE_MAGIC);
            header[4] = WIRE_VERSION;
            header[5] = FrameKind::InitialHello as u8;
            header[6..].copy_from_slice(&0u32.to_be_bytes());
            writer.write_all(&header).await
        });
        assert!(matches!(
            read_raw(&mut reader, FrameKind::InitialHello, AUTH_FRAME_LIMIT).await,
            Err(WireError::InvalidLength)
        ));
        task.await.expect("writer task").expect("write zero frame");

        let (mut writer, mut reader) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            let mut header = [0u8; HEADER_LEN];
            header[..4].copy_from_slice(&WIRE_MAGIC);
            header[4] = WIRE_VERSION;
            header[5] = 0xff;
            header[6..].copy_from_slice(&1u32.to_be_bytes());
            writer.write_all(&header).await
        });
        assert!(matches!(
            read_raw(&mut reader, FrameKind::InitialHello, AUTH_FRAME_LIMIT).await,
            Err(WireError::UnknownFrame)
        ));
        task.await
            .expect("writer task")
            .expect("write unknown frame");

        let (mut writer, mut reader) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            let mut header = [0u8; HEADER_LEN];
            header[..4].copy_from_slice(&WIRE_MAGIC);
            header[4] = WIRE_VERSION;
            header[5] = FrameKind::InitialHello as u8;
            header[6..].copy_from_slice(&((AUTH_FRAME_LIMIT + 1) as u32).to_be_bytes());
            writer.write_all(&header).await
        });
        assert!(matches!(
            read_raw(&mut reader, FrameKind::InitialHello, AUTH_FRAME_LIMIT).await,
            Err(WireError::InvalidLength)
        ));
        task.await
            .expect("writer task")
            .expect("write oversize frame");

        let (mut writer, mut reader) = tokio::io::duplex(128);
        let task = tokio::spawn(async move { writer.write_all(&WIRE_MAGIC[..2]).await });
        task.await
            .expect("writer task")
            .expect("write truncated frame");
        assert!(matches!(
            read_raw(&mut reader, FrameKind::InitialHello, AUTH_FRAME_LIMIT).await,
            Err(WireError::Io(_))
        ));
    }
}
