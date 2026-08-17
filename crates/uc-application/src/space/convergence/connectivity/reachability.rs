//! Current-member reachability refresh.
//!
//! Slice 2 Phase 1 · T6 — `EnsureReachableAllUseCase`.
//!
//! F1 钩子:`SpaceFacade::auto_start_network` 在 `start_network` 成功
//! 之后调用本 usecase,对所有已配对设备并发拨号,填满
//! `IrohPresenceAdapter` 的状态缓存,让后续 `MemberRosterFacade::list_with_presence`
//! 可以直接从缓存读 online/offline 而不再触发拨号。
//!
//! ## 迭代源:`peer_addr_repo.list()`(而不是 `member_repo`)
//!
//! Slice 2 Phase 1 的 T5 已经让 pairing 两端把对方的 transport address blob
//! 落入 `PeerAddressRepositoryPort`;只有出现在 repo 里的设备才能被
//! `IrohPresenceAdapter` 拨号成功。`member_repo` 会多出"身份记录在,
//! 但没地址 blob"的异常条目(例如用户从 Phase 0 Slice 1 升级过来,老记录
//! 没有 blob),对这些设备调 `ensure_reachable` 必然返回
//! `PresenceError::NoAddress`——相当于凭空制造失败报告。迭代 `peer_addr_repo`
//! 天然跳过这些并让数据一致。
//!
//! ## 并发策略
//!
//! `tokio::task::JoinSet`,每个 paired device 一个 task,各自独立 `await`
//! `presence.ensure_reachable`。单个 task 失败(拨号超时、NoAddress、
//! adapter internal)只写进 `report.errors`,不影响其他 task。
//!
//! `task_plan.md:842` 已锁定 N ≤ 10 假设,全员并发不做限流;N > 10
//! 的资源放大属于 T-05(P3),Slice 2 Phase 1 不处理。
//!
//! ## 跳过本机和已移除设备
//!
//! 按 T5 语义 `peer_addr_repo` 只写对端地址,不写本机——repo 里应永远
//! 不包含本机 DeviceId。依然在 usecase 里加一层防御性过滤,用
//! `DeviceIdentityPort::current_device_id()` 对比:万一未来有代码误把
//! 本机地址写进 repo,也不会 self-dial。成员移除已在本机生效的设备同样
//! 不能被重新拨号；重新准入的新成员实例不受旧移除记录影响。

use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::{debug, info, instrument, warn};

use uc_core::ids::DeviceId;
use uc_core::membership::CurrentWorkspacePeerScopePort;
use uc_core::ports::{
    DeviceIdentityPort, PeerAddressRepositoryPort, PresenceError, PresencePort, ReachabilityState,
};

/// Result of a single `execute` pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnsureReachableAllReport {
    /// Total peers iterated (after self-filter).
    pub total: usize,
    /// Peers whose `ensure_reachable` returned `Online`.
    pub online: usize,
    /// Peers whose `ensure_reachable` returned `Offline` or `Unknown`.
    pub offline: usize,
    /// Per-device failures (`NoAddress`, adapter `Internal`, …). These are
    /// not counted in `online`/`offline` — surfacing them separately lets
    /// callers distinguish "reachable and offline" from "probe itself
    /// malfunctioned".
    pub errors: Vec<(DeviceId, String)>,
}

/// Fatal errors: infrastructure-level failures that abort the whole pass.
/// Per-peer probe failures are **not** here — they land in `report.errors`.
#[derive(Debug, thiserror::Error)]
pub enum EnsureReachableAllError {
    #[error("failed to list peer addresses: {0}")]
    Repository(String),
}

pub(crate) struct EnsureReachableAllUseCase {
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    presence: Arc<dyn PresencePort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
}

impl EnsureReachableAllUseCase {
    pub(crate) fn new(
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        presence: Arc<dyn PresencePort>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
    ) -> Self {
        Self {
            peer_addr_repo,
            presence,
            device_identity,
            peer_scope,
        }
    }

