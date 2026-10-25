//! 完整通知动作不能在本地权威读取之后重新取得另一会话的准入基线。
use super::{descriptor, item, progress_budget, terminal, Authority, MemoryPair};
use crate::network::iroh::transfer_progress_v2::TransferProgressWire;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectivityError, ProgressClaimRefusal, ProgressLiveRoundPort, ProgressReceiveError,
    ProgressSourceTask, ProgressTaskAuthorityPort, ProgressTerminalRoundPort,
    TransferProgressProducer,
};
use uc_core::file_transfer::{ProgressActive, ProgressIdentity, ProgressItem, ProgressPhase};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

struct HeldAuthority {
    authority: Authority,
    first: AtomicBool,
    entered: Semaphore,
    released: Mutex<bool>,
    changed: Condvar,
}
impl HeldAuthority {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
struct ReleaseRead(Arc<HeldAuthority>);
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        self.0.release();
    }
}
impl ProgressTaskAuthorityPort for HeldAuthority {
    fn current_task(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Option<ProgressSourceTask> {
        if self.first.swap(false, Ordering::SeqCst) {
            self.entered.add_permits(1);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.changed.wait(released).unwrap();
            }
        }
        self.authority.current_task(peer, source)
    }
    fn claim_terminal(
        &self,
        _: &DeviceId,
        _: ProgressIdentity,
        _: ProgressIdentity,
        _: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal> {
        panic!("complete producer round does not claim local results");
    }
}
#[derive(Clone, Copy, Debug)]
enum Operation {
    Register,
    Contact,
    Update,
    Terminal,
}
pub(super) fn stopped(error: &ConnectivityError) -> bool {
    match error {
        ConnectivityError::Stopped => true,
        ConnectivityError::Shared(error) => stopped(error),
        _ => false,
    }
}
async fn old_action_keeps_its_original_session(operation: Operation) -> anyhow::Result<()> {
    let pair = MemoryPair::new().await?;
    let authority = Arc::new(HeldAuthority {
        authority: Authority {
            peer: pair.sender_id,
        },
        first: AtomicBool::new(true),
        entered: Semaphore::new(0),
        released: Mutex::new(false),
        changed: Condvar::new(),
    });
    let _release = ReleaseRead(authority.clone());
    let producer = TransferProgressProducer::new(
        pair.receiver.clone(),
        Arc::new(TransferProgressWire),
        authority.clone(),
        Arc::new(SystemElapsedClock),
    );
    let target = pair.sender_id;
    let sending = tokio::spawn(async move {
        let cancel = CancellationToken::new();
        let deadline = progress_budget();
        match operation {
            Operation::Register => producer
                .register_current(&target, descriptor().source, &cancel)
                .await
                .map(|_| ()),
            Operation::Contact => {
                producer
                    .report_contact(&target, descriptor(), 1, &deadline, &cancel)
                    .await
            }
            Operation::Update => {
                producer
                    .report_live(
                        &target,
                        descriptor(),
                        ProgressActive {
                            item: item(1),
                            round: 0,
                            sequence: 1,
                            bytes: 8,
                            total: Some(16),
                            phase: ProgressPhase::Transferring,
                        },
                        &deadline,
                        &cancel,
                    )
                    .await
            }
            Operation::Terminal => {
                producer
                    .report_settled(&target, descriptor(), terminal(0), &deadline, &cancel)
                    .await
            }
        }
    });
    timeout(Duration::from_secs(2), authority.entered.acquire())
        .await??
        .forget();
    // 原动作已进入本地读取；重建会话不能为它重取新请求，即使peer和内容描述都相同。
    pair.receiver.disconnect_all().await?;
    authority.release();
    let result = timeout(Duration::from_secs(8), sending).await??;
    let dials = pair.receiver_dials.load(Ordering::SeqCst);
    pair.close().await?;
    assert_eq!(
        dials, 0,
        "{operation:?} obtained a new session after its local read"
    );
    assert!(matches!(result, Err(ProgressReceiveError::Connection(error)) if stopped(&error)));
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registration_cannot_gain_a_new_session_after_an_inflight_local_authority_read(
) -> anyhow::Result<()> {
    old_action_keeps_its_original_session(Operation::Register).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn contact_cannot_gain_a_new_session_after_an_inflight_local_authority_read(
) -> anyhow::Result<()> {
    old_action_keeps_its_original_session(Operation::Contact).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn update_cannot_gain_a_new_session_after_an_inflight_local_authority_read(
) -> anyhow::Result<()> {
    old_action_keeps_its_original_session(Operation::Update).await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_cannot_gain_a_new_session_after_an_inflight_local_authority_read(
) -> anyhow::Result<()> {
    old_action_keeps_its_original_session(Operation::Terminal).await
}
