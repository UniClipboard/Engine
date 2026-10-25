//! 只回传已知业务拒绝；传输、时钟与解析失败保留原来源，不伪造业务结论。
use super::io::Exchange;
use uc_application::deps::ProgressReceiveError;
use uc_sync_protocol::transfer_progress::v2::{Message, RejectionReason, WireId};

pub(super) fn received(reason: RejectionReason) -> ProgressReceiveError {
    match reason {
        RejectionReason::AccessDenied => {
            ProgressReceiveError::AccessDenied(Box::new(PeerAccessDenied))
        }
        RejectionReason::Stale => ProgressReceiveError::Stale,
        RejectionReason::Busy => ProgressReceiveError::Busy,
        RejectionReason::UnknownTask => ProgressReceiveError::UnknownTask,
        RejectionReason::InvalidScope => ProgressReceiveError::InvalidPlan,
    }
}

pub(super) async fn send(
    exchange: &mut Exchange<'_>,
    correlation: WireId,
    error: &ProgressReceiveError,
) -> Result<(), ProgressReceiveError> {
    let reason = match error {
        ProgressReceiveError::AccessDenied(_) => RejectionReason::AccessDenied,
        ProgressReceiveError::Stale => RejectionReason::Stale,
        ProgressReceiveError::Busy => RejectionReason::Busy,
        ProgressReceiveError::UnknownTask => RejectionReason::UnknownTask,
        ProgressReceiveError::InvalidPlan | ProgressReceiveError::Conflict => {
            RejectionReason::InvalidScope
        }
        _ => return Ok(()),
    };
    exchange
        .write(Message::Rejected {
            challenge: correlation,
            reason,
        })
        .await?;
    // 先让客户端结束请求，再发送本端FIN；客户端等待此FIN时继续持有发送半流。
    exchange.end_request().await?;
    exchange.finish_send().await
}

/// 已认证对端明确回传的固定拒绝原因，不使用自由文本或伪造本机I/O失败。
#[derive(Debug, thiserror::Error)]
#[error("progress peer refused access")]
struct PeerAccessDenied;
