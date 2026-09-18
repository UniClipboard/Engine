//! Entry delivery 持久化实现。
//!
//! 为什么需要这个模块:
//! `EntryDeliveryRepositoryPort` 的契约要求"按 (entry_id, target_device_id)
//! upsert"与"按 entry 列出所有目标结果"。SQLite 用 `INSERT OR REPLACE`
//! 自然落地 upsert 语义,FK 由表定义保证(entry 删除时 CASCADE)。本文件
//! 把 wire 中性的 status 枚举与 SQL 字符串做双向映射,保持表结构稳定。

use crate::db::models::entry_delivery::{EntryDeliveryRow, NewEntryDeliveryRow};
use crate::db::ports::DbExecutor;
use crate::db::schema::clipboard_entry_delivery;
use async_trait::async_trait;
use diesel::query_dsl::methods::FilterDsl;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel::ExpressionMethods;
use diesel::RunQueryDsl;
use diesel::SqliteConnection;
use std::sync::Arc;
use tokio::task::spawn_blocking;
use tracing::{instrument, Span};
use uc_core::clipboard::{
    DeliveryFailureReason, EntryDeliveryError, EntryDeliveryRecord, EntryDeliveryStatus,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::EntryDeliveryRepositoryPort;

pub struct DieselEntryDeliveryRepository<E> {
    executor: Arc<E>,
}

impl<E> DieselEntryDeliveryRepository<E> {
    pub fn new(executor: E) -> Self {
        Self {
            executor: Arc::new(executor),
        }
    }
}

impl<E: DbExecutor + 'static> DieselEntryDeliveryRepository<E> {
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, EntryDeliveryError> {
        let executor = Arc::clone(&self.executor);
        let span = Span::current();
        spawn_blocking(move || span.in_scope(|| executor.run(operation)))
            .await
            .map_err(|source| EntryDeliveryError::Storage {
                source: source.into(),
            })?
            .map_err(translate_storage_error)
    }
}

/// 状态在持久化层的字符串编码。变体名保持稳定,不随上层重命名变动。
mod status_codec {
    use super::*;

    pub const PENDING: &str = "pending";
    pub const DELIVERED: &str = "delivered";
    pub const DUPLICATE: &str = "duplicate";
    pub const UNREACHABLE: &str = "unreachable";
    pub const SUPERSEDED: &str = "superseded";
    // Legacy alias: rows written before the Unreachable promotion decode as
    // Unreachable for seamless migration without a schema rewrite.
    const LEGACY_FAILED_OFFLINE: &str = "failed_offline";
    pub const FAILED_LOCAL_POLICY: &str = "failed_local_policy";
    pub const FAILED_PEER_REJECTED: &str = "failed_peer_rejected";
    pub const FAILED_PEER_INCOMPATIBLE: &str = "failed_peer_incompatible";
    pub const FAILED_IO: &str = "failed_io";
    pub const FAILED_INTERNAL: &str = "failed_internal";

    pub fn encode(status: &EntryDeliveryStatus) -> &'static str {
        match status {
            EntryDeliveryStatus::Pending => PENDING,
            EntryDeliveryStatus::Delivered => DELIVERED,
            EntryDeliveryStatus::Duplicate => DUPLICATE,
            EntryDeliveryStatus::Unreachable => UNREACHABLE,
            EntryDeliveryStatus::Superseded => SUPERSEDED,
            EntryDeliveryStatus::Failed { reason } => match reason {
                DeliveryFailureReason::LocalPolicy => FAILED_LOCAL_POLICY,
                DeliveryFailureReason::PeerRejected => FAILED_PEER_REJECTED,
                DeliveryFailureReason::PeerIncompatible => FAILED_PEER_INCOMPATIBLE,
                DeliveryFailureReason::Io => FAILED_IO,
                DeliveryFailureReason::Internal => FAILED_INTERNAL,
            },
        }
    }

    pub fn decode(raw: &str) -> Result<EntryDeliveryStatus, EntryDeliveryError> {
        match raw {
            PENDING => Ok(EntryDeliveryStatus::Pending),
            DELIVERED => Ok(EntryDeliveryStatus::Delivered),
            DUPLICATE => Ok(EntryDeliveryStatus::Duplicate),
            UNREACHABLE | LEGACY_FAILED_OFFLINE => Ok(EntryDeliveryStatus::Unreachable),
            SUPERSEDED => Ok(EntryDeliveryStatus::Superseded),
            FAILED_LOCAL_POLICY => Ok(EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::LocalPolicy,
            }),
            FAILED_PEER_REJECTED => Ok(EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::PeerRejected,
            }),
            FAILED_PEER_INCOMPATIBLE => Ok(EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::PeerIncompatible,
            }),
            FAILED_IO => Ok(EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::Io,
            }),
            FAILED_INTERNAL => Ok(EntryDeliveryStatus::Failed {
                reason: DeliveryFailureReason::Internal,
            }),
            _ => Err(EntryDeliveryError::InvalidStatus),
        }
    }
}

