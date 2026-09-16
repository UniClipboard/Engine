use async_trait::async_trait;

use crate::space::membership::{DeviceTrustStatus, QueryDeviceTrustError};

#[async_trait]
pub(crate) trait QueryMembershipConflictStatusPort: Send + Sync {
    async fn query_status(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError>;
}
