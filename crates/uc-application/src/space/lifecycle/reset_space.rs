use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use uc_core::membership::{
    DeviceManagementResetDataPort, MemberRepositoryPort, RelationshipStateResetPort,
    SpaceSecurityStateResetPort,
};
use uc_core::ports::space::RebindSpaceSessionPort;
use uc_core::ports::{
    AppVersionStatePort, DeviceIdentityPort, LocalIdentityPort, SettingsPort, SetupStatusPort,
};

// use uc_core::membership::{}
//
//

pub(crate) struct ResetSpaceUsecaseDeps {
    pub(crate) legacy_isolation_required:bool,
    pub(crate) adopt_space: Arc<dyn RebindSpaceSessionPort>,
    pub(crate) device_management_reset: Arc<dyn DeviceManagementResetDataPort>,
    pub(crate) relationship_reset: Arc<dyn RelationshipStateResetPort>,
    pub(crate) security_reset: Arc<dyn SpaceSecurityStateResetPort>,
    pub(crate) setup_status: Arc<dyn SetupStatusPort>,
    pub(crate) local_identity: Arc<dyn LocalIdentityPort>,
    pub(crate) device_identity: Arc<dyn DeviceIdentityPort>,
}

/// 用户请求重置当前 Space 后，系统创建或恢复一个只包含本机设备的新 Space
/// 清除旧成员关系和旧空间的安全状态
pub(crate) struct ResetSpaceUseCase {

    pub(crate) async fn execute(&self) -> Result<(), ResetSpaceError> {
    
    }
}
