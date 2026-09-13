#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileUpgradeVersions {
    pub product: String,
    pub engine: String,
}

pub struct ProfileUpgradeSource {
    pub has_data: bool,
    pub source_product: Option<String>,
    pub source_engine: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileUpgradeBackupEntry {
    pub id: String,
    pub created_at_ms: u64,
    pub source_product: Option<String>,
    pub source_engine: Option<String>,
    pub target_product: String,
    pub target_engine: String,
    pub size_bytes: u64,
}
