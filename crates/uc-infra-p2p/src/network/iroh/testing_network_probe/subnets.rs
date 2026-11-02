//! 仅验收构建使用的 subnet owner 观察与原生读取控制。禁止从发送 hook 调用。
use std::collections::VecDeque;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uc_observability_contract::wire_enum;

const EVENT_LIMIT: usize = 512;
const HOLD_LIMIT: Duration = Duration::from_secs(90);

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum SubnetReadFault {
        FailResultOnce => "fail_result_once",
        HoldBeforeNative => "hold_before_native",
        HoldAfterNative => "hold_after_native",
    }
}

wire_enum! {
    serde
    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    pub enum SubnetMarker {
        KernelMutationBegin => "kernel_mutation_begin",
        KernelMutationEnd => "kernel_mutation_end",
        TrafficBegin => "traffic_begin",
        TrafficEnd => "traffic_end",
    }
}

wire_enum! {
    #[derive(Clone, Copy)]
    pub(in super::super) enum SubnetStage: &'static str {
        Prepared => "prepared",
        ReadScheduled => "read_scheduled",
        ReaderEntered => "reader_entered",
        NativeReadStarted => "native_read_started",
        NativeReadFinished => "native_read_finished",
        FaultArmed => "fault_armed",
        FaultHit => "fault_hit",
        HeldBefore => "held_before_native",
        HeldAfter => "held_after_native",
        Released => "read_released",
        HoldTimedOut => "hold_timed_out",
        FailureInjected => "read_failure_injected",
        ReadFailed => "owner_read_failed",
        ReadJoinFailed => "read_join_failed",
        Invalidated => "native_invalidated",
        Published => "published",
        StaleDiscarded => "stale_generation_discarded",
        StoppedDiscarded => "stopped_result_discarded",
        Stopped => "owner_stopped",
        CancelledHold => "hold_cancelled_by_stop",
        WorkerFinished => "worker_body_finished",
        WorkerJoined => "worker_joined",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SubnetEvent {
    pub sequence: u64,
    pub elapsed_micros: u64,
    pub unix_micros: u64,
    pub stage: &'static str,
    pub owner_alias: Option<u64>,
    pub generation: Option<u64>,
    pub count: Option<usize>,
    pub matches_expected: Option<bool>,
    pub lease_until_micros: Option<u64>,
    pub success: Option<bool>,
    pub fault: Option<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubnetSnapshot {
    pub elapsed_micros: u64,
    pub dropped_events: u64,
    pub owner_alias: Option<u64>,
    pub fault_armed: bool,
    pub fault_active: bool,
    pub held_phase: Option<&'static str>,
    pub owner_stopped: bool,
    pub worker_body_finished: bool,
    pub worker_joined: bool,
    pub dynamic_view_matches_now: bool,
    pub lease_expired_now: bool,
    pub events: Vec<SubnetEvent>,
}

struct Held {
    owner: u64,
    generation: u64,
    phase: &'static str,
    until: Instant,
}

struct PublishedView {
    matches: bool,
    until: Instant,
}

struct Probe {
    expected: IpAddr,
    origin: Instant,
    owner: Option<u64>,
    stopped: bool,
    finished: bool,
    joined: bool,
    active_fault: Option<u64>,
    view: Option<PublishedView>,
    plan: Option<(u64, SubnetReadFault)>,
    held: Option<Held>,
    sequence: u64,
    dropped: u64,
    events: VecDeque<SubnetEvent>,
}

impl Probe {
    fn push(
        &mut self,
        stage: &'static str,
        owner: Option<u64>,
        generation: Option<u64>,
        success: Option<bool>,
    ) {
        self.sequence = self.sequence.saturating_add(1);
        if self.events.len() == EVENT_LIMIT {
            self.events.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.events.push_back(SubnetEvent {
            sequence: self.sequence,
            elapsed_micros: micros(self.origin.elapsed()),
            unix_micros: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(micros)
                .unwrap_or(0),
            stage,
            owner_alias: owner,
            generation,
            count: None,
            matches_expected: None,
            lease_until_micros: None,
            success,
            fault: None,
        });
    }
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

struct Registry {
    state: Mutex<Option<Probe>>,
    released: Condvar,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry {
        state: Mutex::new(None),
        released: Condvar::new(),
    })
}

fn state() -> MutexGuard<'static, Option<Probe>> {
    registry()
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 在 Engine 启动前安装。预期 IP 仅保存在内存中，不进入输出。
pub fn install(expected: IpAddr) -> bool {
    let mut state = state();
    if state.is_some() {
        return false;
    }
    *state = Some(Probe {
        expected,
        origin: Instant::now(),
        owner: None,
        stopped: false,
        finished: false,
        joined: false,
        active_fault: None,
        view: None,
        plan: None,
        held: None,
        sequence: 0,
        dropped: 0,
        events: VecDeque::new(),
    });
    true
}

/// 只控制当前 subnet owner 的下一次读取。只允许一个计划或一次挂起。
pub fn arm_read(fault: SubnetReadFault) -> bool {
    let mut state = state();
    let Some(probe) = state.as_mut() else {
        return false;
    };
    let Some(owner) = probe.owner else {
        return false;
    };
    if probe.stopped || probe.plan.is_some() || probe.held.is_some() || probe.active_fault.is_some()
    {
        return false;
    }
    probe.plan = Some((owner, fault));
    probe.push(SubnetStage::FaultArmed.wire(), Some(owner), None, None);
    if let Some(event) = probe.events.back_mut() {
        event.fault = Some(fault.wire());
    }
    true
}

/// 释放已经命中的挂起，不触发刷新、不取消真正的原生读取。
pub fn release_read() -> bool {
    let mut state = state();
    let Some(probe) = state.as_mut() else {
        return false;
    };
    let Some(held) = probe.held.take() else {
        return false;
    };
    probe.push(
        SubnetStage::Released.wire(),
        Some(held.owner),
        Some(held.generation),
        None,
    );
    registry().released.notify_all();
    true
}

/// 标记外部 OS 动作或流量边界；此函数不执行动作，也不修改 subnet owner。
pub fn mark(marker: SubnetMarker) -> bool {
    let mut state = state();
    let Some(probe) = state.as_mut() else {
        return false;
    };
    probe.push(marker.wire(), probe.owner, None, None);
    true
}

/// 返回有界观察。当前 lease 判断不是发送结果；仍须检查真实包与接收端读回。
pub fn snapshot() -> Option<SubnetSnapshot> {
    state().as_ref().map(|probe| {
        let now = Instant::now();
        SubnetSnapshot {
            elapsed_micros: micros(now.saturating_duration_since(probe.origin)),
            dropped_events: probe.dropped,
            owner_alias: probe.owner,
            fault_armed: probe.plan.is_some(),
            fault_active: probe.active_fault.is_some(),
            held_phase: probe.held.as_ref().map(|held| held.phase),
            owner_stopped: probe.stopped,
            worker_body_finished: probe.finished,
            worker_joined: probe.joined,
            dynamic_view_matches_now: probe
                .view
                .as_ref()
                .is_some_and(|view| view.matches && now < view.until),
            lease_expired_now: probe.view.as_ref().is_some_and(|view| now >= view.until),
            events: probe.events.iter().cloned().collect(),
        }
    })
}

/// 导出最终快照后清除输入、记录和计划，并释放挂起的读取。
pub fn clear() {
    *state() = None;
    registry().released.notify_all();
}

pub(in super::super) fn owner_created() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let owner = NEXT.fetch_add(1, Ordering::Relaxed);
    if let Some(probe) = state().as_mut() {
        probe.owner = Some(owner);
        probe.stopped = false;
        probe.finished = false;
        probe.joined = false;
        probe.view = None;
        probe.push(SubnetStage::Prepared.wire(), Some(owner), Some(0), None);
    }
    owner
}

pub(in super::super) fn observe(
    owner: u64,
    generation: u64,
    stage: SubnetStage,
    success: Option<bool>,
) {
    if let Some(probe) = state().as_mut() {
        probe.push(stage.wire(), Some(owner), Some(generation), success);
    }
}

pub(in super::super) fn read_scheduled(owner: u64, generation: u64, lease_until: Instant) {
    if let Some(probe) = state().as_mut() {
        probe.push(
            SubnetStage::ReadScheduled.wire(),
            Some(owner),
            Some(generation),
            None,
        );
        let deadline = micros(lease_until.saturating_duration_since(probe.origin));
        if let Some(event) = probe.events.back_mut() {
            event.lease_until_micros = Some(deadline);
        }
    }
}

pub(in super::super) fn invalidated(owner: u64, generation: u64) {
    if let Some(probe) = state().as_mut() {
        if probe.owner == Some(owner) {
            probe.view = None;
        }
        probe.push(
            SubnetStage::Invalidated.wire(),
            Some(owner),
            Some(generation),
            None,
        );
    }
}

pub(in super::super) fn published(
    owner: u64,
    generation: u64,
    count: usize,
    until: Option<Instant>,
    matches: impl FnOnce(IpAddr) -> bool,
) {
    if let Some(probe) = state().as_mut() {
        let matched = matches(probe.expected);
        if probe.owner == Some(owner) {
            probe.view = until.map(|until| PublishedView {
                matches: matched,
                until,
            });
        }
        probe.push(
            SubnetStage::Published.wire(),
            Some(owner),
            Some(generation),
            None,
        );
        let deadline = until.map(|until| micros(until.saturating_duration_since(probe.origin)));
        if let Some(event) = probe.events.back_mut() {
            event.count = Some(count);
            event.matches_expected = Some(matched);
            event.lease_until_micros = deadline;
        }
    }
}

pub(in super::super) fn stopped(owner: u64, generation: u64) {
    if let Some(probe) = state().as_mut() {
        if probe.owner == Some(owner) {
            probe.view = None;
            if !probe.stopped {
                probe.stopped = true;
                probe.push(
                    SubnetStage::Stopped.wire(),
                    Some(owner),
                    Some(generation),
                    None,
                );
            }
        }
        if probe
            .plan
            .is_some_and(|(planned_owner, _)| planned_owner == owner)
        {
            probe.plan = None;
        }
        if probe.active_fault == Some(owner) {
            probe.active_fault = None;
        }
        if probe.held.as_ref().is_some_and(|held| held.owner == owner) {
            probe.held = None;
            probe.push(
                SubnetStage::CancelledHold.wire(),
                Some(owner),
                Some(generation),
                None,
            );
        }
    }
    registry().released.notify_all();
}

pub(in super::super) fn worker_finished(owner: u64, success: bool) {
    if let Some(probe) = state().as_mut() {
        if probe.owner == Some(owner) {
            probe.finished = true;
        }
        probe.push(
            SubnetStage::WorkerFinished.wire(),
            Some(owner),
            None,
            Some(success),
        );
    }
}

pub(in super::super) fn worker_joined(owner: u64, success: bool) {
    if let Some(probe) = state().as_mut() {
        if probe.owner == Some(owner) {
            probe.joined = true;
        }
        probe.push(
            SubnetStage::WorkerJoined.wire(),
            Some(owner),
            None,
            Some(success),
        );
    }
}

fn hold(owner: u64, generation: u64, phase: SubnetStage) {
    let mut state = state();
    let Some(probe) = state.as_mut() else {
        return;
    };
    if probe.owner != Some(owner) || probe.stopped {
        return;
    }
    let until = Instant::now() + HOLD_LIMIT;
    probe.held = Some(Held {
        owner,
        generation,
        phase: phase.wire(),
        until,
    });
    probe.push(phase.wire(), Some(owner), Some(generation), None);
    loop {
        let waiting = state
            .as_ref()
            .and_then(|probe| probe.held.as_ref())
            .filter(|held| held.owner == owner);
        let Some(waiting) = waiting else {
            break;
        };
        let remaining = waiting.until.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if let Some(probe) = state.as_mut() {
                probe.held = None;
                probe.push(
                    SubnetStage::HoldTimedOut.wire(),
                    Some(owner),
                    Some(generation),
                    None,
                );
            }
            break;
        }
        let (next, _) = registry()
            .released
            .wait_timeout(state, remaining)
            .unwrap_or_else(PoisonError::into_inner);
        state = next;
    }
}

pub(in super::super) fn before_native(owner: u64, generation: u64) -> Option<SubnetReadFault> {
    let fault = {
        let mut state = state();
        let probe = state.as_mut()?;
        probe.push(
            SubnetStage::ReaderEntered.wire(),
            Some(owner),
            Some(generation),
            None,
        );
        let fault = probe
            .plan
            .filter(|(planned_owner, _)| *planned_owner == owner)
            .map(|(_, fault)| fault);
        if fault.is_some() {
            probe.plan = None;
            probe.active_fault = Some(owner);
            probe.push(
                SubnetStage::FaultHit.wire(),
                Some(owner),
                Some(generation),
                None,
            );
            if let Some(event) = probe.events.back_mut() {
                event.fault = fault.map(SubnetReadFault::wire);
            }
        }
        fault
    };
    if fault == Some(SubnetReadFault::HoldBeforeNative) {
        hold(owner, generation, SubnetStage::HeldBefore);
    }
    observe(owner, generation, SubnetStage::NativeReadStarted, None);
    fault
}

pub(in super::super) fn after_native<T>(
    owner: u64,
    generation: u64,
    fault: Option<SubnetReadFault>,
    result: io::Result<T>,
    inspect: impl FnOnce(&T, IpAddr) -> (usize, bool),
) -> io::Result<T> {
    if let Some(probe) = state().as_mut() {
        let sampled = result
            .as_ref()
            .ok()
            .map(|value| inspect(value, probe.expected));
        probe.push(
            SubnetStage::NativeReadFinished.wire(),
            Some(owner),
            Some(generation),
            Some(result.is_ok()),
        );
        if let Some((count, matched)) = sampled {
            if let Some(event) = probe.events.back_mut() {
                event.count = Some(count);
                event.matches_expected = Some(matched);
            }
        }
    }
    if fault == Some(SubnetReadFault::HoldAfterNative) {
        hold(owner, generation, SubnetStage::HeldAfter);
    }
    let result = if fault == Some(SubnetReadFault::FailResultOnce) && result.is_ok() {
        observe(owner, generation, SubnetStage::FailureInjected, Some(false));
        Err(io::Error::other(
            "injected subnet enumeration result failure",
        ))
    } else {
        result
    };
    if let Some(probe) = state().as_mut() {
        if probe.active_fault == Some(owner) {
            probe.active_fault = None;
        }
    }
    result
}
