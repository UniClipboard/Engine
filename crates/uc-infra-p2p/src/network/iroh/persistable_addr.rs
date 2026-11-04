//! 把本机或远端的 iroh [`EndpointAddr`] 整理成**可长期保存**的配对设备地址，
//! 供 [`PeerAddressRepositoryPort`] 的实现密文保存。
//!
//! ## 保存什么
//!
//! 保存的记录只是拨号**线索**，不是可达性证明：
//!
//! | 组成 | 跨进程重启是否稳定 |
//! |------|--------------------|
//! | NodeId | 稳定：由持久私钥派生，连接时由 QUIC 握手认证 |
//! | Relay URL | 基本稳定：随 home relay 选择变化 |
//! | 直连 `Ip(SocketAddr)` | IP 视网络而定；端口默认每次启动随机，配置固定端口（`network.bind_port` 或 `UC_IROH_BIND_PORT`）后稳定 |
//!
//! 直连线索与 relay 线索并存，并且数量有上限（[`MAX_DIRECT_HINTS`]）。在 LAN-only
//! 或 WireGuard 这类没有 relay、也没有组播发现的网络里，已知的对端 IP 与固定端口是
//! 重启后唯一能找到对端的信息；只保存 relay 会让 LAN-only 拨号剥掉 relay 后只剩身份。
//!
//! ## 为什么陈旧端口不再导致拨号挂起
//!
//! 早期实现在存在 relay 时丢弃全部直连地址：随机端口在对端重启后失效，旧版拨号
//! 只按存储地址等待完整握手预算，表现为对端明明在线却一直离线。现在拨号把存储线索
//! 交给 iroh，iroh 在没有选定路径时**同时**启动地址发现（mDNS、DNS/PKARR），初始包发往
//! 全部已知候选，任何一条可用路径胜出（见 `connect.rs` 的
//! `dial_with_a_stale_stored_address_follows_address_lookup` 回归测试）。因此陈旧线索
//! 最多浪费一个候选，不会阻塞新发现的地址；它也从不作为唯一的放行条件。
//!
//! ## 刷新是合并而不是替换
//!
//! 成员历史验证成功后，[`refreshed_remote_addr`] 只把本次连接中**正在使用**的路径
//! （relay 或直连）作为新证据，并保留尚未被否定的旧直连线索：同一 IP 已经在另一个
//! 端口上被证实可用时，旧端口才视为被否定。只有使用中的 relay 会替换旧 relay；
//! 本次只走直连时保留旧 relay 线索。没有任何使用中路径时不改写记录。
//!
//! [`PeerAddressRepositoryPort`]: uc_core::ports::PeerAddressRepositoryPort

use std::collections::BTreeSet;

use chrono::{TimeZone, Utc};
use iroh::endpoint::TransportAddrUsage;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use uc_core::ids::DeviceId;
use uc_core::ports::{ClockPort, PeerAddressRecord, PeerAddressRepositoryPort};
use uc_observability_contract::uc_warn;

/// 一条记录最多保留的直连线索数。足够覆盖同一设备的有线、无线与隧道地址，
/// 又不会让多网卡（容器网桥等）把记录和拨号候选撑大。
pub const MAX_DIRECT_HINTS: usize = 8;

/// 把本机当前地址整理为可保存的公告形态：保留身份、全部 relay 线索，以及至多
/// [`MAX_DIRECT_HINTS`] 个直连线索（按地址排序取前几个，IPv4 在前）。
pub fn to_persistable_addr(addr: EndpointAddr) -> EndpointAddr {
    let EndpointAddr { id, addrs } = addr;
    let mut direct = 0;
    let kept = addrs.into_iter().filter(|addr| {
        if !addr.is_ip() {
            return true;
        }
        direct += 1;
        direct <= MAX_DIRECT_HINTS
    });
    EndpointAddr::from_parts(id, kept)
}

/// 以一次已建立连接的观察刷新已保存记录。
///
/// `observed` 为远端每个已知地址及其是否正在使用。只有正在使用的路径才是新证据：
/// 使用中的 relay 替换旧 relay，使用中的直连排在最前；旧直连线索保留到上限，除非
/// 同一 IP 已在另一个端口上被证实可用。没有使用中路径或结果与原记录相同时返回
/// `None`，调用方不必写入。
pub(super) fn refreshed_remote_addr(
    existing: &EndpointAddr,
    observed: impl IntoIterator<Item = (TransportAddr, bool)>,
) -> Option<EndpointAddr> {
    let active: Vec<TransportAddr> = observed
        .into_iter()
        .filter_map(|(addr, active)| active.then_some(addr))
        .filter(|addr| addr.is_ip() || addr.is_relay())
        .collect();
    if active.is_empty() {
        return None;
    }
    let active_direct: Vec<_> = active
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(socket) => Some(*socket),
            _ => None,
        })
        .collect();
    let relays: Vec<TransportAddr> = if active.iter().any(TransportAddr::is_relay) {
        active
            .iter()
            .filter(|addr| addr.is_relay())
            .cloned()
            .collect()
    } else {
        existing
            .addrs
            .iter()
            .filter(|addr| addr.is_relay())
            .cloned()
            .collect()
    };
    let retained_direct = existing.ip_addrs().copied().filter(|old| {
        !active_direct
            .iter()
            .any(|current| current.ip() == old.ip() && current.port() != old.port())
    });
    let mut seen = BTreeSet::new();
    let direct = active_direct
        .iter()
        .copied()
        .chain(retained_direct)
        .filter(|socket| seen.insert(*socket))
        .take(MAX_DIRECT_HINTS)
        .map(TransportAddr::Ip);
    let refreshed = EndpointAddr::from_parts(existing.id, relays.into_iter().chain(direct));
    (refreshed != *existing).then_some(refreshed)
}

