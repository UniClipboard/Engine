use async_trait::async_trait;

#[async_trait]
pub(crate) trait PendingSpaceInvitationResetPort: Send + Sync {
    async fn cancel_all(&self) -> usize;
}
