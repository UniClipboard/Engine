mod delivery;
mod encrypted_payload;
mod legacy_bootstrap;
mod revocation;
mod space_material;

#[cfg(feature = "test-util")]
mod benchmark;
#[cfg(feature = "test-util")]
pub use benchmark::GroupUpdateDeliveryBenchmark;

#[cfg(test)]
mod tests;

use uc_core::membership::KeyEpochError;

use uc_infra_security::InMemorySession;

#[derive(Clone)]
pub struct DieselSpaceSecurityStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselSpaceSecurityStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

/// Diesel 是这条 repository 唯一真实接触数据库错误类型的位置；在这里把它翻译
/// 成诊断合同的固定分类（见 `ClassifiedGroupUpdateStorageFailure`），往上
/// 只暴露分类结果，不让 security/p2p 的诊断读取反过来认识 Diesel 类型。
fn backend(error: impl Into<anyhow::Error>) -> KeyEpochError {
    let error = error.into();
    match error.downcast::<diesel::result::Error>() {
        Ok(diesel_error) => KeyEpochError::Repository(
            uc_observability_contract::diagnostics::connectivity::ClassifiedGroupUpdateStorageFailure::new(
                classify_diesel_error(&diesel_error),
                diesel_error,
            )
            .into(),
        ),
        Err(error) => KeyEpochError::Repository(error),
    }
}

fn classify_diesel_error(
    error: &diesel::result::Error,
) -> uc_observability_contract::diagnostics::connectivity::GroupUpdateReason {
    use diesel::result::{DatabaseErrorKind as Kind, Error};
    use uc_observability_contract::diagnostics::connectivity::GroupUpdateReason as Reason;
    match error {
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
    }
}

/// 事务闭包内返回的 `KeyEpochError` 保持原分类；其余下层失败按存储失败保留来源。
fn transaction_failure(error: anyhow::Error) -> KeyEpochError {
    match error.downcast::<KeyEpochError>() {
        Ok(error) => error,
        Err(error) => backend(error),
    }
}

fn epoch_to_i64(epoch: u64) -> Result<i64, KeyEpochError> {
    i64::try_from(epoch).map_err(backend)
}

#[cfg(test)]
mod failure_contract_tests {
    use super::*;
    #[test]
    fn backend_failure_retains_its_source_without_public_private_text() {
        let error = backend(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "PRIVATE_STORAGE_PATH",
        ));
        let source = std::error::Error::source(&error).expect("真实存储错误不能被字符串化");
        let source = source
            .downcast_ref::<std::io::Error>()
            .expect("原始 I/O 来源");
        assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(!error.to_string().contains("PRIVATE_STORAGE_PATH"));
        assert!(!format!("{error:?}").contains("PRIVATE_STORAGE_PATH"));
    }

    #[test]
    fn a_real_sqlite_failure_is_classified_at_the_conversion_site_without_a_table_name_leak() {
        use diesel::{Connection, RunQueryDsl};
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(":memory:").expect("sqlite");
        let source = diesel::sql_query("INSERT INTO PRIVATE_MISSING_TABLE VALUES (1)")
            .execute(&mut connection)
            .expect_err("missing table");

        let error = backend(source);

        let classified = std::error::Error::source(&error)
            .and_then(|source| {
                source.downcast_ref::<uc_observability_contract::diagnostics::connectivity::ClassifiedGroupUpdateStorageFailure>()
            })
            .expect("a diesel error must be wrapped as a classified storage failure");
        assert_eq!(
            classified.reason.as_str(),
            "unknown",
            "不能从 SQLite 原始错误正文推测错误码"
        );
        assert!(!error.to_string().contains("PRIVATE_"));
        assert!(!format!("{error:?}").contains("PRIVATE_"));
    }
}
