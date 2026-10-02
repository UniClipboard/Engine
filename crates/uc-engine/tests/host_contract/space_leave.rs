//! 离开空间（`Operation::FactoryResetSpace`）之后实例本身必须可继续使用：
//! 成功结果意味着同一个 Engine 可以查询空空间、再次创建或加入空间，宿主不需要重启。

use std::sync::atomic::Ordering;
use std::time::Duration;

use uc_engine::error_codes::{
    FACTORY_RESET_KEY_MATERIAL_FAILED_CODE, FACTORY_RESET_RESTART_REQUIRED_CODE,
};
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineEvent, HistoryEntryInput, Operation,
    OperationResult, ProfileRecoveryState, SecretString, SendTextInput,
};

use super::{startup::host, MemorySecureStorage};

const PASSPHRASE: &str = "space-leave-test-passphrase";

fn create_space(device_name: &str) -> Operation {
    Operation::CreateSpace(CreateSpaceInput {
        device_name: Some(device_name.into()),
        passphrase: SecretString::new(PASSPHRASE),
        passphrase_confirmation: SecretString::new(PASSPHRASE),
    })
}

async fn local_device_id(engine: &Engine) -> String {
    let OperationResult::LocalDevice(device) =
        engine.execute(Operation::QueryLocalDevice).await.unwrap()
    else {
        panic!("expected local device")
    };
    device.device_id
}

async fn has_completed_space(engine: &Engine) -> bool {
    let OperationResult::SetupState(setup) =
        engine.execute(Operation::QuerySetupState).await.unwrap()
    else {
        panic!("expected setup state")
    };
    setup.has_completed
}

async fn leave(engine: &Engine) {
    assert_eq!(
        engine.execute(Operation::FactoryResetSpace).await.unwrap(),
        OperationResult::SpaceFactoryReset
    );
}

