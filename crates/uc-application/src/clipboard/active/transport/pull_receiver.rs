//! 完整拉取服务；网络取消只停止下一准备动作，已开始的本地动作保留原额度。
use super::error::ActiveClipboardTransportRefusal;
use super::model::CloseOnDrop;
use super::preparation::StopOnDrop;
use super::{
    ActiveClipboardTransportError, ActiveClipboardWirePort, ActivePullLocalWorkPort,
    ActivePullPreparationError, ActivePullPreparationPort, ActivePullPreparationStop,
    ActivePullReceiverPort, ActivePullResponse, CLOSE_BARRIER_TIMEOUT,
};
use crate::connectivity::{ConnectivityError, ConnectivityManager, PeerConnection};
use crate::deps::CurrentSpaceMemberScopePort;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::{timeout_at, Instant};
use uc_observability_contract::{log_fields::log_vocab_debug, uc_debug, uc_info};

pub struct ActivePullReceiver {
    manager: ConnectivityManager,
    wire: Arc<dyn ActiveClipboardWirePort>,
    prepare: Arc<dyn ActivePullPreparationPort>,
    gate: Arc<dyn CurrentSpaceMemberScopePort>,
    local: Arc<dyn ActivePullLocalWorkPort>,
}
impl ActivePullReceiver {
    pub fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn ActiveClipboardWirePort>,
        prepare: Arc<dyn ActivePullPreparationPort>,
        gate: Arc<dyn CurrentSpaceMemberScopePort>,
        local: Arc<dyn ActivePullLocalWorkPort>,
    ) -> Self {
        Self {
            manager,
            wire,
            prepare,
            gate,
            local,
        }
    }
}
#[async_trait]
impl ActivePullReceiverPort for ActivePullReceiver {
    async fn receive(
        &self,
        raw: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> Result<(), ActiveClipboardTransportError> {
        let _close = CloseOnDrop(raw.clone());
        let stop = ActivePullPreparationStop::default();
        let _cancel = StopOnDrop(stop.clone());
        timeout_at(deadline, async {
            let (connection, mut stream) = self.manager.accept_exchange(raw, deadline).await?;
            let hash = self.wire.read_pull_request(stream.receive.as_mut()).await?;
            connection.check_current().await?;
            uc_info!(operation = "active_pull_receive", channel = log_vocab_debug(&connection.channel()), "active clipboard pull request received");
            let (allowed, scope_failure) = match self.gate.snapshot().await {
                Ok(scope) => (scope.local_member_active && scope.usable_peer_device_ids.contains(&connection.peer()), None),
                Err(source) => (false, Some(ActiveClipboardTransportError::Connectivity(ConnectivityError::Admission(Box::new(source))))),
            };
            let response = if !allowed {
                ActivePullResponse::NotAvailable
            } else {
                let quota = stream.hold_for_work();
                let preparation = self.prepare.clone();
                let work_stop = stop.clone();
                let work = Box::pin(async move {
                    let _quota = quota;
                    preparation.prepare(&hash, &work_stop).await
                });
                let result = tokio::select! {
                    biased;
                    _ = connection.closed() => return Err(ActiveClipboardTransportRefusal::PeerClosed.into()),
                    result = self.local.execute(work, stop.clone()) => result,
                };
                match result {
                    Ok(envelope) => ActivePullResponse::Envelope(envelope),
                    Err(ActivePullPreparationError::NotAvailable) => ActivePullResponse::NotAvailable,
                    Err(ActivePullPreparationError::NotUnlocked) => ActivePullResponse::Locked,
                    Err(ActivePullPreparationError::Cancelled(_)) => return Err(ActiveClipboardTransportRefusal::PeerClosed.into()),
                    Err(ActivePullPreparationError::Internal(_)) => ActivePullResponse::Internal,
                }
            };
            connection.check_current().await?;
            self.wire.write_pull_response(stream.send.as_mut(), response).await?;
            stream.send.finish()?;
            let barrier = (Instant::now() + CLOSE_BARRIER_TIMEOUT).min(deadline);
            tokio::select! {
                _ = connection.closed() => {},
                _ = tokio::time::sleep_until(barrier) => {
                    if Instant::now() >= deadline { return Err(ActiveClipboardTransportError::Connectivity(ConnectivityError::DeadlineExpired)); }
                    uc_debug!("active pull response close protection ended");
                }
            }
            // 范围读取失败仍向对端保持原 NotAvailable；本机完整结果保留原错误来源。
            match scope_failure { Some(source) => Err(source), None => Ok(()) }
        }).await?
    }
    async fn shutdown(&self) -> Result<(), ActiveClipboardTransportError> {
        self.manager.shutdown().await?;
        Ok(())
    }
}
