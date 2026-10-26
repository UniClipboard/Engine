//! 按本机发送意图核对完整远端事实；本机owner与协议source不必相同。
use super::owner::authorize;
use super::{ProgressViewOwner, TransferProgressQueryError, TransferProgressReconciliation};
use async_trait::async_trait;
use std::fmt;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{TransferProgressItemStatus, TransferProgressView};
use uc_core::ids::{DeviceId, EntryId};
use uuid::Uuid;

#[derive(Clone, PartialEq, Eq)]
pub struct SentEntryReconciliation {
    pub state: TransferProgressReconciliation,
    pub current: Option<TransferProgressView>,
}
impl fmt::Debug for SentEntryReconciliation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SentEntryReconciliation")
            .field("state", &self.state)
            .field("has_current", &self.current.is_some())
            .finish_non_exhaustive()
    }
}
#[async_trait]
pub(crate) trait ReconcileSentEntryPort: Send + Sync {
    async fn reconcile_sent_entry(
        &self,
        peer: DeviceId,
        owner: &EntryId,
    ) -> Result<SentEntryReconciliation, TransferProgressQueryError>;
}
#[derive(Debug, thiserror::Error)]
#[error("local entry has ambiguous retained outbound identities")]
struct AmbiguousIntent;

#[async_trait]
impl ReconcileSentEntryPort for ProgressViewOwner {
    async fn reconcile_sent_entry(
        &self,
        peer: DeviceId,
        owner: &EntryId,
    ) -> Result<SentEntryReconciliation, TransferProgressQueryError> {
        authorize(&self.current_scope().await?, peer)?;
        let keys = self.projections.owned_keys(peer, owner);
        let key = match keys.as_slice() {
            [] => {
                return Ok(SentEntryReconciliation {
                    state: TransferProgressReconciliation::NotRetained,
                    current: self.get(peer, owner).await?,
                })
            }
            [key] => *key,
            _ => {
                return Err(TransferProgressQueryError::InvalidData(Box::new(
                    AmbiguousIntent,
                )))
            }
        };
        let source = EntryId::from(Uuid::from_bytes(key.source.bytes()).to_string());
        let expected = self
            .projections
            .snapshot(key)
            .and_then(|snapshot| snapshot.task)
            .map(|task| Uuid::from_bytes(task.attempt.bytes()).to_string());
        let mut state = self
            .reconcile(
                peer,
                &source,
                expected.as_deref(),
                &CancellationToken::new(),
            )
            .await?;
        let current = self.get(peer, &source).await?;
        if matches!(
            state,
            TransferProgressReconciliation::Running | TransferProgressReconciliation::Settled
        ) {
            state = match current.as_ref() {
                None => TransferProgressReconciliation::NotRetained,
                Some(view)
                    if expected
                        .as_ref()
                        .is_some_and(|attempt| view.attempt.as_ref() != Some(attempt)) =>
                {
                    TransferProgressReconciliation::Superseded
                }
                Some(view)
                    if view.items.iter().all(|item| {
                        matches!(
                            item.status,
                            TransferProgressItemStatus::Completed
                                | TransferProgressItemStatus::Failed
                                | TransferProgressItemStatus::Cancelled
                        )
                    }) =>
                {
                    TransferProgressReconciliation::Settled
                }
                Some(_) => TransferProgressReconciliation::Running,
            };
        }
        Ok(SentEntryReconciliation { state, current })
    }
}
