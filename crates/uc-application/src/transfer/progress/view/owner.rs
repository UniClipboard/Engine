use super::error::{NotCurrentMember, RemoteScopeDenied, SharedQueryFailure};
use super::{mapping::present, TransferProgressQueryError};
use crate::connectivity::ConnectivityError;
use crate::deps::{CurrentSpaceMemberScope, CurrentSpaceMemberScopePort};
use crate::transfer::progress::{
    ProgressProjectionKey, ProgressProjectionRegistry, ProgressQueryResult, ProgressReceiveError,
    ProgressSnapshotRead, ReadProgressHistoryPort, TransferProgressReconciler,
};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItemState, TransferProgressView, PROJECTION_GLOBAL_CAPACITY,
    PROJECTION_PEER_CAPACITY,
};
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
    pub(super) projections: ProgressProjectionRegistry,
    reconciliation: Arc<TransferProgressReconciler>,
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub(super) history: Arc<dyn ReadProgressHistoryPort>,
    pub(super) local: DeviceId,
}
impl ProgressViewOwner {
    pub(crate) fn new(
        projections: ProgressProjectionRegistry,
        reconciliation: Arc<TransferProgressReconciler>,
        scope: Arc<dyn CurrentSpaceMemberScopePort>,
        history: Arc<dyn ReadProgressHistoryPort>,
        local: DeviceId,
    ) -> Self {
        Self {
            projections,
            reconciliation,
            scope,
            history,
            local,
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
        if let Some(snapshot) = self.projections.snapshot(key) {
            return Ok(Some(present(key, snapshot)));
        }
        self.historical(key, source).await
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
        let snapshots = self.projections.snapshots();
        let runtime_keys = snapshots
            .iter()
            .map(|(key, _)| *key)
            .collect::<HashSet<_>>();
        let mut views = snapshots
            .into_iter()
            .filter(|(key, _)| scope.usable_peer_device_ids.contains(&key.peer))
            .map(|(key, snapshot)| present(key, snapshot))
            .collect::<Vec<_>>();
        let mut fences = views
            .iter()
            .map(|view| self.projections.begin_intent(view.source_entry.clone()))
            .collect::<Vec<_>>();
        let owned = self
            .history
            .list_owned(self.local, &scope.usable_peer_device_ids)
            .await
            .map_err(|source| TransferProgressQueryError::Unavailable(Box::new(source)))?;
        let mut per_peer = HashMap::<DeviceId, usize>::new();
        for view in &views {
            *per_peer.entry(view.peer).or_default() += 1;
        }
        for (peer, entry) in owned {
            if views.len() >= PROJECTION_GLOBAL_CAPACITY {
                break;
            }
            if per_peer.get(&peer).copied().unwrap_or_default() >= PROJECTION_PEER_CAPACITY
                || views
                    .iter()
                    .any(|view| view.peer == peer && view.source_entry == entry)
            {
                continue;
            }
            let fence = self.projections.begin_intent(entry.clone());
            if let Some(view) = self.get(peer, &entry).await? {
                views.push(view);
                fences.push(fence);
                *per_peer.entry(peer).or_default() += 1;
            }
        }
        let current = self.current_scope().await?;
        if !current.local_member_active {
            return Err(TransferProgressQueryError::AccessDenied(Box::new(
                NotCurrentMember,
            )));
        }
        let mut current_views = Vec::with_capacity(views.len());
        for (view, fence) in views.into_iter().zip(fences) {
            if !current.usable_peer_device_ids.contains(&view.peer)
                || !fence.permits_history(view.peer)
            {
                continue;
            }
            let key = key(view.peer, &view.source_entry)?;
            if runtime_keys.contains(&key) {
                // 历史读取会等待存储；已删除或淘汰的实时范围不能由先前快照复活。
                if let Some(snapshot) = self.projections.snapshot(key) {
                    current_views.push(present(key, snapshot));
                }
            } else {
                current_views.push(view);
            }
        }
        Ok(current_views)
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
    pub(super) async fn current_scope(
        &self,
    ) -> Result<CurrentSpaceMemberScope, TransferProgressQueryError> {
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
pub(super) fn authorize(
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
