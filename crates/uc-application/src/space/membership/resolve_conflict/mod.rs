mod error;
mod model;
mod ports;
mod use_case;

pub use error::ResolveMembershipConflictError;
pub use model::{ResolveMembershipConflictInput, ResolveMembershipConflictResult};
pub(crate) use ports::QueryMembershipConflictStatusPort;
pub(crate) use use_case::ResolveMembershipConflictUseCase;

use async_trait::async_trait;

use crate::space::membership::{DeviceTrustStatus, QueryDeviceTrustError, QueryDeviceTrustUseCase};

#[async_trait]
impl QueryMembershipConflictStatusPort for QueryDeviceTrustUseCase {
    async fn query_status(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        self.execute().await
    }
}

#[cfg(test)]
mod tests;
