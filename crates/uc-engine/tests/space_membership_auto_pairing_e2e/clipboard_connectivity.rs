//! 真实 Engine/SQLite/MLS 的在途剪贴板围栏，不新增生产测试开关。
use super::*;
use diesel::connection::SimpleConnection;
use diesel::{Connection, SqliteConnection};
use std::sync::atomic::AtomicBool;
use std::sync::Condvar;
use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::span::Attributes;
use tracing::Event;
use tracing::{Id, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, Registry};

struct ReceiveStarted {
    armed: Arc<AtomicBool>,
    started: Arc<Notify>,
}
impl<S: Subscriber + for<'lookup> LookupSpan<'lookup>> Layer<S> for ReceiveStarted {
    fn on_new_span(&self, attributes: &Attributes<'_>, id: &Id, context: Context<'_, S>) {
        let inbound = context.span(id).is_some_and(|span| {
            span.scope().any(|ancestor| {
                ancestor.metadata().name() == "handle_one"
                    && ancestor.metadata().target() == "uc_application::clipboard::inbound::runtime"
            })
        });
        if attributes.metadata().name() == "apply_inbound.execute"
            && attributes.metadata().target()
                == "uc_application::clipboard::sync::apply_inbound::usecase"
            && inbound
            && self.armed.swap(false, Ordering::SeqCst)
        {
            self.started.notify_one();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实 SQLite/MLS 在途验收，独占进程观测，使用 nextest 精确运行"]
async fn space_switch_ends_in_flight_clipboard_without_undoing_remote_commit() {
    in_flight_clipboard_cutover(Cutover::SenderSpace).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实 SQLite/MLS 在途验收，独占进程观测，使用 nextest 精确运行"]
async fn member_removal_ends_in_flight_clipboard_without_undoing_remote_commit() {
    in_flight_clipboard_cutover(Cutover::RemoveMember).await;
}

#[derive(Clone, Copy)]
enum Cutover {
    SenderSpace,
    RemoveMember,
}

async fn in_flight_clipboard_cutover(cutover: Cutover) {
    let _scenario = TestScenario::start();
    let armed = Arc::new(AtomicBool::new(false));
    let started = Arc::new(Notify::new());
    tracing::subscriber::set_global_default(Registry::default().with(ReceiveStarted {
        armed: armed.clone(),
        started: started.clone(),
    }))
    .expect("install per-process receive stage observer");
    let rendezvous = mount_rendezvous().await;
    let a_host = DeviceHarness::new(rendezvous.uri());
    let b_host = DeviceHarness::new(rendezvous.uri());
    let c_host = DeviceHarness::new(rendezvous.uri());
    let a = Arc::new(a_host.start().await);
    let b = Arc::new(b_host.start().await);
    let c = c_host.start().await;
    let old_space = create_space(&a, "A").await.0;
    let b_id = join_through(&a, &b, "B", &old_space).await.self_device_id;
    let new_space = create_space(&c, "C").await.0;
    tokio::join!(
        wait_for_active_member_count(&a, 2),
        wait_for_active_member_count(&b, 2)
    );
    wait_for_peer_refresh(&a, "before controlled clipboard send").await;
    // 对照：同一真实接收链可持久接受，然后才阻塞 SQLite 写入。
    a.execute(Operation::SendText(SendTextInput {
        text: "clipboard cutover control".into(),
        target_devices: vec![b_id.clone()],
    }))
    .await
    .unwrap();
    wait_for_received_text(&b, "clipboard cutover control").await;
    let changing = a.clone();
    let endpoint_before = query_endpoint_id(&changing, "before clipboard cutover").await;
    let generations = b_host.root.path().join("private/profile-data-generations");
    let databases: Vec<_> = std::fs::read_dir(generations)
        .unwrap()
        .map(|entry| entry.unwrap().path().join("v3-payloads/profile.sqlite"))
        .filter(|path| path.is_file())
        .collect();
    assert_eq!(
        databases.len(),
        1,
        "fresh host has one active profile database"
    );
    let database = &databases[0];
    let mut lock = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    lock.batch_execute("BEGIN IMMEDIATE").unwrap();
    let invitation = if !matches!(cutover, Cutover::RemoveMember) {
        Some(issue_invitation(&c).await)
    } else {
        None
    };
    armed.store(true, Ordering::SeqCst);
    let sending_engine = a.clone();
    let sending_peer = b_id.clone();
    let send_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let send = tokio::spawn(async move {
        sending_engine
            .execute(Operation::SendText(SendTextInput {
                text: "clipboard durable receive across cancellation".into(),
                target_devices: vec![sending_peer],
            }))
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), started.notified())
        .await
        .expect("remote application must begin a real persistent receive");
    assert!(
        !send.is_finished(),
        "SQLite write lock must prevent an early acceptance ACK"
    );
    let join = tokio::time::timeout_at(send_deadline, async {
        if let Some(invitation_code) = invitation {
            let joining = changing.clone();
            let action = tokio::spawn(async move {
                joining
                    .execute(Operation::JoinSpace(JoinSpaceInput {
                        invitation_code,
                        device_name: Some("A".into()),
                        passphrase: SecretString::new(PASSPHRASE),
                        preserve_unreadable_history: false,
                    }))
                    .await
            });
            let OperationResult::JoinSpace(status) = action.await.unwrap().unwrap() else {
                panic!("expected join status");
            };
            Some(status)
        } else {
            a.execute(Operation::RemoveMember(RemoveMemberInput {
                device_id: b_id.clone(),
            }))
            .await
            .unwrap();
            None
        }
    })
    .await
    .expect("cutover must begin within the original send budget");
    let outcome = tokio::time::timeout_at(send_deadline, send)
        .await
        .expect(
            "retirement/revocation must end the in-flight send before the original fan-out timeout",
        )
        .unwrap();
    match outcome {
        Err(_) => {}
        Ok(OperationResult::EntrySent(report)) => assert_eq!(report.total_accepted, 0),
        Ok(_) => panic!("unexpected send result"),
    }
    lock.batch_execute("ROLLBACK").unwrap();
    // 已经交接给旧接收会话的持久工作可以完成；失去发送 ACK 不撤销远端提交。
    wait_for_received_text(&b, "clipboard durable receive across cancellation").await;
    if let Some(status) = join {
        let joined =
            wait_for_completed_join(&changing, "changing device", status, &new_space).await;
        c.execute(Operation::SendText(SendTextInput {
            text: "clipboard after space retirement".into(),
            target_devices: vec![joined.self_device_id],
        }))
        .await
        .unwrap();
        wait_for_received_text(&changing, "clipboard after space retirement").await;
    }
    assert_eq!(
        query_endpoint_id(&changing, "after clipboard cutover").await,
        endpoint_before
    );
    let uc_engine::DevOperationResult::SessionHandoverDiagnostics {
        network_build_count,
        ..
    } = changing
        .execute_dev(uc_engine::DevOperation::QuerySessionHandoverDiagnostics)
        .await
        .unwrap()
    else {
        panic!("expected session diagnostics");
    };
    assert_eq!(network_build_count, 1);
    for engine in [a.as_ref(), b.as_ref(), &c] {
        engine.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    }
}

struct BlockedSystemWrite {
    started: Arc<Notify>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    block_once: AtomicBool,
}
impl HostClipboard for BlockedSystemWrite {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: vec![],
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        if self.block_once.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            let (lock, changed) = self.gate.as_ref();
            let mut released = lock.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
        }
        Ok(())
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(None)
    }
}
// 即使断言失败，也释放宿主阻塞线程，避免测试进程无法退出。
struct WriteRelease(Arc<(Mutex<bool>, Condvar)>);
impl Drop for WriteRelease {
    fn drop(&mut self) {
        let (lock, changed) = self.0.as_ref();
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
}
struct StopApplicationStarted {
    started: Arc<Notify>,
    armed: Arc<AtomicBool>,
}
#[derive(Default)]
struct StopApplicationEvent {
    started: bool,
    application: bool,
}
impl Visit for StopApplicationEvent {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.started |= field.name() == "event.name" && value == "runtime.work.started";
        self.application |= field.name() == "payload" && value.contains("session_stop_application");
    }
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}
impl<S: Subscriber> Layer<S> for StopApplicationStarted {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut record = StopApplicationEvent::default();
        event.record(&mut record);
        if record.started && record.application && self.armed.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
#[ignore = "真实持久接收续工作验收，独占进程观测，使用 nextest 精确运行"]
async fn receiver_space_switch_drains_old_persistent_work_before_new_session() {
    let _scenario = TestScenario::start();
    let stopping = Arc::new(Notify::new());
    let drain_armed = Arc::new(AtomicBool::new(false));
    tracing::subscriber::set_global_default(Registry::default().with(StopApplicationStarted {
        started: stopping.clone(),
        armed: drain_armed.clone(),
    }))
    .unwrap();
    let rendezvous = mount_rendezvous().await;
    let a_host = DeviceHarness::new(rendezvous.uri());
    let b_host = DeviceHarness::new(rendezvous.uri());
    let c_host = DeviceHarness::new(rendezvous.uri());
    let write_started = Arc::new(Notify::new());
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _release_on_exit = WriteRelease(gate.clone());
    let a = a_host.start().await;
    let b = Arc::new(
        b_host
            .start_with_clipboard(Box::new(BlockedSystemWrite {
                started: write_started.clone(),
                gate: gate.clone(),
                block_once: AtomicBool::new(true),
            }))
            .await,
    );
    let c = c_host.start().await;
    let old_space = create_space(&a, "A").await.0;
    let b_id = join_through(&a, &b, "B", &old_space).await.self_device_id;
    let new_space = create_space(&c, "C").await.0;
    tokio::join!(
        wait_for_active_member_count(&a, 2),
        wait_for_active_member_count(&b, 2)
    );
    wait_for_peer_refresh(&a, "before blocked old system write").await;
    let endpoint_before = query_endpoint_id(&b, "before receiver cutover").await;
    let result = a
        .execute(Operation::SendText(SendTextInput {
            text: "clipboard persisted before receiver space switch".into(),
            target_devices: vec![b_id],
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(report) = result else {
        panic!("expected delivery report")
    };
    assert_eq!(
        report.total_accepted, 1,
        "ACK must not wait for the blocked system write"
    );
    tokio::time::timeout(WAIT_TIMEOUT, write_started.notified())
        .await
        .unwrap();
    wait_for_received_text(&b, "clipboard persisted before receiver space switch").await;
    let invitation_code = issue_invitation(&c).await;
    drain_armed.store(true, Ordering::SeqCst);
    let changing = b.clone();
    let mut join = tokio::spawn(async move {
        let OperationResult::JoinSpace(status) = changing
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code,
                device_name: Some("B".into()),
                passphrase: SecretString::new(PASSPHRASE),
                preserve_unreadable_history: false,
            }))
            .await
            .unwrap()
        else {
            panic!("expected join status")
        };
        wait_for_completed_join(&changing, "B", status, &new_space).await
    });
    tokio::time::timeout(WAIT_TIMEOUT, stopping.notified())
        .await
        .expect("actual old Application shutdown must begin");
    assert!(
        tokio::time::timeout(Duration::from_millis(250), &mut join)
            .await
            .is_err(),
        "new session must keep waiting for old owned continuation"
    );
    {
        let (lock, changed) = gate.as_ref();
        *lock.lock().unwrap() = true;
        changed.notify_all();
    }
    let joined = tokio::time::timeout(WAIT_TIMEOUT, join)
        .await
        .unwrap()
        .unwrap();
    c.execute(Operation::SendText(SendTextInput {
        text: "clipboard after receiver drain".into(),
        target_devices: vec![joined.self_device_id],
    }))
    .await
    .unwrap();
    wait_for_received_text(&b, "clipboard after receiver drain").await;
    assert_eq!(
        query_endpoint_id(&b, "after receiver drain").await,
        endpoint_before
    );
    let uc_engine::DevOperationResult::SessionHandoverDiagnostics {
        network_build_count,
        ..
    } = b
        .execute_dev(uc_engine::DevOperation::QuerySessionHandoverDiagnostics)
        .await
        .unwrap()
    else {
        panic!("session diagnostics")
    };
    assert_eq!(network_build_count, 1);
    for engine in [&a, b.as_ref(), &c] {
        engine.shutdown(SHUTDOWN_TIMEOUT).await.unwrap();
    }
}
