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
#[derive(Clone, Copy)]
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