/// 读取远端当前路径并与已保存记录合并；远端未知、没有使用中路径或结果不变时返回 `None`。
pub(super) async fn observed_refreshed_remote_addr(
    endpoint: &Endpoint,
    existing: &EndpointAddr,
) -> Option<EndpointAddr> {
    let info = endpoint.remote_info(existing.id).await?;
    refreshed_remote_addr(
        existing,
        info.into_addrs().map(|addr| {
            let active = matches!(addr.usage(), TransportAddrUsage::Active);
            (addr.into_addr(), active)
        }),
    )
}

pub(super) async fn persist_observed_stable_addr(
    repository: &dyn PeerAddressRepositoryPort,
    clock: &dyn ClockPort,
    device: &DeviceId,
    addr: EndpointAddr,
) {
    let Ok(addr_blob) = postcard::to_stdvec(&addr) else {
        return;
    };
    let Some(observed_at) = Utc.timestamp_millis_opt(clock.now_ms()).single() else {
        return;
    };
    // 仅记录仓储写入失败：编码与时钟越界几乎不会失败；失败会留下陈旧地址，后续拨号才暴露。
    if let Err(error) = repository
        .upsert(&PeerAddressRecord {
            device_id: device.clone(),
            addr_blob,
            observed_at,
        })
        .await
    {
        let (category, stage) = error.diagnostic();
        uc_warn!(
            error_kind = "peer_address_persist",
            reason = category,
            stage = stage,
            "observed peer address could not be saved"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use iroh::{EndpointId, RelayUrl, SecretKey};
    use uc_core::ids::DeviceId;
    use uc_core::ports::{
        ClockPort, PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort,
    };

    #[derive(Default)]
    struct RecordingRepository {
        saved: Mutex<Option<PeerAddressRecord>>,
        fail: bool,
    }

    #[async_trait]
    impl PeerAddressRepositoryPort for RecordingRepository {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(None)
        }

        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            if self.fail {
                return Err(PeerAddressError::Internal("injected".into()));
            }
            *self.saved.lock().unwrap() = Some(record.clone());
            Ok(())
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(Vec::new())
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), PeerAddressError> {
            Ok(())
        }
    }

    struct FixedClock;

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            1_700_000_000_000
        }
    }

    fn test_id() -> EndpointId {
        SecretKey::generate().public()
    }

    fn lan_addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 5), port))
    }

    fn wan_addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 2, 3, 4), port))
    }

    fn relay() -> TransportAddr {
        TransportAddr::Relay("https://relay.example.com/".parse().unwrap())
    }

    fn other_relay() -> TransportAddr {
        TransportAddr::Relay("https://other-relay.example.com/".parse().unwrap())
    }

    fn wg_addr(port: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 8, 0, 2), port))
    }

    #[test]
    fn announcement_keeps_direct_hints_alongside_the_relay() {
        let id = test_id();
        let full = EndpointAddr::from_parts(
            id,
            [
                TransportAddr::Ip(lan_addr(50754)),
                TransportAddr::Ip(wg_addr(51820)),
                relay(),
            ],
        );

        let persisted = to_persistable_addr(full.clone());

        assert_eq!(persisted, full);
    }

    #[test]
    fn announcement_bounds_the_number_of_direct_hints() {
        let id = test_id();
        let many = (0..MAX_DIRECT_HINTS as u16 + 4)
            .map(|offset| TransportAddr::Ip(lan_addr(40000 + offset)))
            .chain([relay()]);

        let persisted = to_persistable_addr(EndpointAddr::from_parts(id, many));

        assert_eq!(persisted.ip_addrs().count(), MAX_DIRECT_HINTS);
        assert_eq!(persisted.relay_urls().count(), 1);
    }

    #[test]
    fn announcement_of_identity_only_or_relay_only_is_unchanged() {
        let id = test_id();
        assert_eq!(
            to_persistable_addr(EndpointAddr::new(id)),
            EndpointAddr::new(id)
        );
        let relay_only = EndpointAddr::from_parts(id, [relay()]);
        assert_eq!(to_persistable_addr(relay_only.clone()), relay_only);
    }

    /// 本次只走直连：记下证实可用的直连地址，保留旧 relay 与其他未被否定的直连线索。
    #[test]
    fn direct_only_refresh_adds_the_working_address_and_keeps_existing_hints() {
        let id = test_id();
        let existing = EndpointAddr::from_parts(id, [TransportAddr::Ip(lan_addr(50754)), relay()]);

        let refreshed = refreshed_remote_addr(
            &existing,
            [
                (TransportAddr::Ip(wg_addr(51820)), true),
                (TransportAddr::Ip(wan_addr(9)), false),
            ],
        )
        .expect("a working direct path is new evidence");

        assert_eq!(
            refreshed,
            EndpointAddr::from_parts(
                id,
                [
                    TransportAddr::Ip(wg_addr(51820)),
                    TransportAddr::Ip(lan_addr(50754)),
                    relay(),
                ],
            )
        );
    }

    /// 同一 IP 已在新端口上证实可用时，旧端口被否定；别的直连线索仍保留。
    #[test]
    fn refresh_replaces_a_hint_whose_port_was_disproven() {
        let id = test_id();
        let existing = EndpointAddr::from_parts(
            id,
            [
                TransportAddr::Ip(wg_addr(40000)),
                TransportAddr::Ip(lan_addr(50754)),
            ],
        );

        let refreshed =
            refreshed_remote_addr(&existing, [(TransportAddr::Ip(wg_addr(51820)), true)])
                .expect("changed");

        assert_eq!(
            refreshed,
            EndpointAddr::from_parts(
                id,
                [
                    TransportAddr::Ip(wg_addr(51820)),
                    TransportAddr::Ip(lan_addr(50754)),
                ],
            )
        );
    }

    /// 本次经 relay 连通：使用中的 relay 替换旧 relay，直连线索不被抹掉。
    #[test]
    fn relay_refresh_replaces_the_old_relay_but_keeps_direct_hints() {
        let id = test_id();
        let existing =
            EndpointAddr::from_parts(id, [TransportAddr::Ip(wg_addr(51820)), other_relay()]);

        let refreshed = refreshed_remote_addr(&existing, [(relay(), true), (other_relay(), false)])
            .expect("changed");

        assert_eq!(
            refreshed,
            EndpointAddr::from_parts(id, [TransportAddr::Ip(wg_addr(51820)), relay()])
        );
    }

    #[test]
    fn refresh_without_active_paths_or_without_change_writes_nothing() {
        let id = test_id();
        let existing = EndpointAddr::from_parts(id, [TransportAddr::Ip(wg_addr(51820)), relay()]);

        assert!(refreshed_remote_addr(
            &existing,
            [
                (TransportAddr::Ip(lan_addr(1)), false),
                (other_relay(), false),
            ],
        )
        .is_none());
        assert!(refreshed_remote_addr(
            &existing,
            [(TransportAddr::Ip(wg_addr(51820)), true), (relay(), true)],
        )
        .is_none());
    }

    #[test]
    fn refresh_keeps_working_paths_first_within_the_hint_bound() {
        let id = test_id();
        let existing = EndpointAddr::from_parts(
            id,
            (0..MAX_DIRECT_HINTS as u16).map(|offset| TransportAddr::Ip(lan_addr(40000 + offset))),
        );

        let refreshed =
            refreshed_remote_addr(&existing, [(TransportAddr::Ip(wg_addr(51820)), true)])
                .expect("changed");

        assert_eq!(refreshed.ip_addrs().count(), MAX_DIRECT_HINTS);
        assert!(refreshed.ip_addrs().any(|addr| *addr == wg_addr(51820)));
    }

    #[tokio::test]
    async fn stable_remote_hint_is_saved_with_the_observation_time_and_failure_is_best_effort() {
        let id = test_id();
        let relay: RelayUrl = "https://active-relay.example.com/".parse().unwrap();
        let addr = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay)]);
        let device = DeviceId::new("device-b");
        let repository = Arc::new(RecordingRepository::default());

        persist_observed_stable_addr(repository.as_ref(), &FixedClock, &device, addr.clone()).await;

        let saved = repository.saved.lock().unwrap().clone().unwrap();
        assert_eq!(saved.device_id, device);
        assert_eq!(saved.observed_at.timestamp_millis(), 1_700_000_000_000);
        assert_eq!(
            postcard::from_bytes::<EndpointAddr>(&saved.addr_blob).unwrap(),
            addr
        );

        let failing = RecordingRepository {
            saved: Mutex::new(None),
            fail: true,
        };
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        persist_observed_stable_addr(&failing, &FixedClock, &device, EndpointAddr::new(id)).await;

        assert_eq!(logs.count("observed peer address could not be saved"), 1);
        assert!(logs
            .output()
            .contains("error_kind=\"peer_address_persist\""));
        assert!(!logs.output().contains("device-b"));
    }
}
