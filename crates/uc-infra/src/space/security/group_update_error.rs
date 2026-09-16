//! 成员更新的失败动作上下文；固定动作名称不包含输入数据。
use uc_core::membership::KeyEpochError;

#[derive(Clone, Copy)]
pub(super) enum GroupUpdateAction {
    DecodeUpdate,
    LoadState,
    ApplySecurityUpdate,
    ValidateUpdate,
    PersistState,
    InstallSecurityState,
}

impl std::fmt::Display for GroupUpdateAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::DecodeUpdate => "decode_update",
            Self::LoadState => "load_state",
            Self::ApplySecurityUpdate => "apply_security_update",
            Self::ValidateUpdate => "validate_update",
            Self::PersistState => "persist_state",
            Self::InstallSecurityState => "install_security_state",
        })
    }
}

#[derive(thiserror::Error)]
#[error("group update {action} failed")]
pub(super) struct GroupUpdateFailure {
    pub(super) action: GroupUpdateAction,
    #[source]
    source: anyhow::Error,
}

impl std::fmt::Debug for GroupUpdateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, formatter)
    }
}

pub(super) fn failed_action(
    action: GroupUpdateAction,
    source: impl Into<anyhow::Error>,
) -> KeyEpochError {
    KeyEpochError::Repository(
        GroupUpdateFailure {
            action,
            source: source.into(),
        }
        .into(),
    )
}

pub(super) fn tag_action(action: GroupUpdateAction, error: KeyEpochError) -> KeyEpochError {
    // 保留既有业务分类；无 source 的规则拒绝不人为构造依赖异常。
    match error {
        KeyEpochError::Repository(source) => failed_action(action, source),
        KeyEpochError::SecurityState { source } => KeyEpochError::SecurityState {
            source: GroupUpdateFailure { action, source }.into(),
        },
        other => other,
    }
}

pub(crate) fn group_update_failure_detail(
    error: &KeyEpochError,
) -> uc_observability_contract::diagnostics::connectivity::GroupUpdateFailureDetail {
    use uc_core::membership::KeyEpochStateIssue as State;
    use uc_observability_contract::diagnostics::connectivity::{
        GroupUpdateFailureDetail, GroupUpdatePhase as Phase, GroupUpdateReason as Reason,
        GroupUpdateSource as Source,
    };
    let mut detail = GroupUpdateFailureDetail {
        phase: Phase::Unknown,
        reason: Reason::Unknown,
        source: Source::Unknown,
    };
    if let KeyEpochError::StateIssue(state) = error {
        detail.source = Source::State;
        (detail.phase, detail.reason) = match state {
            State::MissingMaterial | State::MissingRevocation | State::MissingStage => {
                (Phase::LoadState, Reason::NotFound)
            }
            State::CorruptMaterial => (Phase::ValidateUpdate, Reason::Corrupt),
            State::UnsupportedUpdate => (Phase::ValidateUpdate, Reason::UnsupportedVersion),
            State::OutOfOrderUpdate | State::StateChanged | State::InvalidStage => {
                (Phase::ValidateUpdate, Reason::Conflict)
            }
            State::EpochMismatch => (Phase::ApplySecurityUpdate, Reason::Conflict),
            _ => (Phase::Unknown, Reason::Unknown),
        };
    }
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = current {
        if let Some(context) = error.downcast_ref::<GroupUpdateFailure>() {
            if matches!(detail.phase, Phase::Unknown) {
                detail.phase = match context.action {
                    GroupUpdateAction::DecodeUpdate => Phase::DecodeUpdate,
                    GroupUpdateAction::ValidateUpdate => Phase::ValidateUpdate,
                    GroupUpdateAction::LoadState => Phase::LoadState,
                    GroupUpdateAction::ApplySecurityUpdate => Phase::ApplySecurityUpdate,
                    GroupUpdateAction::PersistState => Phase::PersistState,
                    GroupUpdateAction::InstallSecurityState => Phase::InstallSecurityState,
                };
            }
        } else if let Some(error) = error.downcast_ref::<std::io::Error>() {
            detail.source = Source::Io;
            detail.reason = match error.kind() {
                std::io::ErrorKind::PermissionDenied => Reason::PermissionDenied,
                std::io::ErrorKind::NotFound => Reason::NotFound,
                std::io::ErrorKind::AlreadyExists => Reason::Conflict,
                std::io::ErrorKind::InvalidData => Reason::Corrupt,
                std::io::ErrorKind::InvalidInput => Reason::Constraint,
                std::io::ErrorKind::WouldBlock
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::TimedOut => Reason::Unavailable,
                _ => Reason::Unknown,
            };
        } else if error.is::<serde_json::Error>() {
            detail.source = Source::Decoder;
            detail.reason = Reason::Corrupt;
        } else if let Some(error) = error.downcast_ref::<uc_core::crypto::EncryptionError>() {
            detail.source = Source::Security;
            detail.reason = match error {
                uc_core::crypto::EncryptionError::Locked => Reason::Locked,
                uc_core::crypto::EncryptionError::NotInitialized => Reason::Unavailable,
                uc_core::crypto::EncryptionError::CorruptedBlob
                | uc_core::crypto::EncryptionError::CorruptedKeySlot => Reason::Corrupt,
                _ => Reason::Unknown,
            };
        } else if let Some(error) = error.downcast_ref::<diesel::result::Error>() {
            use diesel::result::{DatabaseErrorKind as Kind, Error};
            detail.source = Source::Storage;
            detail.reason = match error {
                Error::NotFound => Reason::NotFound,
                Error::DatabaseError(
                    Kind::UniqueViolation
                    | Kind::ForeignKeyViolation
                    | Kind::NotNullViolation
                    | Kind::CheckViolation,
                    _,
                ) => Reason::Constraint,
                Error::DatabaseError(Kind::SerializationFailure, _) => Reason::Conflict,
                Error::DatabaseError(Kind::ReadOnlyTransaction, _) => Reason::PermissionDenied,
                Error::DatabaseError(Kind::ClosedConnection | Kind::UnableToSendCommand, _) => {
                    Reason::Unavailable
                }
                _ => Reason::Unknown,
            };
        }
        current = error.source();
    }
    detail
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_sqlite_failure_keeps_the_persist_stage_and_safe_source_category() {
        use diesel::{Connection, RunQueryDsl};
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(":memory:").expect("sqlite");
        let source = diesel::sql_query("INSERT INTO PRIVATE_MISSING_TABLE VALUES (1)")
            .execute(&mut connection)
            .expect_err("missing table");
        let error = failed_action(GroupUpdateAction::PersistState, source);
        let detail = group_update_failure_detail(&error);
        assert_eq!(detail.phase.as_str(), "persist_state");
        assert_eq!(detail.source.as_str(), "storage");
        assert_eq!(
            detail.reason.as_str(),
            "unknown",
            "不能从 SQLite 原始错误正文推测错误码"
        );
        assert!(!error.to_string().contains("PRIVATE_"));
        assert!(!format!("{error:?}").contains("PRIVATE_"));
    }
}