/// Mobile 的真实顺序：离开 -> 刷新空间状态 -> 打开加入页读取设备组选择。
async fn assert_empty_space_is_queryable(engine: &Engine) {
    assert!(!has_completed_space(engine).await);
    let OperationResult::DeviceGroupChoices(_) = engine
        .execute(Operation::QueryDeviceGroupChoices)
        .await
        .expect("an empty space must answer device group choices")
    else {
        panic!("expected device group choices")
    };
    let OperationResult::Devices(devices) = engine.execute(Operation::ListDevices).await.unwrap()
    else {
        panic!("expected device list")
    };
    assert!(
        devices.is_empty(),
        "leaving must not retain the member roster"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leaving_a_space_keeps_the_same_instance_usable() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, mut events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();

    engine.execute(create_space("first space")).await.unwrap();
    let first_identity = local_device_id(&engine).await;
    assert!(has_completed_space(&engine).await);

    leave(&engine).await;
    assert_empty_space_is_queryable(&engine).await;

    // 同一实例可以直接再次创建空间，并得到全新的本机身份。
    engine.execute(create_space("second space")).await.unwrap();
    assert!(has_completed_space(&engine).await);
    let second_identity = local_device_id(&engine).await;
    assert_ne!(first_identity, second_identity);
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "written after leaving".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved entry")
    };
    let OperationResult::HistoryEntry(entry) = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: sent.entry_id,
        }))
        .await
        .unwrap()
    else {
        panic!("expected history entry")
    };
    assert_eq!(entry.content, "written after leaving");

    // 重复离开：第二次、第三次仍然成功，并且每次都回到可查询的空状态。
    leave(&engine).await;
    assert_empty_space_is_queryable(&engine).await;
    leave(&engine).await;
    assert_empty_space_is_queryable(&engine).await;

    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    while events.next().await.is_some() {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hosts_that_restart_after_leaving_keep_working() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine.execute(create_space("restart host")).await.unwrap();
    leave(&engine).await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (restarted, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    assert_empty_space_is_queryable(&restarted).await;
    restarted
        .execute(create_space("after restart"))
        .await
        .unwrap();
    restarted.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_leave_keeps_its_progress_and_the_same_instance_can_finish_it() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine.execute(create_space("failing leave")).await.unwrap();

    storage.fail_deletes.store(true, Ordering::SeqCst);
    let failed = engine
        .execute(Operation::FactoryResetSpace)
        .await
        .expect_err("a leave that could not wipe its keys must not report success");
    assert_eq!(failed.code(), FACTORY_RESET_KEY_MATERIAL_FAILED_CODE);
    // 重置已持久地开始，业务操作在完成前统一不可用，而不是半个空间。
    assert_eq!(
        engine
            .execute(Operation::QuerySetupState)
            .await
            .unwrap_err()
            .code(),
        1103
    );

    storage.fail_deletes.store(false, Ordering::SeqCst);
    leave(&engine).await;
    assert_empty_space_is_queryable(&engine).await;
    engine
        .execute(create_space("after failed leave"))
        .await
        .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_leave_whose_fresh_runtime_cannot_start_reports_restart_required() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, mut events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(create_space("blocked rebuild"))
        .await
        .unwrap();

    // 宿主把临时目录换成一个文件：重置可以完成，新运行期无法创建它的导入目录。
    let temporary = root.path().join("temporary");
    std::fs::remove_dir_all(&temporary).unwrap();
    std::fs::write(&temporary, b"blocked").unwrap();

    let error = engine
        .execute(Operation::FactoryResetSpace)
        .await
        .expect_err("an unusable instance must not be reported as a successful leave");
    assert_eq!(error.code(), FACTORY_RESET_RESTART_REQUIRED_CODE);
    assert!(!error.is_retryable());

    // 之后的状态是明确的：恢复状态要求重启，业务操作返回同一个结构化错误。
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(summary.restart_required);
    for operation in [Operation::QuerySetupState, Operation::FactoryResetSpace] {
        assert_eq!(
            engine.execute(operation).await.unwrap_err().code(),
            FACTORY_RESET_RESTART_REQUIRED_CODE
        );
    }
    let mut saw_restart_required = false;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);
    while let Some(event) = events.next().await {
        if let EngineEvent::ProfileRecoveryChanged(summary) = event {
            saw_restart_required |= summary.restart_required;
        }
    }
    assert!(saw_restart_required);

    // 宿主按提示重启，同一份资料作为全新安装启动。
    std::fs::remove_file(&temporary).unwrap();
    std::fs::create_dir_all(&temporary).unwrap();
    let (restarted, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    assert_empty_space_is_queryable(&restarted).await;
    restarted
        .execute(create_space("after restart"))
        .await
        .unwrap();
    restarted.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lifecycle_requests_after_leaving_reach_the_fresh_runtime() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine.execute(create_space("lifecycle")).await.unwrap();
    leave(&engine).await;

    engine.suspend().await.unwrap();
    engine.resume().await.unwrap();
    assert_empty_space_is_queryable(&engine).await;
    engine.execute(create_space("resumed")).await.unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

/// 反复离开不得累积旧运行期的任务与句柄（网络端点、数据库连接、监听任务）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_leaves_do_not_accumulate_tasks_or_descriptors() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();

    let mut samples = Vec::new();
    for round in 0..6 {
        engine
            .execute(create_space(&format!("leak round {round}")))
            .await
            .unwrap();
        leave(&engine).await;
        // 让被取消的任务有机会退出后再取样。
        tokio::time::sleep(Duration::from_millis(500)).await;
        samples.push(resource_sample());
    }
    // 取样本身有 ±1 的抖动；每轮泄漏哪怕一个任务或句柄，后三轮也会比前三轮多出至少两个。
    let (early, late) = samples.split_at(3);
    let early_tasks = early.iter().map(|sample| sample.tasks).min().unwrap();
    let late_tasks = late.iter().map(|sample| sample.tasks).max().unwrap();
    let early_descriptors = early.iter().map(|sample| sample.descriptors).min().unwrap();
    let late_descriptors = late.iter().map(|sample| sample.descriptors).max().unwrap();
    assert!(
        late_tasks <= early_tasks + 1,
        "alive tasks grew across leave cycles: {samples:?}"
    );
    assert!(
        late_descriptors <= early_descriptors + 1,
        "open descriptors grew across leave cycles: {samples:?}"
    );
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[derive(Debug, Clone, Copy)]
struct ResourceSample {
    tasks: usize,
    descriptors: usize,
}

fn resource_sample() -> ResourceSample {
    ResourceSample {
        tasks: tokio::runtime::Handle::current()
            .metrics()
            .num_alive_tasks(),
        descriptors: std::fs::read_dir("/dev/fd")
            .map(|entries| entries.count())
            .unwrap_or(0),
    }
}
