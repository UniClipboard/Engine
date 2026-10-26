use super::super::trace_context::inject_current;
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectionProtocol, ConnectivityError, ConnectivityManager,
};
use uc_core::ids::DeviceId;
use uc_core::ports::{
    ClipboardDispatchError, ClipboardDispatchPort, ClipboardHeader, ConnectionChannel, DispatchAck,
    DispatchReport, SyncPayload,
};
use uc_sync_protocol::clipboard::{self as clipboard_wire, AckCode, WireEncodeError};

const OPERATION_TIMEOUT: Duration = Duration::from_secs(75);
pub struct ClipboardDispatchAdapter {
    manager: ConnectivityManager,
    timeout: Duration,
}
impl ClipboardDispatchAdapter {
    pub fn new(manager: ConnectivityManager) -> Self {
        Self {
            manager,
            timeout: OPERATION_TIMEOUT,
        }
    }
    #[cfg(test)]
    pub(super) fn with_ack_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    async fn exchange(
        &self,
        connection: &ConnectionLease,
        header: &ClipboardHeader,
        payload: &SyncPayload,
        deadline: Instant,
    ) -> Result<DispatchAck, ClipboardDispatchError> {
        let mut stream = connection
            .open_bi(deadline)
            .await
            .map_err(map_connectivity_err)?;
        let mut wire_header = header.clone();
        if connection.protocol() != ConnectionProtocol::AttributedClipboard {
            // 实际协商的旧能力只发送冻结旧头；不追加尾随数据或在错误后重试旧格式。
            wire_header.source_entry = None;
        }
        clipboard_wire::write_frame(
            &mut stream.send,
            &wire_header,
            inject_current(),
            &payload.ciphertext,
        )
        .await
        .map_err(map_encode_err)?;
        // 帧已交给传输层后，流失败只说明确认未到达；对端可能已保存，重试靠接收端去重。
        stream
            .send
            .finish()
            .map_err(|error| ClipboardDispatchError::Transient(Box::new(error)))?;
        let mut ack = [0u8; 1];
        stream
            .receive
            .read_exact(&mut ack)
            .await
            .map_err(|error| ClipboardDispatchError::Transient(Box::new(error)))?;
        match AckCode::try_from(ack[0]) {
            Ok(AckCode::Accepted) => Ok(DispatchAck::Accepted),
            Ok(AckCode::DuplicateIgnored) => Ok(DispatchAck::DuplicateIgnored),
            Ok(AckCode::Incompatible) => Err(ClipboardDispatchError::PeerIncompatible),
            Ok(AckCode::Rejected) => Err(ClipboardDispatchError::PeerRejected(
                "peer returned Rejected ack".into(),
            )),
            Err(_) => Err(ClipboardDispatchError::PeerRejected(
                "peer returned unknown ack byte".into(),
            )),
        }
    }
}
#[async_trait]
impl ClipboardDispatchPort for ClipboardDispatchAdapter {
    async fn dispatch(
        &self,
        target: &DeviceId,
        header: &ClipboardHeader,
        payload: SyncPayload,
    ) -> DispatchReport {
        let deadline = Instant::now() + self.timeout;
        if payload.ciphertext.len() > clipboard_wire::MAX_PAYLOAD_SIZE as usize {
            return DispatchReport {
                transport: ConnectionChannel::Unknown,
                outcome: Err(ClipboardDispatchError::LocalPolicyExceeded(format!(
                    "ciphertext {} bytes exceeds wire MAX_PAYLOAD_SIZE {}",
                    payload.ciphertext.len(),
                    clipboard_wire::MAX_PAYLOAD_SIZE
                ))),
            };
        }
        let connection = match self
            .manager
            .request(*target)
            .connect(deadline, &CancellationToken::new())
            .await
        {
            Ok(connection) => connection,
            Err(error) => {
                return DispatchReport {
                    transport: ConnectionChannel::Unknown,
                    outcome: Err(map_acquisition_err(error)),
                };
            }
        };
        let outcome = match timeout_at(
            deadline,
            self.exchange(&connection, header, &payload, deadline),
        )
        .await
        {
            Ok(result) => result,
            Err(error) => Err(ClipboardDispatchError::Transient(Box::new(error))),
        };
        // presence 已确认对端离线时，暂时性网络失败归为离线；本机授权与资源失败保持原分类。
        let outcome = match outcome {
            Err(ClipboardDispatchError::Transient(_)) if connection.is_unreachable() => {
                Err(ClipboardDispatchError::Offline)
            }
            outcome => outcome,
        };
        DispatchReport {
            transport: connection.channel(),
            outcome,
        }
    }
}

/// 取得连接失败的分类。准入授权拒绝（含经共享拨号结果返回的拒绝）是终态失败，必须先于
/// 既有的宽泛不可用判断处理，否则会被记为可恢复的离线并被反复重试；其余真正的不可达
/// （无地址、无可用连接、底层拨号失败）仍归为离线，交给恢复负责人。终态分类保留原始来源。
fn map_acquisition_err(error: ConnectivityError) -> ClipboardDispatchError {
    if error.is_denied() {
        ClipboardDispatchError::Io(Box::new(error))
    } else if error.is_unavailable() {
        // 既有稳定端口的离线分类是最后的业务边界；manager 内保留完整来源。
        ClipboardDispatchError::Offline
    } else {
        map_connectivity_err(error)
    }
}

/// 暂时性连接或容量失败可重试；其余连接失败（准入拒绝、本机负责人停止、地址资料读取失败、
/// 准入结算失败等）保持不可重试。
fn map_connectivity_err(error: ConnectivityError) -> ClipboardDispatchError {
    if error.is_transient() {
        ClipboardDispatchError::Transient(Box::new(error))
    } else {
        ClipboardDispatchError::Io(Box::new(error))
    }
}

fn map_encode_err(err: WireEncodeError) -> ClipboardDispatchError {
    match err {
        WireEncodeError::SourceIdentity(source) => ClipboardDispatchError::Internal(
            anyhow::Error::new(source)
                .context("source entry encode")
                .into(),
        ),
        WireEncodeError::Io(ioerr) => ClipboardDispatchError::Transient(
            anyhow::Error::new(ioerr).context("frame write").into(),
        ),
        WireEncodeError::PayloadTooLarge { size, max } => {
            // wire codec also enforces MAX_PAYLOAD_SIZE; if we somehow get
            // here (the upstream early-reject in `dispatch` should have
            // caught it first), surface the same `LocalPolicyExceeded`
            // semantics — peer was never reached.
            ClipboardDispatchError::LocalPolicyExceeded(format!(
                "wire codec payload {size} bytes exceeds local maximum {max}"
            ))
        }
        WireEncodeError::HeaderTooLarge { size, max } => ClipboardDispatchError::Internal(
            format!("self-built header {size} bytes exceeds {max}").into(),
        ),
        WireEncodeError::Postcard(err) => ClipboardDispatchError::Internal(
            anyhow::Error::from(err).context("header encode").into(),
        ),
        WireEncodeError::UnsupportedVersion(version) => ClipboardDispatchError::Internal(
            format!("unsupported outbound wire version {version}").into(),
        ),
    }
}
