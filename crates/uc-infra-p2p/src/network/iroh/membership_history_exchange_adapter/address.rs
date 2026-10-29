use super::super::peer_address_resolver::PeerAddressResolver;
use super::super::persistable_addr::{
    observed_refreshed_remote_addr, persist_observed_stable_addr,
};
use async_trait::async_trait;
use iroh::Endpoint;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use uc_application::deps::RefreshVerifiedPeerAddressPort;
use uc_core::ids::DeviceId;
use uc_core::ports::{ClockPort, PeerAddressRepositoryPort};
use uc_observability_contract::uc_warn;

/// 已验证对端地址刷新的唯一负责人，成员历史与 presence 两条路径共用同一实例。
///
/// 同一对端的刷新串行执行并合并：最多一次正在运行、一次待运行。运行期间到达的请求只标记待运行
/// 并立即返回，当前一次结束后再按最新路径刷新一次，因此较早的刷新不会覆盖较新的地址，未完成的
/// 工作也不会随联系次数增长。
pub struct IrohVerifiedPeerAddressRefresh {
    endpoint: Arc<Endpoint>,
    resolver: PeerAddressResolver,
    repository: Arc<dyn PeerAddressRepositoryPort>,
    clock: Arc<dyn ClockPort>,
    /// 正在刷新的对端；值表示运行期间是否又收到请求。
    in_flight: Mutex<HashMap<DeviceId, bool>>,
}
impl IrohVerifiedPeerAddressRefresh {
    pub fn new(
        endpoint: Arc<Endpoint>,
        repository: Arc<dyn PeerAddressRepositoryPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            endpoint,
            resolver: PeerAddressResolver::new(repository.clone()),
            repository,
            clock,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    async fn refresh_once(&self, peer: &DeviceId) {
        let address = match self.resolver.resolve(peer).await {
            Ok(Some(address)) => address,
            Ok(None) => return,
            Err(source) => {
                uc_warn!(error_kind = source.kind(), "成员历史验证后地址读取失败");
                return;
            }
        };
        // 合并而不是替换：使用中的路径是新证据，未被否定的旧直连线索继续保留。
        if let Some(observed) = observed_refreshed_remote_addr(&self.endpoint, &address).await {
            persist_observed_stable_addr(
                self.repository.as_ref(),
                self.clock.as_ref(),
                peer,
                observed,
            )
            .await;
        }
    }

    fn in_flight(&self) -> std::sync::MutexGuard<'_, HashMap<DeviceId, bool>> {
        self.in_flight
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// 运行中的刷新被取消（调用方超时或关闭）时释放该对端，后续请求可以重新开始。
struct Running<'a> {
    owner: &'a IrohVerifiedPeerAddressRefresh,
    peer: DeviceId,
    finished: bool,
}
impl Drop for Running<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.owner.in_flight().remove(&self.peer);
        }
    }
}

#[async_trait]
impl RefreshVerifiedPeerAddressPort for IrohVerifiedPeerAddressRefresh {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId) {
        {
            let mut in_flight = self.in_flight();
            if let Some(pending) = in_flight.get_mut(peer) {
                *pending = true;
                return;
            }
            in_flight.insert(*peer, false);
        }
        let mut running = Running {
            owner: self,
            peer: *peer,
            finished: false,
        };
        loop {
            self.refresh_once(peer).await;
            // 是否再运行一次与释放该对端在同一把锁内决定，不会漏掉期间到达的请求。
            let mut in_flight = self.in_flight();
            match in_flight.get_mut(peer) {
                Some(pending) if *pending => *pending = false,
                _ => {
                    in_flight.remove(peer);
                    running.finished = true;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
