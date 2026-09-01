//! 最终 V3 profile/control generation 的 promotion 前验证边界。
//!
//! 验证按最终 repository 路由重新打开两库，不复用转换连接；只有完整性、外键、
//! 业务 row 唯一归属与 schema fingerprint 都稳定时才允许 journal 进入 Verified。

use std::path::Path;

use diesel::{Connection as _, RunQueryDsl as _};

use super::journal::UpgradeJournalV1;
use super::target::TargetGenerationStager;
use super::ProfileStorageUpgradeError;

pub(super) struct RuntimeGenerationValidator;

pub(super) struct VerifiedRuntimeGeneration {
    pub(super) profile_schema_digest: [u8; 32],
    pub(super) control_schema_digest: [u8; 32],
}

impl RuntimeGenerationValidator {
    pub(super) const fn new() -> Self {
        Self
    }

    pub(super) fn validate(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<VerifiedRuntimeGeneration, ProfileStorageUpgradeError> {
        target.verify_separated(journal)?;
        let paths = target.paths(journal);
        let profile = paths.payload_output.join("profile.sqlite");
        validate_database(&profile)?;
        validate_database(&paths.control_database)?;
        target.verify_runtime_row_ownership(journal)?;
        target.verify_source_revision(journal)?;
        Ok(VerifiedRuntimeGeneration {
            profile_schema_digest: schema_digest(&profile)?,
            control_schema_digest: schema_digest(&paths.control_database)?,
        })
    }

    pub(super) fn verify(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<(), ProfileStorageUpgradeError> {
        let verified = self.validate(journal, target)?;
        if journal.verified_profile_schema_digest() != Some(verified.profile_schema_digest)
            || journal.verified_control_schema_digest() != Some(verified.control_schema_digest)
        {
            return Err(corrupt(anyhow::anyhow!(
                "verified runtime schema fingerprint mismatch"
            )));
        }
        Ok(())
    }
}

#[derive(diesel::QueryableByName)]
struct IntegrityRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    integrity_check: String,
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct SchemaRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    object_type: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    sql: String,
}

fn validate_database(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    let integrity = diesel::sql_query("PRAGMA integrity_check")
        .get_result::<IntegrityRow>(&mut connection)
        .map_err(database_corrupt)?;
    if integrity.integrity_check != "ok" {
        return Err(corrupt(anyhow::anyhow!(
            "runtime generation SQLite integrity check failed"
        )));
    }
    let foreign_key_violations =
        diesel::sql_query("SELECT COUNT(*) AS count FROM pragma_foreign_key_check")
            .get_result::<CountRow>(&mut connection)
            .map_err(database_corrupt)?;
    if foreign_key_violations.count != 0 {
        return Err(corrupt(anyhow::anyhow!(
            "runtime generation contains foreign-key violations"
        )));
    }
    Ok(())
}

fn schema_digest(path: &Path) -> Result<[u8; 32], ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    let rows = diesel::sql_query(
        "SELECT type AS object_type, name, sql FROM sqlite_master \
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )
    .load::<SchemaRow>(&mut connection)
    .map_err(database_corrupt)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"uniclipboard/runtime-generation-schema/v1\0");
    for row in rows {
        for value in [
            row.object_type.as_bytes(),
            row.name.as_bytes(),
            row.sql.as_bytes(),
        ] {
            hasher.update(&(value.len() as u64).to_be_bytes());
            hasher.update(value);
        }
    }
    Ok(*hasher.finalize().as_bytes())
}

fn open_connection(
    path: &Path,
) -> Result<diesel::sqlite::SqliteConnection, ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| corrupt(anyhow::anyhow!("runtime generation path is invalid")))?;
    diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
        corrupt(anyhow::Error::new(source).context("open final runtime generation"))
    })
}

fn database_corrupt(source: diesel::result::Error) -> ProfileStorageUpgradeError {
    corrupt(anyhow::Error::new(source).context("validate final runtime generation"))
}

fn corrupt(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Corrupt { source }
}

#[cfg(test)]
mod tests {
    use super::{schema_digest, validate_database};
    use crate::db::pool::init_db_pool;

    #[test]
    fn final_database_validation_rejects_invalid_sqlite() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("profile.sqlite");
        drop(init_db_pool(database.to_str().unwrap()).unwrap());
        std::fs::write(&database, b"not a sqlite database").unwrap();

        assert!(validate_database(&database).is_err());
    }

    #[test]
    fn schema_fingerprint_is_stable_across_connections() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("profile.sqlite");
        drop(init_db_pool(database.to_str().unwrap()).unwrap());

        let first = schema_digest(&database).unwrap();
        let second = schema_digest(&database).unwrap();

        assert_eq!(first, second);
    }
}
