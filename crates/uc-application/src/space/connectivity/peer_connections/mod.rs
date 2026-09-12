//! 已配对设备连接的唯一调度与恢复负责人。

mod runtime;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot, watch, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerReachabilityPort, ReachabilityState};

use crate::facade::roster::PresenceRefreshReport;
use crate::space::membership::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};
use runtime::ConnectionRuntime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectivityOpportunity {
    Foreground,
    SystemWake,
    NetworkChanged,
}

/// 仅表示重新检查的机会；不能据此授予成员资格或直接设置在线。
pub enum ConnectionHint {
    NetworkChanged,
    PeerAddressChanged(DeviceId),
}

#[derive(Debug, thiserror::Error)]
pub enum PeerConnectionError {
    #[error("connection environment observation failed")]
    Environment(#[source] anyhow::Error),
    #[error("peer connection coordinator is closed")]
    Closed,
    #[error("peer connection coordinator is paused")]
    Paused,
    #[error("peer connection coordinator is busy")]
    Busy,
    #[error("peer scope could not be read")]
    Scope(#[source] CurrentSpaceMemberScopeError),
    #[error("peer scope read timed out")]
    ScopeTimeout(#[source] tokio::time::error::Elapsed),
    #[error("peer connection response was interrupted")]
    Response(#[source] oneshot::error::RecvError),
    #[error("peer connection task failed")]
    Task(#[source] tokio::task::JoinError),
}

enum Command {
    Refresh(oneshot::Sender<Result<PresenceRefreshReport, PeerConnectionError>>),
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
}

/// 任务只持有其依赖和接收端，不持有 owner，避免关闭时形成引用环。
pub(crate) struct PeerConnectionCoordinator {
    commands: mpsc::Sender<Command>,
    opportunity: watch::Sender<ConnectivityOpportunity>,
    cancel: CancellationToken,
    runtime: Mutex<(Option<ConnectionRuntime>, Option<JoinHandle<()>>)>,
}

impl PeerConnectionCoordinator {
    pub(crate) fn new(
        scope: Arc<dyn CurrentSpaceMemberScopePort>,
        presence: Arc<dyn PeerReachabilityPort>,
        hints: BoxStream<'static, Result<ConnectionHint, anyhow::Error>>,
    ) -> Arc<Self> {
        let (commands, receiver) = mpsc::channel(32);
        let (opportunity, opportunities) = watch::channel(ConnectivityOpportunity::Foreground);
        let cancel = CancellationToken::new();
        let runtime = ConnectionRuntime::new(
            scope,
            presence,
            hints,
            receiver,
            opportunities,
            cancel.clone(),
        );
        Arc::new(Self {
            commands,
            opportunity,
            cancel,
            runtime: Mutex::new((Some(runtime), None)),
        })
    }

    pub(crate) async fn start(&self) {
        let mut slot = self.runtime.lock().await;
        if let Some(runtime) = slot.0.take() {
            if !self.cancel.is_cancelled() {
                slot.1 = Some(tokio::spawn(runtime.run()));
            }
        }
    }

    pub(crate) fn notify_opportunity(
        &self,
        reason: ConnectivityOpportunity,
    ) -> Result<(), PeerConnectionError> {
        if self.cancel.is_cancelled() {
            return Err(PeerConnectionError::Closed);
        }
        self.opportunity.send_replace(reason);
        Ok(())
    }

    pub(crate) async fn refresh(&self) -> Result<PresenceRefreshReport, PeerConnectionError> {
        let (send, receive) = oneshot::channel();
        self.send(Command::Refresh(send))?;
        receive.await.map_err(PeerConnectionError::Response)?
    }

    pub(crate) async fn pause(&self) -> Result<(), PeerConnectionError> {
        let (send, receive) = oneshot::channel();
        self.send(Command::Pause(send))?;
        receive.await.map_err(PeerConnectionError::Response)
    }

    pub(crate) async fn resume(&self) -> Result<(), PeerConnectionError> {
        let (send, receive) = oneshot::channel();
        self.send(Command::Resume(send))?;
        receive.await.map_err(PeerConnectionError::Response)
    }

    fn send(&self, command: Command) -> Result<(), PeerConnectionError> {
        if self.cancel.is_cancelled() {
            return Err(PeerConnectionError::Closed);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => PeerConnectionError::Busy,
                mpsc::error::TrySendError::Closed(_) => PeerConnectionError::Closed,
            })
    }

    pub(crate) async fn shutdown(&self) -> Result<(), PeerConnectionError> {
        self.cancel.cancel();
        let mut slot = self.runtime.lock().await;
        slot.0.take();
        if let Some(task) = slot.1.take() {
            task.await.map_err(PeerConnectionError::Task)?;
        }
        Ok(())
    }
}

impl Drop for PeerConnectionCoordinator {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