fn row_to_record(row: EntryDeliveryRow) -> Result<EntryDeliveryRecord, EntryDeliveryError> {
    Ok(EntryDeliveryRecord {
        entry_id: EntryId::from(row.entry_id),
        target_device_id: DeviceId::new(row.target_device_id),
        status: status_codec::decode(&row.status)?,
        reason_detail: row.reason_detail,
        updated_at_ms: row.updated_at_ms,
    })
}

#[async_trait]
impl<E> EntryDeliveryRepositoryPort for DieselEntryDeliveryRepository<E>
where
    E: DbExecutor + 'static,
{
    #[instrument(
        name = "infra.sqlite.upsert_entry_delivery",
        skip_all,
        fields(
            operation = "record_attempt",
            table = "clipboard_entry_delivery",
            entry_id = %record.entry_id,
            target_device_id = %record.target_device_id,
        )
    )]
    async fn record_attempt(&self, record: &EntryDeliveryRecord) -> Result<(), EntryDeliveryError> {
        let new_row = NewEntryDeliveryRow {
            entry_id: record.entry_id.to_string(),
            target_device_id: record.target_device_id.to_string(),
            status: status_codec::encode(&record.status).to_string(),
            reason_detail: record.reason_detail.clone(),
            updated_at_ms: record.updated_at_ms,
        };

        self.run(move |conn| {
            diesel::replace_into(clipboard_entry_delivery::table)
                .values(&new_row)
                .execute(conn)?;
            Ok(())
        })
        .await
    }

    #[instrument(
        name = "infra.sqlite.query_entry_delivery",
        skip_all,
        fields(
            operation = "list_by_entry",
            table = "clipboard_entry_delivery",
            entry_id = %entry_id,
        )
    )]
    async fn list_by_entry(
        &self,
        entry_id: &EntryId,
    ) -> Result<Vec<EntryDeliveryRecord>, EntryDeliveryError> {
        let entry_id_str = entry_id.to_string();
        let rows: Vec<EntryDeliveryRow> = self
            .run(move |conn| {
                Ok(clipboard_entry_delivery::table
                    .filter(clipboard_entry_delivery::entry_id.eq(&entry_id_str))
                    .load::<EntryDeliveryRow>(conn)?)
            })
            .await?;

        rows.into_iter().map(row_to_record).collect()
    }
}

