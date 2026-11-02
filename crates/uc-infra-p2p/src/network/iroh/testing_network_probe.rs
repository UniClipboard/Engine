//! 仅验收构建使用的网络探针。预期地址和身份只在内存中比较，不进入输出。
pub mod subnets;

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use iroh::EndpointAddr;
use serde::Serialize;
use uc_core::ids::DeviceId;
use uc_observability_contract::wire_enum;

const EVENT_LIMIT: usize = 512;
const SOCKET_LIMIT: usize = 64;

wire_enum! {
    #[derive(Debug, Clone, Copy)]
    pub(super) enum AddressRefreshStage: &'static str {
        LocalPublishRequested => "local_publish_requested",
        MdnsCandidateValidated => "mdns_candidate_validated",
        DirectoryLookupStarted => "directory_lookup_started",
        DirectoryCandidateReady => "directory_candidate_ready",
        Requested => "requested",
        Coalesced => "coalesced",
        Started => "started",
        StoredReadStarted => "stored_read_started",
        StoredMissing => "stored_missing",
        StoredReadFailed => "stored_read_failed",
        StoredDecoded => "stored_decoded",
        StoredInvalid => "stored_invalid",
        RefreshStored => "refresh_stored",
        RefreshStoredMissing => "refresh_stored_missing",
        RefreshReadFailed => "refresh_read_failed",
        RemoteMissing => "remote_missing",
        RemoteKnown => "remote_known",
        RemoteActive => "remote_active",
        MergeChanged => "merge_changed",
        MergeUnchanged => "merge_unchanged",
        EncodeFailed => "encode_failed",
        ClockInvalid => "clock_invalid",
        UpsertStarted => "upsert_started",
        UpsertFinished => "upsert_finished",
        Finished => "finished",
        Cancelled => "cancelled",
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AddressRefreshEvent {
    pub sequence: u64,
    pub elapsed_micros: u64,
    pub stage: &'static str,
    pub direct_count: Option<usize>,
    pub relay_count: Option<usize>,
    pub has_expected_socket: Option<bool>,
    pub identity_matches_expected: Option<bool>,
    pub success: Option<bool>,
    pub comparison_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddressRefreshSnapshot {
    pub expected_socket_configured: bool,
    pub expected_identity_configured: bool,
    pub dropped_events: u64,
    pub events: Vec<AddressRefreshEvent>,
}

struct Probe {
    peer: DeviceId,
    expected_socket: Option<SocketAddr>,
    expected_identity: Option<[u8; 32]>,
    origin: Instant,
    sequence: u64,
    dropped: u64,
    events: VecDeque<Sample>,
}

// 比较材料不实现 Serialize 或 Debug，只在有界内存中保留到 clear。
struct Sample {
    event: AddressRefreshEvent,
    sockets: Option<Vec<SocketAddr>>,
    identity: Option<[u8; 32]>,
}

fn state() -> MutexGuard<'static, Option<Probe>> {
    static STATE: OnceLock<Mutex<Option<Probe>>> = OnceLock::new();
    STATE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// 宿主应在 Engine 启动前安装探针。返回 false 表示已有探针，原探针保持不变。
/// 每个进程只跟踪一个已配对设备；重启后应重新安装。无效输入返回 false。
pub fn install(peer: &str) -> bool {
    let Some(peer) = DeviceId::try_new(peer) else {
        return false;
    };
    let mut state = state();
    if state.is_some() {
        return false;
    }
    *state = Some(Probe {
        peer,
        expected_socket: None,
        expected_identity: None,
        origin: Instant::now(),
        sequence: 0,
        dropped: 0,
        events: VecDeque::new(),
    });
    true
}

/// 端口分配后设置比较输入，快照会用该输入重新比较此前的有界样本。
/// 宿主不得输出这些输入。此方法不访问 Engine，也不触发刷新。
pub fn set_expected(socket: SocketAddr, identity: Option<[u8; 32]>) -> bool {
    let mut state = state();
    let Some(probe) = state.as_mut() else {
        return false;
    };
    probe.expected_socket = Some(socket);
    probe.expected_identity = identity;
    true
}

/// 仅返回固定阶段、数量、时间和匹配布尔值，不返回比较材料。
pub fn snapshot() -> Option<AddressRefreshSnapshot> {
    state().as_ref().map(|probe| AddressRefreshSnapshot {
        expected_socket_configured: probe.expected_socket.is_some(),
        expected_identity_configured: probe.expected_identity.is_some(),
        dropped_events: probe.dropped,
        events: probe
            .events
            .iter()
            .map(|sample| {
                let mut event = sample.event.clone();
                event.has_expected_socket = sample.sockets.as_ref().and_then(|sockets| {
                    probe.expected_socket.and_then(|expected| {
                        if sockets.contains(&expected) {
                            Some(true)
                        } else if event.comparison_truncated {
                            None
                        } else {
                            Some(false)
                        }
                    })
                });
                event.identity_matches_expected = sample.identity.and_then(|identity| {
                    probe.expected_identity.map(|expected| identity == expected)
                });
                event
            })
            .collect(),
    })
}

/// 宿主在本轮验收结束后移除探针与内存中的比较输入。
pub fn clear() {
    *state() = None;
}

pub(super) fn record(
    peer: &DeviceId,
    stage: AddressRefreshStage,
    address: Option<&EndpointAddr>,
    success: Option<bool>,
) {
    let mut state = state();
    let Some(probe) = state.as_mut().filter(|probe| probe.peer == *peer) else {
        return;
    };
    push(probe, stage, address, success);
}

/// 发现早于设备身份归属，只记录传输候选；验收必须核对预期身份匹配，不能据此授予成员资格。
pub(super) fn record_discovery(
    stage: AddressRefreshStage,
    address: &EndpointAddr,
    success: Option<bool>,
) {
    if let Some(probe) = state().as_mut() {
        push(probe, stage, Some(address), success);
    }
}

fn push(
    probe: &mut Probe,
    stage: AddressRefreshStage,
    address: Option<&EndpointAddr>,
    success: Option<bool>,
) {
    probe.sequence = probe.sequence.saturating_add(1);
    let event = AddressRefreshEvent {
        sequence: probe.sequence,
        elapsed_micros: probe.origin.elapsed().as_micros().min(u64::MAX as u128) as u64,
        stage: stage.wire(),
        direct_count: address.map(|address| address.ip_addrs().count()),
        relay_count: address.map(|address| address.relay_urls().count()),
        has_expected_socket: None,
        identity_matches_expected: None,
        success,
        comparison_truncated: address
            .is_some_and(|address| address.ip_addrs().count() > SOCKET_LIMIT),
    };
    if probe.events.len() == EVENT_LIMIT {
        probe.events.pop_front();
        probe.dropped = probe.dropped.saturating_add(1);
    }
    probe.events.push_back(Sample {
        event,
        sockets: address.map(|address| address.ip_addrs().copied().take(SOCKET_LIMIT).collect()),
        identity: address.map(|address| *address.id.as_bytes()),
    });
}
