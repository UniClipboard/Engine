use super::super::trace_context::set_remote_parent;
use async_trait::async_trait;
use futures_util::stream::{FuturesUnordered, StreamExt};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tracing::Instrument;
use uc_application::deps::{
    ClipboardDelivery, ClipboardReceiverPort, ConnectionLease, ConnectionMode, ConnectivityError,
    ConnectivityManager, PeerConnection, PeerSendStream, StreamLease,
};
use uc_core::ports::{InboundClipboard, InboundClipboardDisposition, InboundClipboardReceipt};
use uc_observability_contract::diagnostics::connectivity::{
    complete_clipboard_receive_failure, ClipboardReceiveFailure, ClipboardReceiveObservation,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};
use uc_sync_protocol::clipboard::{self as clipboard_wire, AckCode};

const INBOUND_CHANNEL_CAPACITY: usize = 64;
const APPLICATION_SETTLEMENT_TIMEOUT: Duration = Duration::from_secs(60);
const FRAME_READ_TIMEOUT: Duration = Duration::from_secs(60);

pub struct ClipboardReceiverAdapter {
    manager: ConnectivityManager,
    event_tx: broadcast::Sender<ClipboardDelivery>,
}
impl ClipboardReceiverAdapter {
    pub fn new(manager: ConnectivityManager) -> Self {
        let (event_tx, _) = broadcast::channel(INBOUND_CHANNEL_CAPACITY);
        Self { manager, event_tx }
    }
    pub async fn shutdown(&self) -> Result<(), ConnectivityError> {
        self.manager.shutdown().await
    }
    pub async fn receive(&self, raw: Arc<dyn PeerConnection>) {
        let connection = match tokio::time::timeout(
            FRAME_READ_TIMEOUT,
            self.manager.accept(raw.clone()),
        )
        .await
        {
            Ok(Ok(connection)) => connection,
            Err(_) => {
                raw.close();
                return;
            }
            Ok(Err(error)) if error.is_busy() => {
                raw.close();
                return;
            }
            rejection => {
                // 保留拒绝结果内的有限额度直到回执结算，不创建无界拒绝任务。
                let _rejection = rejection;
                // 未准入连接只允许一次有界拒绝回执，不解码或交接内容。
                let _ = tokio::time::timeout(FRAME_READ_TIMEOUT, async {
                    if let Ok((mut send, _recv)) = raw.accept_bi().await {
                        emit_ack(&mut send, AckCode::Rejected).await;
                        raw.closed().await;
                    }
                })
                .await;
                raw.close();
                return;
            }
        };
        if connection.mode() == ConnectionMode::SingleExchange {
            if let Ok(Ok(stream)) =
                tokio::time::timeout(FRAME_READ_TIMEOUT, connection.accept_bi()).await
            {
                self.exchange(connection.clone(), stream).await;
                let _ =
                    tokio::time::timeout(APPLICATION_SETTLEMENT_TIMEOUT, connection.closed()).await;
            }
            return;
        }
        // 同一被 generation scope 包裹的任务内并发轮询，避免 spawn 丢失 task-local 世代。
        let mut exchanges = FuturesUnordered::new();
        let mut accepting = Box::pin(tokio::time::timeout(
            FRAME_READ_TIMEOUT,
            connection.accept_bi(),
        ));
        loop {
            tokio::select! {
                biased;
                _ = connection.closed() => break,
                _ = exchanges.next(), if !exchanges.is_empty() => {},
                stream = &mut accepting => {
                    match stream {
                        Ok(Ok(stream)) => exchanges.push(self.exchange(connection.clone(), stream)),
                        Ok(Err(_)) => break,
                        Err(_) if exchanges.is_empty() => break,
                        Err(_) => {},
                    }
                    // 兄弟流完成不取消已接受且正在准入的流，也不重置其排队期限。
                    accepting.set(tokio::time::timeout(FRAME_READ_TIMEOUT, connection.accept_bi()));
                }
            }
        }
    }
    async fn exchange(&self, connection: ConnectionLease, mut stream: StreamLease) {
        let peer_device_id = connection.peer();
        let read_deadline = tokio::time::Instant::now() + FRAME_READ_TIMEOUT;
        let decoded = match tokio::time::timeout(
            FRAME_READ_TIMEOUT,
            clipboard_wire::read_frame_header(&mut stream.receive),
        )
        .await
        {
            Err(_) => {
                emit_ack(&mut stream.send, AckCode::Rejected).await;
                return;
            }
            Ok(decoded) => match decoded {
                Ok(decoded) => decoded,
                Err(err) => {
                    uc_warn!(
                        error_kind = "frame_decode",
                        io_error_kind = io_error_kind(&err),
                        "clipboard receiver: frame decode failed; sending Rejected ack"
                    );
                    let ack = if matches!(err, clipboard_wire::WireDecodeError::IncompatibleLayout)
                    {
                        AckCode::Incompatible
                    } else {
                        AckCode::Rejected
                    };
                    emit_ack(&mut stream.send, ack).await;

                    return;
                }
            },
        };

        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Clipboard,
            operation: DiagnosticOperation::ClipboardReceive,
            role: DiagnosticRole::Server,
            kind: DiagnosticSpanKind::Server,
        });
        let _ = set_remote_parent(&span, decoded.trace_context.as_ref());

        let receive_observation = ClipboardReceiveObservation::default();
        let operation_observation = receive_observation.clone();
        let result = async move {
            let started = Instant::now();
            let ciphertext = match tokio::time::timeout_at(
                read_deadline,
                clipboard_wire::read_frame_payload(&mut stream.receive, decoded.payload_len()),
            )
            .await
            {
                Err(_) => {
                    complete_operation(OperationCompletion::failed(
                        DiagnosticDomain::Clipboard,
                        DiagnosticOperation::ClipboardReceive,
                        DiagnosticRole::Server,
                        DiagnosticErrorType::Timeout,
                        started.elapsed(),
                    ));
                    emit_ack(&mut stream.send, AckCode::Rejected).await;
                    return;
                }
                Ok(payload) => match payload {
                    Ok(ciphertext) => ciphertext,
                    Err(error) => {
                        complete_operation(OperationCompletion::failed(
                            DiagnosticDomain::Clipboard,
                            DiagnosticOperation::ClipboardReceive,
                            DiagnosticRole::Server,
                            DiagnosticErrorType::StreamFailed,
                            started.elapsed(),
                        ));
                        uc_warn!(
                            error_kind = "payload_read",
                            io_error_kind = io_error_kind(&error),
                            "clipboard receiver: payload read failed; sending Rejected ack"
                        );
                        emit_ack(&mut stream.send, AckCode::Rejected).await;
                        return;
                    }
                },
            };

            // 复核失败时没有应用层结算可言，但这次接收已经开始：必须以明确结果结束，而不是静默丢弃。
            let current = tokio::time::timeout_at(read_deadline, connection.check_current())
                .await
                .map_err(ConnectivityError::TimedOut)
                .and_then(|checked| checked);
            if let Err(error) = current {
                let (failure, error_type) = match error {
                    ConnectivityError::Denied => (
                        ClipboardReceiveFailure::AdmissionRevoked,
                        DiagnosticErrorType::PeerRejected,
                    ),
                    ConnectivityError::TimedOut(_) | ConnectivityError::DeadlineExpired => (
                        ClipboardReceiveFailure::AdmissionCheckTimedOut,
                        DiagnosticErrorType::Timeout,
                    ),
                    _ => (
                        ClipboardReceiveFailure::AdmissionCheckFailed,
                        DiagnosticErrorType::Unavailable,
                    ),
                };
                operation_observation.finish_failure(
                    failure,
                    OperationCompletion::failed(
                        DiagnosticDomain::Clipboard,
                        DiagnosticOperation::ClipboardReceive,
                        DiagnosticRole::Server,
                        error_type,
                        started.elapsed(),
                    ),
                );
                return;
            }
            // 认证后的任务通过原有广播交接；没有消费者时拒绝，绝不伪报保存成功。
            let (receipt, result) = InboundClipboardReceipt::pending();
            let transport = connection.channel();
            let inbound = InboundClipboard {
                peer_device_id,
                header: decoded.header,
                ciphertext,
                transport,
                receipt,
            };
            if self.event_tx.send(ClipboardDelivery::new(inbound)).is_err() {
                uc_debug!("clipboard receiver: no subscribers attached; inbound frame dropped");
                emit_ack(&mut stream.send, AckCode::Rejected).await;
                complete_clipboard_receive_failure(
                    ClipboardReceiveFailure::NoConsumer,
                    OperationCompletion::failed(
                        DiagnosticDomain::Clipboard,
                        DiagnosticOperation::ClipboardReceive,
                        DiagnosticRole::Server,
                        DiagnosticErrorType::Unavailable,
                        started.elapsed(),
                    ),
                );
                return;
            }

            let settlement =
                tokio::time::timeout(APPLICATION_SETTLEMENT_TIMEOUT, result.wait()).await;
            let (disposition, failure) = match settlement {
                Ok(Some(disposition)) => (
                    Some(disposition),
                    ClipboardReceiveFailure::ApplicationRejected,
                ),
                Ok(None) => (None, ClipboardReceiveFailure::ReceiptDropped),
                Err(_) => (None, ClipboardReceiveFailure::SettlementTimeout),
            };
            let ack = match disposition {
                Some(InboundClipboardDisposition::Applied) => AckCode::Accepted,
                Some(InboundClipboardDisposition::Duplicate) => AckCode::DuplicateIgnored,
                Some(InboundClipboardDisposition::Rejected) | None => AckCode::Rejected,
            };

            let completion = match ack {
                AckCode::Accepted | AckCode::DuplicateIgnored => OperationCompletion::succeeded(
                    DiagnosticDomain::Clipboard,
                    DiagnosticOperation::ClipboardReceive,
                    DiagnosticRole::Server,
                    started.elapsed(),
                ),
                AckCode::Rejected | AckCode::Incompatible => OperationCompletion::failed(
                    DiagnosticDomain::Clipboard,
                    DiagnosticOperation::ClipboardReceive,
                    DiagnosticRole::Server,
                    DiagnosticErrorType::Unavailable,
                    started.elapsed(),
                ),
            };
            if matches!(ack, AckCode::Rejected | AckCode::Incompatible) {
                operation_observation.finish_failure(failure, completion);
            } else {
                complete_operation(completion);
            }

            // 独立流发送持久接受结果；其他流不等待本次结算。
            emit_ack(&mut stream.send, ack).await;
        }
        .instrument(span);
        receive_observation.scope(result).await;
    }
}
#[async_trait]
impl ClipboardReceiverPort for ClipboardReceiverAdapter {
    fn subscribe(&self) -> broadcast::Receiver<ClipboardDelivery> {
        self.event_tx.subscribe()
    }
}
async fn emit_ack(send: &mut Box<dyn PeerSendStream>, ack: AckCode) {
    let write = tokio::time::timeout(FRAME_READ_TIMEOUT, send.write_all(&[ack.as_byte()])).await;
    let error = match write {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(_) => {
            uc_debug!(
                error_kind = "ack_timeout",
                "clipboard receiver: ack deadline expired"
            );
            return;
        }
    };
    if let Some(error) = error {
        uc_debug!(
            error_kind = "ack_write",
            io_error_kind = io_error_kind(&error),
            "clipboard receiver: ack write failed"
        );
        return;
    }
    if let Err(error) = send.finish() {
        uc_debug!(
            error_kind = "send_finish",
            io_error_kind = io_error_kind(&error),
            "clipboard receiver: send.finish failed"
        );
    }
}