/// 把底层错误翻译为领域错误。FK violation 反映"引用了不存在的 entry",
/// 其它一律按 Storage 归类。
fn translate_storage_error(source: anyhow::Error) -> EntryDeliveryError {
    if source.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<DieselError>(),
            Some(DieselError::DatabaseError(
                DatabaseErrorKind::ForeignKeyViolation,
                _
            ))
        )
    }) {
        EntryDeliveryError::EntryNotFound { source }
    } else {
        EntryDeliveryError::Storage { source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::models::{NewClipboardEntryRow, NewClipboardEventRow};
    use crate::db::pool::init_db_pool;
    use crate::db::ports::DbExecutor;
    use crate::db::schema::{clipboard_entry, clipboard_event};
    use diesel::connection::SimpleConnection;
    use std::error::Error as StdError;
    use std::io::{Error as IoError, ErrorKind};
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::{tempdir, TempDir};
    use tokio::sync::oneshot;
    use tokio::task::{spawn_blocking, JoinError};
    use tokio::time::timeout;

    type Repo = DieselEntryDeliveryRepository<DieselSqliteExecutor>;

    fn make_repo() -> (Repo, DieselSqliteExecutor, TempDir) {
        let tempdir = tempdir().unwrap();
        let path = tempdir.path().join("delivery-repo.sqlite");
        let path_str = path.to_str().unwrap();
        let pool_for_repo = init_db_pool(path_str).unwrap();
        let pool_for_seed = init_db_pool(path_str).unwrap();
        let repo = DieselEntryDeliveryRepository::new(DieselSqliteExecutor::new(pool_for_repo));
        (repo, DieselSqliteExecutor::new(pool_for_seed), tempdir)
    }

    fn seed_entry(executor: &DieselSqliteExecutor, entry_id: &str) {
        let event_id = format!("ev-{entry_id}");
        let event_row = NewClipboardEventRow {
            event_id: event_id.clone(),
            captured_at_ms: 1_700_000_000_000,
            source_device: "test-device".into(),
            snapshot_hash: format!("blake3v1:{entry_id}"),
        };
        let entry_row = NewClipboardEntryRow {
            entry_id: entry_id.to_string(),
            event_id,
            created_at_ms: 1_700_000_000_000,
            active_time_ms: 1_700_000_000_000,
            total_size: 0,
            pinned: false,
            delivery_tracked: true,
            is_favorited: false,
            content_category: "text".into(),
        };
        executor
            .run(move |conn| {
                diesel::insert_into(clipboard_event::table)
                    .values(&event_row)
                    .execute(conn)?;
                diesel::insert_into(clipboard_entry::table)
                    .values(&entry_row)
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();
    }

    fn make_record(
        entry_id: &str,
        target: &str,
        status: EntryDeliveryStatus,
    ) -> EntryDeliveryRecord {
        EntryDeliveryRecord {
            entry_id: EntryId::from(entry_id.to_string()),
            target_device_id: DeviceId::new(target.to_string()),
            status,
            reason_detail: None,
            updated_at_ms: 1_700_000_000_001,
        }
    }

    struct FailingExecutor {
        panics: bool,
    }

    impl DbExecutor for FailingExecutor {
        fn run<T>(
            &self,
            _: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
        ) -> anyhow::Result<T> {
            if self.panics {
                panic!("private executor payload");
            }
            Err(IoError::new(ErrorKind::PermissionDenied, "private FOREIGN KEY text").into())
        }
    }

    #[tokio::test]
    async fn storage_error_keeps_its_source_without_matching_private_text() {
        let repo = DieselEntryDeliveryRepository::new(FailingExecutor { panics: false });
        let error = repo
            .list_by_entry(&EntryId::from("entry"))
            .await
            .unwrap_err();
        assert!(StdError::source(&error).is_some());
        assert!(!format!("{error:?}").contains("private"));
        let EntryDeliveryError::Storage { source } = error else {
            panic!("must classify by the original type")
        };
        assert_eq!(
            source.downcast_ref::<IoError>().unwrap().kind(),
            ErrorKind::PermissionDenied
        );
    }

    #[tokio::test]
    async fn worker_panic_keeps_its_source_and_safe_summary() {
        let repo = DieselEntryDeliveryRepository::new(FailingExecutor { panics: true });
        let error = repo
            .list_by_entry(&EntryId::from("entry"))
            .await
            .unwrap_err();
        assert!(StdError::source(&error).is_some());
        assert!(!format!("{error:?}").contains("private"));
        let EntryDeliveryError::Storage { source } = error else {
            panic!("expected storage failure")
        };
        assert!(source.downcast_ref::<JoinError>().unwrap().is_panic());
    }

    #[tokio::test]
    async fn invalid_persisted_status_does_not_disclose_its_value() {
        let (repo, executor, _directory) = make_repo();
        seed_entry(&executor, "entry");
        repo.record_attempt(&make_record(
            "entry",
            "peer",
            EntryDeliveryStatus::Delivered,
        ))
        .await
        .unwrap();
        executor
            .run(|conn| {
                diesel::update(clipboard_entry_delivery::table)
                    .set(clipboard_entry_delivery::status.eq("private invalid status"))
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();
        let error = repo
            .list_by_entry(&EntryId::from("entry"))
            .await
            .unwrap_err();
        assert!(matches!(error, EntryDeliveryError::InvalidStatus));
        assert!(!format!("{error:?}").contains("private"));
    }

    #[tokio::test]
    async fn database_write_contention_does_not_block_runtime_notifications() {
        let (repo, seed, _directory) = make_repo();
        seed_entry(&seed, "held-entry");
        let (entered, blocked) = oneshot::channel();
        let (release, wait) = mpsc::channel();
        let holder = spawn_blocking(move || {
            seed.run(|conn| {
                conn.batch_execute("BEGIN IMMEDIATE")?;
                entered.send(()).unwrap();
                // 回归失败时也释放真实锁，避免测试遗留阻塞线程。
                let _ = wait.recv_timeout(Duration::from_secs(2));
                conn.batch_execute("COMMIT")?;
                Ok(())
            })
        });
        blocked.await.unwrap();
        let record = make_record("held-entry", "peer", EntryDeliveryStatus::Delivered);
        let mut write = Box::pin(repo.record_attempt(&record));
        let during_contention = timeout(Duration::from_millis(20), write.as_mut()).await;
        let _ = release.send(());
        holder.await.unwrap().unwrap();
        assert!(
            during_contention.is_err(),
            "runtime must remain responsive while the database waits"
        );
        write.await.unwrap();
        assert_eq!(
            repo.list_by_entry(&record.entry_id).await.unwrap(),
            vec![record]
        );
    }

    #[test]
    fn decode_legacy_failed_offline_as_unreachable() {
        let status = status_codec::decode("failed_offline").unwrap();
        assert!(matches!(status, EntryDeliveryStatus::Unreachable));
    }

    #[test]
    fn decode_new_unreachable_round_trips() {
        let encoded = status_codec::encode(&EntryDeliveryStatus::Unreachable);
        assert_eq!(encoded, "unreachable");
        let decoded = status_codec::decode(encoded).unwrap();
        assert!(matches!(decoded, EntryDeliveryStatus::Unreachable));
    }

    #[test]
    fn pending_delivery_round_trips() {
        let encoded = status_codec::encode(&EntryDeliveryStatus::Pending);
        assert_eq!(encoded, "pending");
        assert_eq!(
            status_codec::decode(encoded).unwrap(),
            EntryDeliveryStatus::Pending
        );
    }

    #[test]
    fn superseded_delivery_round_trips() {
        let encoded = status_codec::encode(&EntryDeliveryStatus::Superseded);
        assert_eq!(encoded, "superseded");
        assert_eq!(
            status_codec::decode(encoded).unwrap(),
            EntryDeliveryStatus::Superseded
        );
    }

    #[test]
    fn incompatible_delivery_failure_round_trips() {
        let status = EntryDeliveryStatus::Failed {
            reason: DeliveryFailureReason::PeerIncompatible,
        };
        let encoded = status_codec::encode(&status);
        assert_eq!(encoded, "failed_peer_incompatible");
        assert_eq!(status_codec::decode(encoded).unwrap(), status);
    }

    #[tokio::test]
    async fn record_attempt_inserts_new_row() {
        let (repo, seed_exec, _tempdir) = make_repo();
        seed_entry(&seed_exec, "entry-1");

        let rec = make_record("entry-1", "peer-A", EntryDeliveryStatus::Delivered);
        repo.record_attempt(&rec).await.expect("upsert ok");

        let listed = repo
            .list_by_entry(&EntryId::from("entry-1"))
            .await
            .expect("list ok");
        assert_eq!(listed.len(), 1);
        assert!(matches!(listed[0].status, EntryDeliveryStatus::Delivered));
        assert_eq!(listed[0].target_device_id.to_string(), "peer-A");
    }

    #[tokio::test]
    async fn record_attempt_upserts_existing_row() {
        let (repo, seed_exec, _tempdir) = make_repo();
        seed_entry(&seed_exec, "entry-1");

        repo.record_attempt(&make_record(
            "entry-1",
            "peer-A",
            EntryDeliveryStatus::Pending,
        ))
        .await
        .unwrap();
        repo.record_attempt(&make_record(
            "entry-1",
            "peer-A",
            EntryDeliveryStatus::Unreachable,
        ))
        .await
        .unwrap();

        let listed = repo.list_by_entry(&EntryId::from("entry-1")).await.unwrap();
        assert_eq!(listed.len(), 1, "upsert 不应增加行数");
        assert!(matches!(listed[0].status, EntryDeliveryStatus::Unreachable));
    }

    #[tokio::test]
    async fn list_by_entry_returns_all_targets() {
        let (repo, seed_exec, _tempdir) = make_repo();
        seed_entry(&seed_exec, "entry-1");

        for (peer, status) in [
            ("peer-A", EntryDeliveryStatus::Delivered),
            ("peer-B", EntryDeliveryStatus::Duplicate),
            (
                "peer-C",
                EntryDeliveryStatus::Failed {
                    reason: DeliveryFailureReason::Io,
                },
            ),
        ] {
            repo.record_attempt(&make_record("entry-1", peer, status))
                .await
                .unwrap();
        }

        let mut listed = repo.list_by_entry(&EntryId::from("entry-1")).await.unwrap();
        listed.sort_by(|a, b| {
            a.target_device_id
                .to_string()
                .cmp(&b.target_device_id.to_string())
        });
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].target_device_id.to_string(), "peer-A");
        assert_eq!(listed[2].target_device_id.to_string(), "peer-C");
    }

    #[tokio::test]
    async fn record_attempt_on_missing_entry_returns_entry_not_found() {
        let (repo, _seed_exec, _tempdir) = make_repo();
        let result = repo
            .record_attempt(&make_record(
                "ghost-entry",
                "peer-A",
                EntryDeliveryStatus::Delivered,
            ))
            .await;
        match result {
            Err(EntryDeliveryError::EntryNotFound { source }) => {
                assert!(source.chain().any(|cause| matches!(
                    cause.downcast_ref::<DieselError>(),
                    Some(DieselError::DatabaseError(
                        DatabaseErrorKind::ForeignKeyViolation,
                        _
                    ))
                )));
            }
            other => panic!("预期 EntryNotFound,实际 {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_by_entry_returns_empty_for_unknown() {
        let (repo, _seed_exec, _tempdir) = make_repo();
        let listed = repo
            .list_by_entry(&EntryId::from("never-existed"))
            .await
            .unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn fk_cascade_deletes_delivery_rows() {
        let (repo, seed_exec, _tempdir) = make_repo();
        seed_entry(&seed_exec, "entry-1");

        repo.record_attempt(&make_record(
            "entry-1",
            "peer-A",
            EntryDeliveryStatus::Delivered,
        ))
        .await
        .unwrap();

        // 删 entry,delivery 应被 CASCADE
        seed_exec
            .run(move |conn| {
                diesel::delete(clipboard_entry::table)
                    .filter(clipboard_entry::entry_id.eq("entry-1"))
                    .execute(conn)?;
                Ok(())
            })
            .unwrap();

        let listed = repo.list_by_entry(&EntryId::from("entry-1")).await.unwrap();
        assert!(listed.is_empty(), "FK CASCADE 应清理 delivery 行");
    }
}
