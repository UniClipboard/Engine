use serde::{Deserialize, Serialize};

/// 已知来源记录；未知字段不冒充当前版本，归档层不把字段当作签名验证结果。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBackupSource {
    pub product_version: Option<String>,
    pub engine_version: Option<String>,
    pub platform: String,
    pub architecture: String,
    pub installation_channel: Option<String>,
    pub artifact_digest: Option<[u8; 32]>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileArchiveReceipt {
    pub archive_id: [u8; 16],
    pub source: ProfileBackupSource,
    pub archive_digest: [u8; 32],
}
