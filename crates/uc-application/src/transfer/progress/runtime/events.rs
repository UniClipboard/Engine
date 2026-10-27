//! 宿主只收到失效提示；网络采用与ACK不等待事件渲染或宿主重读。
use crate::facade::host_event::{HostEvent, HostEventBus, TransferHostEvent};
use crate::transfer::progress::ProgressProjectionEvent;
use std::sync::Arc;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

pub(super) async fn observe_events(
    mut events: broadcast::Receiver<ProgressProjectionEvent>,
    host: Arc<HostEventBus>,
    cancel: CancellationToken,
) {
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return,
            next = events.recv() => next,
        };
        match next {
            Ok(_) | Err(RecvError::Lagged(_)) => host.emit_or_warn(HostEvent::Transfer(
                TransferHostEvent::OutboundProgressChanged,
            )),
            Err(RecvError::Closed) => return,
        }
    }
}
