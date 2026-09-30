//! 对端地址进入 [`PeerAddressRepositoryPort`] 前的整理规则。
//!
//! 仓储里的记录有两个来源，规则不同：
//!
//! 1. **配对时对端自报的地址**（[`to_persistable_addr`]）：尚未被任何一次连接验证。
//!    有 relay 时去掉直连 IP，只留节点 ID 与 relay；没有 relay 时（LAN-only 或禁用
//!    relay 的测试）保留直连 IP，否则拨号时没有任何路径可试。
//! 2. **成员历史交换成功后观察到的路径**（[`reusable_remote_addr`]）：只保存本次连接
//!    正在使用的路径。直连地址只保存私有地址空间内、且拨号过滤会保留的地址；公网与
//!    NAT 映射地址、回环和链路本地地址都不保存。本次没有使用 relay 时沿用已保存的
//!    relay，避免直连成功反而丢掉长期提示。
//!
//! 默认随机端口重启后，保存的直连地址会失效。这不会拖慢拨号：iroh 在选定路径前把
//! 握手包同时发往全部已知路径（已保存地址、mDNS 与其他发现结果），`connect.rs` 的
//! 单次尝试上限为 3 秒并按 0/500/1500ms 错峰，失效地址只是得不到回应。下一次成功
//! 交换会覆盖旧记录。
//!
//! [`PeerAddressRepositoryPort`]: uc_core::ports::PeerAddressRepositoryPort

use chrono::{TimeZone, Utc};
use iroh::endpoint::TransportAddrUsage;
use iroh::{Endpoint, EndpointAddr, TransportAddr};
use uc_core::ids::DeviceId;
use uc_core::network::{is_private_address, TrustedNetworks};
use uc_core::ports::{ClockPort, PeerAddressRecord, PeerAddressRepositoryPort};

use super::addr_filter::is_virtual_nic_ip;

/// 把配对时对端自报的地址整理成可保存形式：有 relay 时去掉直连 IP；没有 relay 时原样
/// 保留，调用方仍有可拨的路径。见模块文档。
pub fn to_persistable_addr(addr: EndpointAddr) -> EndpointAddr {
    let has_relay = addr
        .addrs
        .iter()
        .any(|a| matches!(a, TransportAddr::Relay(_)));
    if !has_relay {
        return addr;
    }

    let EndpointAddr { id, addrs } = addr;
    let kept = addrs
        .into_iter()
        .filter(|addr| !matches!(addr, TransportAddr::Ip(_)));
    EndpointAddr::from_parts(id, kept)
}

/// 从一次已建立连接的路径观察中提取可长期保存的远端地址。
///
/// `observed` 为 `(地址, 是否正在使用)`。保留正在使用的 relay，以及正在使用、位于私有
/// 地址空间且未被可信判定排除的直连地址；本次没有正在使用的 relay 时沿用 `stored`
/// 中的 relay。没有任何正在使用的可保存路径时返回 `None`，不覆盖已有记录。
pub(super) fn reusable_remote_addr(
    stored: &EndpointAddr,
    observed: impl IntoIterator<Item = (TransportAddr, bool)>,
    trusted: &TrustedNetworks,
) -> Option<EndpointAddr> {
    let (relays, directs): (Vec<_>, Vec<_>) = observed
        .into_iter()
        .filter_map(|(addr, active)| active.then_some(addr))
        .filter(|addr| match addr {
            TransportAddr::Ip(socket) => {
                is_private_address(socket.ip()) && !is_virtual_nic_ip(socket.ip(), trusted)
            }
            TransportAddr::Relay(_) => true,
            _ => false,
        })
        .partition(|addr| matches!(addr, TransportAddr::Relay(_)));
    if relays.is_empty() && directs.is_empty() {
        return None;
    }
    let relays = if relays.is_empty() {
        stored
            .addrs
            .iter()
            .filter(|addr| matches!(addr, TransportAddr::Relay(_)))
            .cloned()
            .collect()
    } else {
        relays
    };
    Some(EndpointAddr::from_parts(
        stored.id,
        directs.into_iter().chain(relays),
    ))
}

