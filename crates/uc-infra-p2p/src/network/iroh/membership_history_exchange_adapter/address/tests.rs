use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use async_trait::async_trait;
use iroh::{endpoint::presets, Endpoint, RelayMode};
use tokio::sync::Semaphore;
use uc_application::deps::RefreshVerifiedPeerAddressPort;
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_infra_local::SystemClock;

use super::IrohVerifiedPeerAddressRefresh;

/// 读取在放行前阻塞，并记录读取次数与最大并发。
struct GatedRepository {
    gate: Semaphore,
    reads: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}
impl Default for GatedRepository {
    fn default() -> Self {
        Self {
            gate: Semaphore::new(0),
            reads: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        }
    }
}
#[async_trait]
impl PeerAddressRepositoryPort for GatedRepository {
    async fn get(&self, _: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        let permit = self.gate.acquire().await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        drop(permit);
        Ok(None)
    }
    async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(Vec::new())
    }
    async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
        Ok(())
    }
}

async fn refresh(repository: Arc<GatedRepository>) -> Arc<IrohVerifiedPeerAddressRefresh> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap();
    Arc::new(IrohVerifiedPeerAddressRefresh::new(
        Arc::new(endpoint),
        repository,
        Arc::new(SystemClock),
    ))
}

async fn wait_for_reads(repository: &GatedRepository, count: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while repository.reads.load(Ordering::SeqCst) < count {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("refresh reads the stored address");
}

/// 同一对端的刷新串行执行：运行期间的多次请求立即返回并合并为一次后续刷新。
#[tokio::test]
async fn refreshes_of_one_peer_run_one_at_a_time_and_coalesce_pending_requests() {
    let repository = Arc::new(GatedRepository::default());
    let owner = refresh(repository.clone()).await;
    let peer = DeviceId::new("peer-a");
    let first = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh_verified_peer_address(&peer).await }
    });
    wait_for_reads(&repository, 1).await;
    for _ in 0..3 {
        tokio::time::timeout(
            Duration::from_secs(1),
            owner.refresh_verified_peer_address(&peer),
        )
        .await
        .expect("a request during a running refresh returns at once");
    }
    repository.gate.add_permits(16);
    first.await.unwrap();
    assert_eq!(
        repository.reads.load(Ordering::SeqCst),
        2,
        "one rerun for all pending requests"
    );
    assert_eq!(repository.max_active.load(Ordering::SeqCst), 1);
}

/// 运行中的刷新被取消后释放该对端，下一次请求重新执行。
#[tokio::test]
async fn a_cancelled_refresh_does_not_block_later_requests() {
    let repository = Arc::new(GatedRepository::default());
    let owner = refresh(repository.clone()).await;
    let peer = DeviceId::new("peer-a");
    let cancelled = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh_verified_peer_address(&peer).await }
    });
    wait_for_reads(&repository, 1).await;
    cancelled.abort();
    let _ = cancelled.await;
    repository.gate.add_permits(16);
    owner.refresh_verified_peer_address(&peer).await;
    assert_eq!(repository.reads.load(Ordering::SeqCst), 2);
}
