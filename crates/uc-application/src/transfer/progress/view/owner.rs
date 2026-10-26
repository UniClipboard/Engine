use super::error::{NotCurrentMember, RemoteScopeDenied, SharedQueryFailure};
use super::{mapping::present, TransferProgressQueryError};
use crate::connectivity::ConnectivityError;
use crate::deps::{CurrentSpaceMemberScope, CurrentSpaceMemberScopePort};
use crate::transfer::progress::{
    ProgressProjectionKey, ProgressProjectionRegistry, ProgressQueryResult, ProgressReceiveError,
    ProgressSnapshotRead, TransferProgressReconciler,
};
use std::error::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{ProgressIdentity, ProgressItemState, TransferProgressView};
use uc_core::ids::{DeviceId, EntryId};
use uc_observability_contract::uc_debug;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferProgressReconciliation {
    Running,
    Settled,
    Superseded,
    NotRetained,
    Unsupported,
    Unreachable,
    Busy,
}

pub(crate) struct ProgressViewOwner {
    projections: ProgressProjectionRegistry,
    reconciliation: Arc<TransferProgressReconciler>,
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
}
impl ProgressViewOwner {
    pub(crate) fn new(
        projections: ProgressProjectionRegistry,
        reconciliation: Arc<TransferProgressReconciler>,
        scope: Arc<dyn CurrentSpaceMemberScopePort>,
    ) -> Self {
        Self {
            projections,
            reconciliation,
            scope,
        }
    }
    pub(crate) async fn get(
        &self,
        peer: DeviceId,
        source: &EntryId,
    ) -> Result<Option<TransferProgressView>, TransferProgressQueryError> {
        let key = key(peer, source)?;
        let scope = self.current_scope().await?;
        authorize(&scope, peer)?;
        Ok(self
            .projections
            .snapshot(key)
            .map(|snapshot| present(key, snapshot)))
    }
    pub(crate) async fn list(
        &self,
    ) -> Result<Vec<TransferProgressView>, TransferProgressQueryError> {
        let scope = self.current_scope().await?;
        if !scope.local_member_active {
            return Err(TransferProgressQueryError::AccessDenied(Box::new(
                NotCurrentMember,
            )));
        }
        Ok(self
            .projections
            .snapshots()
            .into_iter()
            .filter(|(key, _)| scope.usable_peer_device_ids.contains(&key.peer))
            .map(|(key, snapshot)| present(key, snapshot))
            .collect())
    }
    pub(crate) async fn reconcile(
        &self,
        peer: DeviceId,
        source: &EntryId,
        expected_attempt: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<TransferProgressReconciliation, TransferProgressQueryError> {
        let key = key(peer, source)?;
        authorize(&self.current_scope().await?, peer)?;
        let expected_attempt = expected_attempt
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|source| TransferProgressQueryError::InvalidInput(Box::new(source)))?;
        if self
            .projections
            .snapshot(key)
            .and_then(|snapshot| snapshot.task)
            .is_some_and(|task| {
                expected_attempt
                    .is_some_and(|expected| *expected.as_bytes() != task.attempt.bytes())
            })
        {
            return Ok(TransferProgressReconciliation::Superseded);
        }
        let outcome = self.reconciliation.refresh(key, cancel).await;
        // 网络等待后仍复核当前成员；撤销不能令旧事实重新可见。
        authorize(&self.current_scope().await?, peer)?;
        if let Ok(ProgressSnapshotRead::Complete(snapshot)) = outcome.as_ref() {
            if expected_attempt
                .is_some_and(|expected| *expected.as_bytes() != snapshot.descriptor.attempt.bytes())
            {
                return Ok(TransferProgressReconciliation::Superseded);
            }
        }
        classify(outcome)
    }
    async fn current_scope(&self) -> Result<CurrentSpaceMemberScope, TransferProgressQueryError> {
        self.scope.snapshot().await.map_err(|source| {
            let error = TransferProgressQueryError::from(source);
            uc_debug!(
                error_class = error.class(),
                error = &error as &dyn Error,
                "transfer progress membership read failed"
            );
            error
        })
    }
}
fn key(
    peer: DeviceId,
    source: &EntryId,
) -> Result<ProgressProjectionKey, TransferProgressQueryError> {
    let source = Uuid::parse_str(source.as_ref())
        .map_err(|source| TransferProgressQueryError::InvalidInput(Box::new(source)))?;
    Ok(ProgressProjectionKey {
        peer,
        source: ProgressIdentity::new(*source.as_bytes()),
    })
}
fn authorize(
    scope: &CurrentSpaceMemberScope,
    peer: DeviceId,
) -> Result<(), TransferProgressQueryError> {
    if !scope.local_member_active || !scope.usable_peer_device_ids.contains(&peer) {
        return Err(TransferProgressQueryError::AccessDenied(Box::new(
            NotCurrentMember,
        )));
    }
    Ok(())
}

pub(super) fn classify(
    outcome: ProgressQueryResult,
) -> Result<TransferProgressReconciliation, TransferProgressQueryError> {
    match outcome.as_ref() {
        Ok(ProgressSnapshotRead::Complete(snapshot)) => Ok(
            if snapshot
                .items
                .iter()
                .all(|item| matches!(item, ProgressItemState::Terminal(_)))
            {
                TransferProgressReconciliation::Settled
            } else {
                TransferProgressReconciliation::Running
            },
        ),
        Ok(ProgressSnapshotRead::Superseded(_)) => Ok(TransferProgressReconciliation::Superseded),
        Ok(ProgressSnapshotRead::NotRetained) | Err(ProgressReceiveError::UnknownTask) => {
            Ok(TransferProgressReconciliation::NotRetained)
        }
        Ok(ProgressSnapshotRead::Unsupported) | Err(ProgressReceiveError::Unsupported) => {
            Ok(TransferProgressReconciliation::Unsupported)
        }
        Ok(ProgressSnapshotRead::Busy) | Err(ProgressReceiveError::Busy) => {
            Ok(TransferProgressReconciliation::Busy)
        }
        Ok(ProgressSnapshotRead::AccessDenied) => Err(TransferProgressQueryError::AccessDenied(
            Box::new(RemoteScopeDenied),
        )),
        Err(ProgressReceiveError::Stale) => Ok(TransferProgressReconciliation::Superseded),
        Err(ProgressReceiveError::Connection(
            ConnectivityError::Offline | ConnectivityError::AddressMissing,
        )) => Ok(TransferProgressReconciliation::Unreachable),
        Err(source) => {
            let failure: Box<dyn Error + Send + Sync> =
                Box::new(SharedQueryFailure(outcome.clone()));
            let error = match source {
                ProgressReceiveError::AccessDenied(_)
                | ProgressReceiveError::Connection(ConnectivityError::Denied) => {
                    TransferProgressQueryError::AccessDenied(failure)
                }
                ProgressReceiveError::Malformed(_)
                | ProgressReceiveError::Snapshot(_)
                | ProgressReceiveError::Protocol
                | ProgressReceiveError::Conflict
                | ProgressReceiveError::InvalidPlan => {
                    TransferProgressQueryError::InvalidData(failure)
                }
                _ => TransferProgressQueryError::Unavailable(failure),
            };
            uc_debug!(
                error_class = error.class(),
                source_class = source.class(),
                error = &error as &dyn Error,
                "transfer progress reconciliation failed"
            );
            Err(error)
        }
    }
}