pub(super) async fn observed_reusable_remote_addr(
    endpoint: &Endpoint,
    stored: &EndpointAddr,
    trusted: &TrustedNetworks,
) -> Option<EndpointAddr> {
    let info = endpoint.remote_info(stored.id).await?;
    reusable_remote_addr(
        stored,
        info.into_addrs().map(|addr| {
            let active = matches!(addr.usage(), TransportAddrUsage::Active);
            (addr.into_addr(), active)
        }),
        trusted,
    )
}

pub(super) async fn persist_observed_addr(
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
    let _ = repository
        .upsert(&PeerAddressRecord {
            device_id: device.clone(),
            addr_blob,
            observed_at,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

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

    #[test]
    fn drops_ip_entries_keeps_relay() {
        let id = test_id();
        let relay: RelayUrl = "https://relay.example.com/".parse().unwrap();

        let full = EndpointAddr::from_parts(
            id,
            [
                TransportAddr::Ip(lan_addr(50754)),
                TransportAddr::Ip(wan_addr(59875)),
                TransportAddr::Relay(relay.clone()),
            ],
        );

        let persisted = to_persistable_addr(full);
        assert_eq!(persisted.id, id);
        assert_eq!(persisted.addrs.len(), 1);
        assert!(persisted
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Relay(_))));
    }

    #[test]
    fn id_only_input_is_unchanged() {
        let id = test_id();
        let addr = EndpointAddr::new(id);
        let persisted = to_persistable_addr(addr);
        assert_eq!(persisted.id, id);
        assert!(persisted.addrs.is_empty());
    }

    #[test]
    fn ip_only_input_is_kept_intact() {
        // No relay present → no discovery fallback to rely on → keep the
        // IPs as the only dialable paths. Mirrors test fixtures that
        // bind endpoints with `RelayMode::Disabled`.
        let id = test_id();
        let addr = EndpointAddr::from_parts(id, [TransportAddr::Ip(lan_addr(50754))]);
        let persisted = to_persistable_addr(addr);
        assert_eq!(persisted.id, id);
        assert_eq!(persisted.addrs.len(), 1);
        assert!(persisted
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Ip(_))));
    }

    #[test]
    fn relay_only_input_is_unchanged() {
        let id = test_id();
        let relay: RelayUrl = "https://relay.example.com/".parse().unwrap();
        let addr = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay)]);
        let persisted = to_persistable_addr(addr);
        assert_eq!(persisted.id, id);
        assert_eq!(persisted.addrs.len(), 1);
        assert!(persisted
            .addrs
            .iter()
            .all(|a| matches!(a, TransportAddr::Relay(_))));
    }

    fn socket(raw: &str) -> TransportAddr {
        TransportAddr::Ip(raw.parse().unwrap())
    }

    fn relay(raw: &str) -> TransportAddr {
        TransportAddr::Relay(raw.parse().unwrap())
    }

    #[test]
    fn reusable_remote_addr_keeps_active_private_directs_and_active_relays() {
        let id = test_id();
        let stored = EndpointAddr::from_parts(id, [relay("https://old-relay.example.com/")]);
        let trusted = TrustedNetworks::parse(&["100.64.1.0/24"]).unwrap();

        let saved = reusable_remote_addr(
            &stored,
            [
                (socket("192.168.1.5:50754"), true),
                (socket("10.8.0.2:50754"), true),
                (socket("100.64.1.9:50754"), true),
                (socket("192.168.1.6:50754"), false),
                (relay("https://active-relay.example.com/"), true),
                (relay("https://idle-relay.example.com/"), false),
            ],
            &trusted,
        )
        .expect("active private paths are reusable");

        assert_eq!(saved.id, id);
        assert_eq!(
            saved.addrs.into_iter().collect::<Vec<_>>(),
            vec![
                relay("https://active-relay.example.com/"),
                socket("10.8.0.2:50754"),
                socket("100.64.1.9:50754"),
                socket("192.168.1.5:50754"),
            ]
        );
    }

    #[test]
    fn reusable_remote_addr_never_saves_public_loopback_link_local_or_untrusted_overlay() {
        let stored = EndpointAddr::new(test_id());
        let saved = reusable_remote_addr(
            &stored,
            [
                (socket("1.2.3.4:59875"), true),
                (socket("127.0.0.1:50754"), true),
                (socket("169.254.1.1:50754"), true),
                (socket("198.18.0.1:50754"), true),
                (socket("[fe80::1]:50754"), true),
                (socket("100.64.1.9:50754"), true),
            ],
            &TrustedNetworks::default(),
        );

        assert!(saved.is_none(), "no active path is reusable");
    }

    #[test]
    fn direct_only_observation_keeps_the_stored_relay_hint() {
        let id = test_id();
        let stored = EndpointAddr::from_parts(
            id,
            [
                socket("192.168.1.5:40000"),
                relay("https://home-relay.example.com/"),
            ],
        );

        let saved = reusable_remote_addr(
            &stored,
            [(socket("192.168.1.5:50754"), true)],
            &TrustedNetworks::default(),
        )
        .expect("an active private direct path is reusable");

        assert_eq!(
            saved.addrs.into_iter().collect::<Vec<_>>(),
            vec![
                relay("https://home-relay.example.com/"),
                socket("192.168.1.5:50754"),
            ]
        );
    }

    async fn has_active_direct_path(endpoint: &Endpoint, remote: EndpointId) -> bool {
        endpoint.remote_info(remote).await.is_some_and(|info| {
            info.addrs().any(|addr| {
                matches!(addr.addr(), TransportAddr::Ip(_))
                    && matches!(addr.usage(), TransportAddrUsage::Active)
            })
        })
    }

    /// 写回依赖的 iroh 行为：双方都把本次连接实际使用的直连路径标为正在使用，
    /// 并且连接关闭后仍保留该标记，因此同步结果提交后再观察不会丢失证据。
    #[tokio::test]
    async fn both_sides_keep_the_used_direct_path_active_after_the_connection_closes() {
        const ALPN: &[u8] = b"test/reusable-remote-addr";
        let bind = || async {
            Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_address_lookup()
                .alpns(vec![ALPN.to_vec()])
                .bind()
                .await
                .unwrap()
        };
        let client = bind().await;
        let server = bind().await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.addr().addrs.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();

        let (outgoing, incoming) = tokio::join!(client.connect(server.addr(), ALPN), async {
            server.accept().await.unwrap().await.unwrap()
        });
        let outgoing = outgoing.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !(has_active_direct_path(&client, server.id()).await
                && has_active_direct_path(&server, client.id()).await)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both sides register the used direct path");

        outgoing.close(0u32.into(), b"done");
        tokio::time::timeout(Duration::from_secs(2), incoming.closed())
            .await
            .unwrap();

        assert!(has_active_direct_path(&client, server.id()).await);
        assert!(has_active_direct_path(&server, client.id()).await);
    }

    #[tokio::test]
    async fn stable_remote_hint_is_saved_with_the_observation_time_and_failure_is_best_effort() {
        let id = test_id();
        let relay: RelayUrl = "https://active-relay.example.com/".parse().unwrap();
        let addr = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay)]);
        let device = DeviceId::new("device-b");
        let repository = Arc::new(RecordingRepository::default());

        persist_observed_addr(repository.as_ref(), &FixedClock, &device, addr.clone()).await;

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
        persist_observed_addr(&failing, &FixedClock, &device, EndpointAddr::new(id)).await;
    }
}
