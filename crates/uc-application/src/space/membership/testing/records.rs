//! 成员状态负责人测试台：内存成员记录、可控时钟、事件与唤醒记录。

use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MemberInstanceId, MembershipLedger,
    MembershipLedgerSnapshot, PeerLinkSnapshot, PeerRelation, VersionedMembershipHistory,
};
use uc_core::ports::{ClockPort, EmitError, HostEvent, HostEventEmitterPort, MembershipHostEvent};

use crate::space::membership::{
    MembershipLedgerError, MembershipOwner, MembershipProjectionPlan, MembershipRecord,
    MembershipRecordCommit, MembershipRecordStorePort, SpaceMembershipRecord,
    WakeSpaceMembershipMaintenancePort,
};

/// 按修订号条件提交的内存成员记录。
pub(crate) struct MemoryMembershipRecords {
    record: Mutex<MembershipRecord>,
    loads: AtomicUsize,
    commits: AtomicUsize,
    remaining_conflicts: AtomicUsize,
    remaining_failures: AtomicUsize,
    projections: Mutex<Vec<MembershipProjectionPlan>>,
    generation: AtomicU64,
}

impl MemoryMembershipRecords {
    pub(crate) fn new(record: MembershipRecord) -> Arc<Self> {
        Arc::new(Self {
            record: Mutex::new(record),
            loads: AtomicUsize::new(0),
            commits: AtomicUsize::new(0),
            remaining_conflicts: AtomicUsize::new(0),
            remaining_failures: AtomicUsize::new(0),
            projections: Mutex::new(Vec::new()),
            generation: AtomicU64::new(0),
        })
    }

    pub(crate) fn empty() -> Arc<Self> {
        Self::new(MembershipRecord::NoSpace { revision: 0 })
    }

    pub(crate) fn record(&self) -> MembershipRecord {
        self.record.lock().unwrap().clone()
    }

    pub(crate) fn replace(&self, record: MembershipRecord) {
        *self.record.lock().unwrap() = record;
    }

    /// 模拟控制世代切换、恢复出厂等替换数据库：记录换成 `record`，数据库代号改变。
    pub(crate) fn replace_database(&self, record: MembershipRecord) {
        *self.record.lock().unwrap() = record;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn load_count(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    pub(crate) fn commit_count(&self) -> usize {
        self.commits.load(Ordering::SeqCst)
    }

    /// 之后的若干次提交以修订号冲突失败。
    pub(crate) fn conflict_next_commits(&self, count: usize) {
        self.remaining_conflicts.store(count, Ordering::SeqCst);
    }

    /// 之后的若干次提交以存储暂不可用失败，记录保持不变。
    pub(crate) fn fail_next_commits(&self, count: usize) {
        self.remaining_failures.store(count, Ordering::SeqCst);
    }

    pub(crate) fn last_projection(&self) -> Option<MembershipProjectionPlan> {
        self.projections.lock().unwrap().last().cloned()
    }

    pub(crate) fn ledger(&self) -> MembershipLedger {
        match self.record() {
            MembershipRecord::Space(space) => MembershipLedger::restore(space.ledger).unwrap(),
            MembershipRecord::NoSpace { .. } => panic!("the test record has no current space"),
        }
    }

    pub(crate) fn space(&self) -> SpaceMembershipRecord {
        match self.record() {
            MembershipRecord::Space(space) => *space,
            MembershipRecord::NoSpace { .. } => panic!("the test record has no current space"),
        }
    }
}

fn take_one(counter: &AtomicUsize) -> bool {
    counter
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
            remaining.checked_sub(1)
        })
        .is_ok()
}

#[async_trait]
impl MembershipRecordStorePort for MemoryMembershipRecords {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.record())
    }

    async fn commit(&self, commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError> {
        if take_one(&self.remaining_conflicts) {
            return Err(MembershipLedgerError::Conflict);
        }
        if take_one(&self.remaining_failures) {
            return Err(MembershipLedgerError::unavailable());
        }
        let mut record = self.record.lock().unwrap();
        if record.revision() != commit.expected_revision
            || commit.replacement.revision() <= commit.expected_revision
        {
            return Err(MembershipLedgerError::Conflict);
        }
        if let Some(projection) = commit.projection {
            self.projections.lock().unwrap().push(projection);
        }
        *record = commit.replacement;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

/// 可手动推进的时钟。
pub(crate) struct TestClock(AtomicI64);

impl TestClock {
    pub(crate) fn at(now_ms: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now_ms)))
    }

    pub(crate) fn set(&self, now_ms: i64) {
        self.0.store(now_ms, Ordering::SeqCst);
    }

    pub(crate) fn advance(&self, delta_ms: i64) {
        self.0.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl ClockPort for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub(crate) struct RecordingHostEvents(Mutex<Vec<HostEvent>>);

impl RecordingHostEvents {
    /// 已发布的设备信任变化修订号。
    pub(crate) fn committed_revisions(&self) -> Vec<u64> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                HostEvent::Membership(MembershipHostEvent::LedgerCommitted { revision }) => {
                    Some(*revision)
                }
                _ => None,
            })
            .collect()
    }
}

impl HostEventEmitterPort for RecordingHostEvents {
    fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct RecordingWake {
    wakes: AtomicUsize,
    deadlines: Mutex<Vec<i64>>,
}

impl RecordingWake {
    pub(crate) fn wake_count(&self) -> usize {
        self.wakes.load(Ordering::SeqCst)
    }