    #[instrument(skip_all)]
    pub(crate) async fn execute(
        &self,
    ) -> Result<EnsureReachableAllReport, EnsureReachableAllError> {
        let records = self.peer_addr_repo.list().await.map_err(|err| {
            EnsureReachableAllError::Repository(format!("peer_addr_repo.list: {err}"))
        })?;
        let scope = self.peer_scope.snapshot().await.map_err(|error| {
            EnsureReachableAllError::Repository(format!("current peer scope: {error:?}"))
        })?;

        let local = self.device_identity.current_device_id();
        let mut targets = Vec::with_capacity(records.len());
        for record in records {
            if record.device_id == local {
                // Defensive: T5 never writes self, but guard against
                // future bugs that could cause a self-dial loop.
                debug!(
                    device_id = %record.device_id.as_str(),
                    "peer_addr_repo returned local device; skipping self-dial"
                );
                continue;
            }
            if !scope.peer_device_ids.contains(&record.device_id) {
                continue;
            }
            targets.push(record.device_id);
        }

        if targets.is_empty() {
            info!("ensure_reachable_all: no paired peers; report is empty");
            return Ok(EnsureReachableAllReport::default());
        }

        let mut set: JoinSet<(DeviceId, Result<ReachabilityState, PresenceError>)> = JoinSet::new();
        for device_id in &targets {
            let presence = Arc::clone(&self.presence);
            let device_id = *device_id;
            set.spawn(async move {
                // 用 verify_reachable 而非 ensure_reachable：probe 场景需要
                // 绕过 IrohPresenceAdapter 的 fast-path，对已有 alive 连接
                // 的 peer 也强制重新拨号。否则对端断网期间 fast-path 会持续
                // 返回 Online，UI 要等 ~60s QUIC max_idle_timeout watchdog
                // 才能切灰。F1 hook 路径下 peers map 为空，verify_reachable
                // 与 ensure_reachable 行为等价。
                let result = presence.verify_reachable(&device_id).await;
                (device_id, result)
            });
        }

        let mut report = EnsureReachableAllReport {
            total: targets.len(),
            ..Default::default()
        };

        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((device_id, Ok(ReachabilityState::Online))) => {
                    debug!(device_id = %device_id.as_str(), "ensure_reachable → Online");
                    report.online += 1;
                }
                Ok((device_id, Ok(ReachabilityState::Offline)))
                | Ok((device_id, Ok(ReachabilityState::Unknown))) => {
                    debug!(device_id = %device_id.as_str(), "ensure_reachable → Offline/Unknown");
                    report.offline += 1;
                }
                Ok((device_id, Err(err))) => {
                    warn!(
                        device_id = %device_id.as_str(),
                        error = %err,
                        "ensure_reachable returned error"
                    );
                    report.errors.push((device_id, err.to_string()));
                }
                Err(join_err) => {
                    // A spawned task panicked; `ensure_reachable` should
                    // not panic in practice, but surface it so the caller
                    // sees the anomaly instead of silently losing a peer.
                    warn!(error = %join_err, "ensure_reachable task panicked/cancelled");
                }
            }
        }

        info!(
            total = report.total,
            online = report.online,
            offline = report.offline,
            errors = report.errors.len(),
            "ensure_reachable_all completed"
        );

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests use mockall-generated mocks (project convention since
    //! Slice 2 T5 — see `uc-application/usecases/pairing/redeem_invitation.rs`).
    //! Each test asserts one concurrency/failure-isolation property of the
    //! usecase.
    use super::*;
    use async_trait::async_trait;
    use chrono::{TimeZone, Utc};
    use std::time::Duration;
    use tokio::sync::broadcast;
    use uc_core::ids::DeviceId;
    use uc_core::ports::{PeerAddressError, PeerAddressRecord};

    mockall::mock! {
        pub PeerAddrRepo {}

        #[async_trait]
        impl PeerAddressRepositoryPort for PeerAddrRepo {
            async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError>;
            async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError>;
            async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError>;
            async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError>;
        }
    }

    mockall::mock! {
        pub Presence {}

        #[async_trait]
        impl PresencePort for Presence {
            async fn ensure_reachable(
                &self,
                device: &DeviceId,
            ) -> Result<ReachabilityState, PresenceError>;
            async fn current_state(&self, device: &DeviceId) -> ReachabilityState;
            fn subscribe(&self) -> broadcast::Receiver<uc_core::ports::PresenceEvent>;
        }
    }

    struct FixedDevice(DeviceId);
    impl DeviceIdentityPort for FixedDevice {
        fn current_device_id(&self) -> DeviceId {
            self.0.clone()
        }
    }

    struct AllTestPeers;

    #[async_trait]
    impl CurrentWorkspacePeerScopePort for AllTestPeers {
        async fn snapshot(
            &self,
        ) -> Result<
            uc_core::membership::CurrentWorkspacePeerSnapshot,
            uc_core::membership::CurrentWorkspacePeerScopeError,
        > {
            Ok(uc_core::membership::CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: uc_core::membership::CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: [
                    "peer-a",
                    "peer-b",
                    "peer-c",
                    "peer-ok-1",
                    "peer-err",
                    "peer-ok-2",
                ]
                .into_iter()
                .map(DeviceId::new)
                .collect(),
            })
        }
    }

    struct RetainedPeerOnly;

    #[async_trait]
    impl CurrentWorkspacePeerScopePort for RetainedPeerOnly {
        async fn snapshot(
            &self,
        ) -> Result<
            uc_core::membership::CurrentWorkspacePeerSnapshot,
            uc_core::membership::CurrentWorkspacePeerScopeError,
        > {
            Ok(uc_core::membership::CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: uc_core::membership::CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: vec![DeviceId::new("retained")],
            })
        }
    }

    fn record(device: &str, blob: &[u8]) -> PeerAddressRecord {
        PeerAddressRecord {
            device_id: DeviceId::new(device),
            addr_blob: blob.to_vec(),
            observed_at: Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap(),
        }
    }

    #[tokio::test]
    async fn empty_repo_returns_empty_report_without_touching_presence() {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| Ok(vec![]));
        // presence mock has zero expectations; any call would panic on drop.
        let presence = MockPresence::new();
        let local = DeviceId::new("local-device");

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(local)),
            Arc::new(AllTestPeers),
        );

        let report = uc.execute().await.expect("ok");
        assert_eq!(report, EnsureReachableAllReport::default());
    }

    #[tokio::test]
    async fn repository_failure_surfaces_as_fatal_error() {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list()
            .times(1)
            .returning(|| Err(PeerAddressError::Internal("sqlite down".into())));
        let presence = MockPresence::new();
        let local = DeviceId::new("local-device");

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(local)),
            Arc::new(AllTestPeers),
        );

        let err = uc.execute().await.unwrap_err();
        let EnsureReachableAllError::Repository(msg) = err;
        assert!(msg.contains("sqlite down"), "msg = {msg}");
    }

    #[tokio::test]
    async fn happy_path_three_peers_all_online() {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| {
            Ok(vec![
                record("peer-a", &[0x01]),
                record("peer-b", &[0x02]),
                record("peer-c", &[0x03]),
            ])
        });
        let mut presence = MockPresence::new();
        presence
            .expect_ensure_reachable()
            .times(3)
            .returning(|_| Ok(ReachabilityState::Online));

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(DeviceId::new("local-device"))),
            Arc::new(AllTestPeers),
        );

        let report = uc.execute().await.expect("ok");
        assert_eq!(report.total, 3);
        assert_eq!(report.online, 3);
        assert_eq!(report.offline, 0);
        assert!(report.errors.is_empty());
    }

    #[tokio::test]
    async fn single_failure_does_not_block_others() {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| {
            Ok(vec![
                record("peer-ok-1", &[0x01]),
                record("peer-err", &[0x02]),
                record("peer-ok-2", &[0x03]),
            ])
        });
        let mut presence = MockPresence::new();
        presence
            .expect_ensure_reachable()
            .withf(|d| d.as_str() == "peer-err")
            .times(1)
            .returning(|d| Err(PresenceError::NoAddress(d.clone())));
        presence
            .expect_ensure_reachable()
            .withf(|d| d.as_str() == "peer-ok-1")
            .times(1)
            .returning(|_| Ok(ReachabilityState::Online));
        presence
            .expect_ensure_reachable()
            .withf(|d| d.as_str() == "peer-ok-2")
            .times(1)
            .returning(|_| Ok(ReachabilityState::Offline));

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(DeviceId::new("local-device"))),
            Arc::new(AllTestPeers),
        );

        let report = uc.execute().await.expect("ok");
        assert_eq!(report.total, 3);
        assert_eq!(report.online, 1);
        assert_eq!(report.offline, 1);
        assert_eq!(report.errors.len(), 1);
        assert_eq!(report.errors[0].0.as_str(), "peer-err");
        assert!(report.errors[0].1.contains("no known address"));
    }

    #[tokio::test]
    async fn local_device_in_repo_is_skipped() {
        // Defensive filter test: repo erroneously contains the local device
        // id (future-bug guard). The usecase must skip it without calling
        // ensure_reachable on itself.
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| {
            Ok(vec![
                record("local-device", &[0x01]),
                record("peer-a", &[0x02]),
            ])
        });
        let mut presence = MockPresence::new();
        presence
            .expect_ensure_reachable()
            .withf(|d| d.as_str() == "peer-a")
            .times(1)
            .returning(|_| Ok(ReachabilityState::Online));

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(DeviceId::new("local-device"))),
            Arc::new(AllTestPeers),
        );

        let report = uc.execute().await.expect("ok");
        assert_eq!(report.total, 1);
        assert_eq!(report.online, 1);
    }

    /// Hand-written concurrent fake for `PresencePort`. mockall's
    /// `.returning(...)` closure is stored behind an internal `Mutex`
    /// (required for `FnMut`); parallel calls queue on that lock and
    /// serialise the awaited body, which defeats a concurrency assertion.
    /// A direct trait impl sidesteps that and each call's `tokio::time::sleep`
    /// yields cleanly on any runtime flavour.
    struct SleepyPresence {
        delay: Duration,
        state: ReachabilityState,
    }
    #[async_trait]
    impl PresencePort for SleepyPresence {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            tokio::time::sleep(self.delay).await;
            Ok(self.state)
        }
        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            unreachable!("not exercised")
        }
        fn subscribe(&self) -> broadcast::Receiver<uc_core::ports::PresenceEvent> {
            broadcast::channel(1).1
        }
    }

    #[tokio::test]
    async fn concurrent_execution_independent_tasks() {
        // 3 probes × 200ms: serial wall time ≥ 600ms; concurrent
        // expected ≈ 200ms + overhead. Upper bound 400ms keeps the
        // assertion stable under CI jitter while still catching any
        // regression that re-serialises the JoinSet.
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| {
            Ok(vec![
                record("peer-a", &[0x01]),
                record("peer-b", &[0x02]),
                record("peer-c", &[0x03]),
            ])
        });
        let presence = SleepyPresence {
            delay: Duration::from_millis(200),
            state: ReachabilityState::Online,
        };

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(DeviceId::new("local-device"))),
            Arc::new(AllTestPeers),
        );

        let started = std::time::Instant::now();
        let report = uc.execute().await.expect("ok");
        let elapsed = started.elapsed();
        assert_eq!(report.total, 3);
        assert_eq!(report.online, 3);
        assert!(
            elapsed < Duration::from_millis(400),
            "ensure_reachable_all appears serial: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn removed_peer_is_not_probed_during_a_refresh() {
        let mut repo = MockPeerAddrRepo::new();
        repo.expect_list().times(1).returning(|| {
            Ok(vec![
                record("removed", &[0x01]),
                record("retained", &[0x02]),
            ])
        });
        let mut presence = MockPresence::new();
        presence
            .expect_ensure_reachable()
            .withf(|device| device.as_str() == "retained")
            .times(1)
            .returning(|_| Ok(ReachabilityState::Online));

        let uc = EnsureReachableAllUseCase::new(
            Arc::new(repo),
            Arc::new(presence),
            Arc::new(FixedDevice(DeviceId::new("local-device"))),
            Arc::new(RetainedPeerOnly),
        );

        let report = uc.execute().await.expect("ok");
        assert_eq!(report.total, 1);
        assert_eq!(report.online, 1);
    }
}
