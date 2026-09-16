use crate::space::membership::{QueryDeviceTrustError, QueryMembershipConflictsError};

#[derive(Debug, thiserror::Error)]
pub enum QueryDeviceGroupChoicesError {
    #[error("查询设备信任状态失败")]
    DeviceTrust {
        #[source]
        source: QueryDeviceTrustError,
    },
    #[error("查询成员分支冲突失败")]
    MembershipConflict {
        #[source]
        source: QueryMembershipConflictsError,
    },
}
