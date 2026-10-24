//! 在真实登记端口的状态读取与写入之间提交另一事务，验证写资格与密文结果。
use super::{DieselFileTransferRepository, PendingInboundTransfer, RecordReceiverTransferPort};
use crate::db::connection::{is_lock_contention, sqlite_failure_class};
use crate::db::pool::init_db_pool;
use crate::db::ports::DbExecutor;
use crate::db::schema::file_transfer;
use crate::file_transfer::persistence_cipher::test_keys;
use diesel::connection::{InstrumentationEvent, SimpleConnection};
use diesel::dsl::sql;
use diesel::prelude::*;
use diesel::r2d2::{ConnectionManager, PooledConnection};
use diesel::select;
use diesel::sql_types::Integer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

type PooledDatabase = PooledConnection<ConnectionManager<SqliteConnection>>;
struct Database(Mutex<PooledDatabase>);
impl DbExecutor for Database {
    fn run<T>(
        &self,
        work: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        work(&mut self.0.lock().unwrap())
    }
}

#[tokio::test]
async fn pending_registration_survives_an_unrelated_concurrent_commit() {
    for existing in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("registration.sqlite");
        let pool = init_db_pool(path.to_str().unwrap()).unwrap();
        let mut reader = pool.get().unwrap();
        reader
            .batch_execute(
                "CREATE TABLE registration_concurrency_probe (value INTEGER NOT NULL);
             INSERT INTO registration_concurrency_probe VALUES (0);",
            )
            .unwrap();
        let executor = Arc::new(Database(Mutex::new(reader)));
        let (keys, profile) = test_keys::ports();
        let repository = DieselFileTransferRepository::new(executor.clone(), keys, profile);
        let pending = PendingInboundTransfer {
            item_role: None,
            transfer_id: "registration".into(),
            entry_id: "entry".into(),
            attempt_id: None,
            origin_device_id: "peer".into(),
            filename: "PRIVATE_FILENAME".into(),
            file_size: Some(8 * 1024 * 1024),
            cached_path: "PRIVATE_PATH".into(),
            created_at_ms: 1,
        };
        if existing {
            repository.upsert_pending_transfer(&pending).await.unwrap();
        }
        let mut writer = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        // 竞争写方只尝试立即取得锁；不能在同步查询回调里等待持锁的登记操作。
        writer.batch_execute("PRAGMA busy_timeout = 0;").unwrap();
        let writer = Arc::new(Mutex::new(writer));
        let attempted = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(AtomicBool::new(false));
        let competing = writer.clone();
        let observed = attempted.clone();
        let wrote = committed.clone();
        executor.0.lock().unwrap().set_instrumentation(move |event: InstrumentationEvent<'_>| {
            if let InstrumentationEvent::FinishQuery { query, error: None, .. } = event {
                let query = query.to_string();
                if query.starts_with("SELECT") && query.contains("file_transfer")
                    && !observed.swap(true, Ordering::SeqCst)
                {
                    let mut writer = competing.lock().unwrap();
                    match writer.batch_execute("BEGIN IMMEDIATE;") {
                        Ok(()) => {
                            writer.batch_execute(
                                "UPDATE registration_concurrency_probe SET value = value + 1; COMMIT;"
                            ).unwrap();
                            wrote.store(true, Ordering::SeqCst);
                        }
                        Err(source) => assert!(is_lock_contention(&source)),
                    }
                }
            }
        });
        let result = repository.upsert_pending_transfer(&pending).await;
        assert!(
            attempted.load(Ordering::SeqCst),
            "必须实际在登记SELECT后尝试并发提交"
        );
        if !committed.load(Ordering::SeqCst) {
            writer.lock().unwrap().batch_execute(
                "BEGIN IMMEDIATE; UPDATE registration_concurrency_probe SET value = value + 1; COMMIT;"
            ).unwrap();
        }
        assert_eq!(
            result.map_err(|source| sqlite_failure_class(&source).unwrap_or("non_sqlite")),
            Ok(()),
            "无关写入不得使已接受的pending登记失败，existing={existing}"
        );
        let ciphertext = executor
            .run(|connection| {
                file_transfer::table
                    .filter(file_transfer::transfer_id.eq("registration"))
                    .select(file_transfer::metadata_ciphertext)
                    .first::<Vec<u8>>(connection)
                    .map_err(Into::into)
            })
            .unwrap();
        let metadata = repository
            .protection
            .resolve()
            .await
            .unwrap()
            .open_metadata("registration", &ciphertext)
            .await
            .unwrap();
        assert_eq!(metadata.filename, "PRIVATE_FILENAME");
        assert_eq!(metadata.cached_path.as_deref(), Some("PRIVATE_PATH"));
        assert!(!ciphertext
            .windows(b"PRIVATE_FILENAME".len())
            .any(|window| window == b"PRIVATE_FILENAME"));
        let value = select(sql::<Integer>(
            "(SELECT value FROM registration_concurrency_probe)",
        ))
        .get_result::<i32>(&mut *writer.lock().unwrap())
        .unwrap();
        assert_eq!(value, 1, "竞争写方也必须实际完成且只提交一次");
    }
}
