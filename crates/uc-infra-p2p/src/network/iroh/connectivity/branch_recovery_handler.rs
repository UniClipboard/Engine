//! 将认证连接交给Uni及完整分支恢复接收负责人。
use std::{fmt, sync::Arc, time::Duration};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use tokio::time::Instant;
use uc_application::deps::{BranchRecoveryReceiver, ConnectivityManager};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

use super::IrohPeerConnection;

#[derive(Clone)]
pub struct IrohBranchRecoveryHandler {
    manager: ConnectivityManager,
    receiver: Arc<BranchRecoveryReceiver>,
}

impl IrohBranchRecoveryHandler {
    pub fn new(manager: ConnectivityManager, receiver: Arc<BranchRecoveryReceiver>) -> Self {
        Self { manager, receiver }
    }
}

impl fmt::Debug for IrohBranchRecoveryHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IrohBranchRecoveryHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohBranchRecoveryHandler {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        let raw = IrohPeerConnection::wrap(native);
        let started = Instant::now();
        match self
            .manager
            .accept_exchange(raw.clone(), started + Duration::from_secs(10))
            .await
        {
            Ok((connection, stream)) => {
                if let Err(source) = self
                    .receiver
                    .receive(connection, stream, started + Duration::from_secs(30))
                    .await
                {
                    uc_warn!(
                        error_kind = "branch_recovery_receive",
                        io_error_kind = io_error_kind(source.as_ref()),
                        "成员分支恢复入站交换失败"
                    );
                }
            }
            Err(source) => {
                uc_warn!(
                    error_kind = "branch_recovery_accept",
                    io_error_kind = io_error_kind(&source),
                    "成员分支恢复入站连接不可用"
                );
                raw.close();
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        if let Err(source) = self.manager.shutdown().await {
            uc_warn!(
                error_kind = "branch_recovery_shutdown",
                io_error_kind = io_error_kind(&source),
                "成员分支恢复连接关闭失败"
            );
        }
    }
}
