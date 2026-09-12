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
