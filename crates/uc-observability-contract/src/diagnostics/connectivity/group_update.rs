//! 成员更新完成记录的安全本地细节；所有输出都是固定分类。
#[derive(Clone, Copy)]
pub enum GroupUpdatePhase {
    DecodeUpdate,
    ValidateUpdate,
    LoadState,
    ApplySecurityUpdate,
    PersistState,
    InstallSecurityState,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub enum GroupUpdateReason {
    Unavailable,
    Locked,
    NotFound,
    Conflict,
    Constraint,
    Corrupt,
    PermissionDenied,
    UnsupportedVersion,
    Unknown,
}
#[derive(Clone, Copy)]
pub enum GroupUpdateSource {
    Storage,
    Security,
    Decoder,
    Io,
    State,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct GroupUpdateFailureDetail {
    pub phase: GroupUpdatePhase,
    pub reason: GroupUpdateReason,
    pub source: GroupUpdateSource,
}

impl GroupUpdatePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DecodeUpdate => "decode_update",
            Self::ValidateUpdate => "validate_update",
            Self::LoadState => "load_state",
            Self::ApplySecurityUpdate => "apply_security_update",
            Self::PersistState => "persist_state",
            Self::InstallSecurityState => "install_security_state",
            Self::Unknown => "unknown",
        }
    }
}
impl GroupUpdateReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Locked => "locked",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Constraint => "constraint",
            Self::Corrupt => "corrupt",
            Self::PermissionDenied => "permission_denied",
            Self::UnsupportedVersion => "unsupported_version",
            Self::Unknown => "unknown",
        }
    }
}
impl GroupUpdateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Storage => "storage",
            Self::Security => "security",
            Self::Decoder => "decoder",
            Self::Io => "io",
            Self::State => "state",
            Self::Unknown => "unknown",
        }
    }
}

/// 存储层在真实错误转换处（而非诊断读取处）附加的固定分类。
///
/// 持久化实现（例如 SQLite/Diesel）在这里把具体库错误类型翻译成
/// [`GroupUpdateReason`]，并保留原错误作为 `source`。读取方（security、p2p）
/// 只需识别这个类型即可得到 `GroupUpdateSource::Storage` + 对应 reason，
/// 不需要也不允许直接认识任何存储库的错误类型。
#[derive(Debug)]
pub struct ClassifiedGroupUpdateStorageFailure {
    pub reason: GroupUpdateReason,
    source: anyhow::Error,
}

impl ClassifiedGroupUpdateStorageFailure {
    pub fn new(reason: GroupUpdateReason, source: impl Into<anyhow::Error>) -> Self {
        Self {
            reason,
            source: source.into(),
        }
    }
}

impl std::fmt::Display for ClassifiedGroupUpdateStorageFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("classified storage failure")
    }
}

impl std::error::Error for ClassifiedGroupUpdateStorageFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
