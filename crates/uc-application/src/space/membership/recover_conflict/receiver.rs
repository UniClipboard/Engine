//! 恢复签发的完整入站交换；本地工作由会话负责人监督至实际结束。
use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio::time::{timeout_at, Instant};
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

use super::ports::BranchRecoveryIssuerScope;
use super::{
    BeginMembershipBranchRecoveryInput, BranchRecoveryRequest, BranchRecoveryResponse,
    BranchRecoveryWirePort, IssueMembershipBranchRecoveryError, IssueMembershipBranchRecoveryInput,
    IssueMembershipBranchRecoveryPort,
};
use crate::connectivity::{ConnectionLease, ConnectionProtocol, ConnectivityError, StreamLease};

impl BranchRecoveryIssuerScope for ConnectionLease {
    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        ConnectionLease::check_frozen(self)
    }
}

pub type BranchRecoveryWork = BoxFuture<'static, Result<BranchRecoveryResponse, anyhow::Error>>;

#[async_trait]
pub trait BranchRecoveryLocalWorkPort: Send + Sync {
    async fn execute(
        &self,
        work: BranchRecoveryWork,
    ) -> Result<Result<BranchRecoveryResponse, anyhow::Error>, anyhow::Error>;
}

pub struct BranchRecoveryReceiver {
    wire: Arc<dyn BranchRecoveryWirePort>,
    local_work: Arc<dyn BranchRecoveryLocalWorkPort>,
    issuer: Arc<dyn IssueMembershipBranchRecoveryPort>,
}

impl BranchRecoveryReceiver {
    pub fn new(
        wire: Arc<dyn BranchRecoveryWirePort>,
        local_work: Arc<dyn BranchRecoveryLocalWorkPort>,
        issuer: Arc<dyn IssueMembershipBranchRecoveryPort>,
    ) -> Self {
        Self {
            wire,
            local_work,
            issuer,
        }
    }

    pub async fn receive(
        &self,
        connection: ConnectionLease,
        mut stream: StreamLease,
        deadline: Instant,
    ) -> Result<(), anyhow::Error> {
        if connection.protocol() != ConnectionProtocol::MembershipBranchRecovery {
            return Err(anyhow::anyhow!("unexpected recovery protocol"));
        }
        let request = self
            .wire
            .read_request(stream.receive.as_mut(), deadline)
            .await?;
        connection.check_current().await?;
        let peer = connection.peer();
        let issuer = self.issuer.clone();
        let active = connection.clone();
        let hold = stream.hold_for_work();
        let work: BranchRecoveryWork = Box::pin(async move {
            let _hold = hold;
            active.check_current().await?;
            match request {
                BranchRecoveryRequest::GroupInfo {
                    conflict_id,
                    target_branch_id,
                    recipient_member,
                } => {
                    match issuer
                        .begin_membership_branch_recovery(BeginMembershipBranchRecoveryInput {
                            scope: Arc::new(active.clone()),
                            source_device_id: peer,
                            conflict_id,
                            target_branch_id,
                            recipient_member,
                        })
                        .await
                    {
                        Ok(group_info) => Ok(BranchRecoveryResponse::GroupInfo(group_info)),
                        Err(source) => Err(anyhow::Error::new(source)),
                    }
                }
                BranchRecoveryRequest::ExternalCommit {
                    conflict_id,
                    target_branch_id,
                    recipient_member,
                    external_commit,
                } => {
                    match issuer
                        .issue_membership_branch_recovery(IssueMembershipBranchRecoveryInput {
                            scope: Arc::new(active.clone()),
                            source_device_id: peer,
                            conflict_id,
                            target_branch_id,
                            recipient_member,
                            external_commit,
                        })
                        .await
                    {
                        Ok(package) => Ok(BranchRecoveryResponse::RecoveryPackage(package)),
                        Err(source) => Err(anyhow::Error::new(source)),
                    }
                }
            }
        });
        let work = Box::pin(observe_issuer_work(work));
        let response = match timeout_at(deadline, self.local_work.execute(work)).await?? {
            Ok(response) => response,
            Err(_) => BranchRecoveryResponse::Rejected,
        };
        connection.check_current().await?;
        self.wire
            .write_response(stream.send.as_mut(), response, deadline)
            .await?;
        timeout_at(deadline, stream.send.settled()).await??;
        connection.close();
        Ok(())
    }
}

async fn observe_issuer_work(
    work: BranchRecoveryWork,
) -> Result<BranchRecoveryResponse, anyhow::Error> {
    let result = work.await;
    if let Err(source) = &result {
        record_issuer_failure(source);
    }
    result
}

fn record_issuer_failure(source: &anyhow::Error) {
    // discarded-source[business-outcome]: 对端只收到拒绝，本机按固定分类记录。
    match source.downcast_ref::<IssueMembershipBranchRecoveryError>() {
        Some(IssueMembershipBranchRecoveryError::Rejected { .. }) => {
            uc_debug!(error_kind = "rejected", "成员分支恢复签发拒绝")
        }
        Some(IssueMembershipBranchRecoveryError::Corrupt { .. }) => uc_warn!(
            error_kind = "issuer_corrupt",
            io_error_kind = io_error_kind(source.as_ref()),
            "成员分支恢复签发状态损坏"
        ),
        _ => uc_warn!(
            error_kind = "issuer_unavailable",
            io_error_kind = io_error_kind(source.as_ref()),
            "成员分支恢复签发不可用"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        observe_issuer_work, record_issuer_failure, BranchRecoveryWork,
        IssueMembershipBranchRecoveryError,
    };
    use uc_testkit::log_capture::CapturedLogs;

    #[test]
    fn issuer_failure_categories_do_not_expose_sources() {
        let logs = CapturedLogs::default();
        let _guard = logs.install();
        let source = || anyhow::anyhow!("PRIVATE_ISSUER_DETAIL");
        for error in [
            IssueMembershipBranchRecoveryError::Rejected { source: source() },
            IssueMembershipBranchRecoveryError::Unavailable { source: source() },
            IssueMembershipBranchRecoveryError::Corrupt { source: source() },
        ] {
            record_issuer_failure(&anyhow::Error::new(error));
        }
        assert_eq!(logs.count("成员分支恢复签发拒绝"), 1);
        assert_eq!(logs.count("成员分支恢复签发不可用"), 1);
        assert_eq!(logs.count("成员分支恢复签发状态损坏"), 1);
        assert!(!logs.output().contains("PRIVATE"));
    }

    #[tokio::test]
    async fn late_issuer_failure_is_recorded_after_waiter_is_dropped() {
        let logs = CapturedLogs::default();
        let _guard = logs.install();
        let (release, wait) = tokio::sync::oneshot::channel::<()>();
        let work: BranchRecoveryWork = Box::pin(async move {
            wait.await.unwrap();
            Err(anyhow::Error::new(
                IssueMembershipBranchRecoveryError::Unavailable {
                    source: anyhow::anyhow!("PRIVATE_LATE_FAILURE"),
                },
            ))
        });
        let detached = tokio::spawn(observe_issuer_work(work));
        drop(detached);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while logs.count("成员分支恢复签发不可用") == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("后台失败应被记录");
        assert_eq!(logs.count("成员分支恢复签发不可用"), 1);
        assert!(!logs.output().contains("PRIVATE"));
    }
}
