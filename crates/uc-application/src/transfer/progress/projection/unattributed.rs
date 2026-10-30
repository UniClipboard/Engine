//! 无关联通知复用原意图与发布围栏，不从远端消息创建条目。
use super::{ProgressProjectionKey, ProgressProjectionRegistry};
use crate::transfer::progress::{ProgressReceiveError, ProgressUnattributedReceiverPort};
use async_trait::async_trait;
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;

#[async_trait]
impl ProgressUnattributedReceiverPort for ProgressProjectionRegistry {
    async fn observe_unattributed(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Result<(), ProgressReceiveError> {
        let key = ProgressProjectionKey {
            peer: *peer,
            source,
        };
        let receiver = self
            .receiver(key)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        receiver.observe_unattributed(peer, source).await?;
        if !self.publish(key, &receiver) {
            return Err(ProgressReceiveError::UnknownTask);
        }
        Ok(())
    }
}
