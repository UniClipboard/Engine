use async_trait::async_trait;

use super::{ProfileKeyAccessProbe, ProfileKeyAccessProbePortError};

#[async_trait]
pub trait ProbeProfileKeyAccessPort: Send + Sync {
    async fn probe_profile_key_access(
        &self,
    ) -> Result<ProfileKeyAccessProbe, ProfileKeyAccessProbePortError>;
}