    pub(crate) fn deadlines(&self) -> Vec<i64> {
        self.deadlines.lock().unwrap().clone()
    }
}

impl WakeSpaceMembershipMaintenancePort for RecordingWake {
    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }

    fn schedule_at(&self, expires_at_ms: i64, _now_ms: i64) {
        self.deadlines.lock().unwrap().push(expires_at_ms);
    }
}

pub(crate) struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

/// 一个 Owner 及其全部可观察协作者。
pub(crate) struct OwnerFixture {
    pub(crate) owner: Arc<MembershipOwner>,
    pub(crate) records: Arc<MemoryMembershipRecords>,
    pub(crate) clock: Arc<TestClock>,
    pub(crate) events: Arc<RecordingHostEvents>,
    pub(crate) wake: Arc<RecordingWake>,
}

impl OwnerFixture {
    pub(crate) fn new(record: MembershipRecord) -> Self {
        Self::with_verifier(record, Arc::new(AcceptingVerifier))
    }

    pub(crate) fn with_verifier(
        record: MembershipRecord,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    ) -> Self {
        let records = MemoryMembershipRecords::new(record);
        let clock = TestClock::at(1_000_000);
        let events = Arc::new(RecordingHostEvents::default());
        let wake = Arc::new(RecordingWake::default());
        let owner = Arc::new(MembershipOwner::new(
            records.clone(),
            verifier,
            clock.clone(),
            events.clone(),
            wake.clone(),
        ));
        Self {
            owner,
            records,
            clock,
            events,
            wake,
        }
    }
}

impl OwnerFixture {
    /// 在同一持久记录上重新建立负责人，模拟进程重启。
    pub(crate) fn reopen(&self) -> Arc<MembershipOwner> {
        Arc::new(MembershipOwner::new(
            self.records.clone(),
            Arc::new(AcceptingVerifier),
            self.clock.clone(),
            self.events.clone(),
            self.wake.clone(),
        ))
    }

    /// 直接改写持久记录并让负责人重新读取。
    pub(crate) fn edit(&self, change: impl FnOnce(&mut SpaceMembershipRecord)) {
        let mut space = self.records.space();
        change(&mut space);
        self.records
            .replace(MembershipRecord::Space(Box::new(space)));
        self.owner.reload_for_test();
    }
}

/// 以 Core 起点规则建立当前 Space 的成员记录：其他成员一致、尚未确认本机位置。
pub(crate) fn started_record(
    history: VersionedMembershipHistory,
    local_device_id: DeviceId,
    local_member: MemberInstanceId,
    revision: u64,
) -> MembershipRecord {
    let ledger = MembershipLedger::start(history, local_device_id, local_member, revision).unwrap();
    record_of(ledger.snapshot())
}

pub(crate) fn record_of(ledger: MembershipLedgerSnapshot) -> MembershipRecord {
    MembershipRecord::Space(Box::new(SpaceMembershipRecord {
        ledger,
        history_exchange: Default::default(),
        branch_recovery: Default::default(),
    }))
}

/// 修改记录中一个成员对端的关系与已确认位置；记录必须仍满足账本不变量。
pub(crate) fn with_peer_relation(
    record: MembershipRecord,
    peer: &DeviceId,
    relation: PeerRelation,
    confirmed_position: Option<BaseMembershipHistoryPosition>,
) -> MembershipRecord {
    let MembershipRecord::Space(mut space) = record else {
        panic!("the test record has no current space");
    };
    match space.ledger.peers.get_mut(peer) {
        Some(PeerLinkSnapshot::Member(member)) => {
            member.relation = relation;
            member.confirmed_position = confirmed_position;
        }
        _ => panic!("the test peer is not a member"),
    }
    MembershipRecord::Space(space)
}

/// 修改记录中一个成员对端的同步退避。
pub(crate) fn with_peer_sync(
    record: MembershipRecord,
    peer: &DeviceId,
    sync: uc_core::membership::PeerSyncBackoffSnapshot,
) -> MembershipRecord {
    let MembershipRecord::Space(mut space) = record else {
        panic!("the test record has no current space");
    };
    match space.ledger.peers.get_mut(peer) {
        Some(PeerLinkSnapshot::Member(member)) => member.sync = sync,
        _ => panic!("the test peer is not a member"),
    }
    MembershipRecord::Space(space)
}

/// 在测试中直接改写持久历史（模拟其他来源已提交的事实），按账本规范化后写回。
pub(crate) fn replace_history(
    records: &MemoryMembershipRecords,
    change: impl FnOnce(&mut VersionedMembershipHistory),
) {
    let MembershipRecord::Space(mut space) = records.record() else {
        panic!("the test record has no current space");
    };
    change(&mut space.ledger.history);
    space.ledger = MembershipLedger::restore_normalized(space.ledger)
        .unwrap()
        .snapshot();
    records.replace(MembershipRecord::Space(space));
}

/// 固定工作模式的许可来源。
pub(crate) struct FixedSpaceWorkMode(pub(crate) crate::space::membership::SpaceWorkMode);

impl FixedSpaceWorkMode {
    pub(crate) fn active() -> Arc<Self> {
        Arc::new(Self(crate::space::membership::SpaceWorkMode::Active))
    }
}

#[async_trait]
impl crate::space::membership::AcquireSpaceWorkPermitPort for FixedSpaceWorkMode {
    async fn acquire_space_work_permit(
        &self,
    ) -> Result<
        crate::space::membership::SpaceWorkPermit,
        crate::space::membership::QuerySpaceWorkModeError,
    > {
        Ok(crate::space::membership::SpaceWorkPermit::unlocked(self.0))
    }
}
