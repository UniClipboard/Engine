//! 长期 mDNS 实例的唯一持有者。
//!
//! 启动时组播套接字无法建立（例如网卡尚未就绪）不再让本机发现一直关闭到重启：
//! endpoint 照常启动，后台按 1 秒起、每次翻倍、最长 60 秒的有界退避重新建立，
//! 与 swarm-discovery 的接口安装重试采用同一节奏。建立成功后补发最近一次本机发布的
//! 地址，已存在的目录订阅从此开始收到观察。重试任务由本对象持有，关闭时取消。

use std::{
    fmt,
    sync::{Arc, Mutex, PoisonError, Weak},
    time::Duration,
};

use futures_util::{stream::BoxStream, StreamExt};
use iroh::{
    address_lookup::{AddressLookup, AddressLookupBuilderError, EndpointData, Error, Item},
    EndpointId,
};
use iroh_mdns_address_lookup::{DiscoveryEvent as MdnsEvent, MdnsAddressLookup};
use tokio::{sync::watch, task::JoinHandle};
use uc_observability_contract::diagnostics::connectivity::{
    LocalDiagnosticSource, NetworkRecorder, SourceCapability, SourceCollection,
};
use uc_observability_contract::{error_source::io_error_kind, uc_info, uc_warn};

/// 首次重试延迟。
const RETRY_INITIAL: Duration = Duration::from_secs(1);
/// 最大重试延迟；持续失败时最多按此间隔尝试。
const RETRY_MAX: Duration = Duration::from_secs(60);

type Build = Box<dyn FnMut() -> Result<MdnsAddressLookup, AddressLookupBuilderError> + Send>;

/// 可克隆的长期 mDNS 句柄；未建立时发布只记录最新地址，解析返回 `None`。
#[derive(Clone)]
pub(in super::super) struct LifetimeMdns {
    inner: Arc<Inner>,
}

struct Inner {
    service: watch::Sender<Option<MdnsAddressLookup>>,
    /// 同一把锁串行化发布、安装与停止：安装后补发的总是最新地址，停止后不再安装。
    state: Mutex<State>,
    retry: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct State {
    /// 最近一次本机发布。
    published: Option<EndpointData>,
    /// 已关闭或中止；此后建立的服务立即中止，不再安装。
    stopped: bool,
}

impl fmt::Debug for LifetimeMdns {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LifetimeMdns")
    }
}

impl LifetimeMdns {
    /// 立即尝试建立一次；失败时只记录一次固定警告并在后台按退避重试。
    pub(in super::super) fn start(
        build: impl FnMut() -> Result<MdnsAddressLookup, AddressLookupBuilderError> + Send + 'static,
        recorder: NetworkRecorder,
    ) -> Self {
        Self::start_with_backoff(build, recorder, RETRY_INITIAL, RETRY_MAX)
    }

    pub(super) fn start_with_backoff(
        build: impl FnMut() -> Result<MdnsAddressLookup, AddressLookupBuilderError> + Send + 'static,
        recorder: NetworkRecorder,
        initial: Duration,
        max: Duration,
    ) -> Self {
        let mut build: Build = Box::new(build);
        let lifetime = Self {
            inner: Arc::new(Inner {
                service: watch::channel(None).0,
                state: Mutex::new(State::default()),
                retry: Mutex::new(None),
            }),
        };
        match build() {
            Ok(service) => {
                lifetime.install(service);
            }
            Err(error) => {
                uc_warn!(
                    error_kind = "mdns_unavailable",
                    io_error_kind = io_error_kind(&error),
                    "local peer discovery unavailable; continuing without mDNS and retrying in the background"
                );
                let weak = Arc::downgrade(&lifetime.inner);
                let task = tokio::spawn(retry(weak, build, recorder, initial, max));
                *lifetime
                    .inner
                    .retry
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(task);
            }
        }
        lifetime
    }

    pub(in super::super) fn is_available(&self) -> bool {
        self.inner.service.borrow().is_some()
    }

    fn current(&self) -> Option<MdnsAddressLookup> {
        self.inner.service.borrow().clone()
    }

    /// 安装建立好的服务；已停止时中止该服务并返回 `false`。
    fn install(&self, service: MdnsAddressLookup) -> bool {
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.stopped {
            drop(state);
            service.abort();
            return false;
        }
        if let Some(data) = state.published.as_ref() {
            service.publish(data);
        }
        self.inner.service.send_replace(Some(service));
        true
    }

    /// 发现事件流：未建立时等待建立，之后转交原服务的订阅。
    pub(in super::super) fn events(&self) -> BoxStream<'static, MdnsEvent> {
        let mut installed = self.inner.service.subscribe();
        Box::pin(
            futures_util::stream::once(async move {
                // 发送端随本对象释放时流直接结束，由目录负责人按来源停止处理。
                let service = installed
                    .wait_for(Option::is_some)
                    .await
                    .ok()
                    .and_then(|service| service.clone());
                match service {
                    Some(service) => service.subscribe().await.boxed(),
                    None => futures_util::stream::empty().boxed(),
                }
            })
            .flatten(),
        )
    }

    /// 停止并取消重试，等待重试任务真正退出（包括正在执行的建立），再等待已安装的服务结束。
    pub(in super::super) async fn shutdown(&self) -> Result<(), anyhow::Error> {
        if let Some(task) = self.stop() {
            task.abort();
            match task.await {
                Err(error) if error.is_panic() => return Err(error.into()),
                Ok(()) | Err(_) => {}
            }
        }
        match self.current() {
            Some(service) => service.shutdown().await.map_err(Into::into),
            None => Ok(()),
        }
    }

    /// 不等待地停止重试与服务；用于未完整构建或被丢弃的节点。仍在建立的服务由停止标记拦下。
    pub(in super::super) fn abort(&self) {
        if let Some(task) = self.stop() {
            task.abort();
        }
        if let Some(service) = self.current() {
            service.abort();
        }
    }

    /// 标记停止并取出重试任务；标记与安装共用一把锁，此后完成的建立不会再安装。
    fn stop(&self) -> Option<JoinHandle<()>> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stopped = true;
        self.inner
            .retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl AddressLookup for LifetimeMdns {
    fn publish(&self, data: &EndpointData) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.published = Some(data.clone());
        if let Some(service) = self.current() {
            service.publish(data);
        }
    }

    fn resolve(&self, endpoint: EndpointId) -> Option<BoxStream<'static, Result<Item, Error>>> {
        self.current()?.resolve(endpoint)
    }
}

async fn retry(
    lifetime: Weak<Inner>,
    mut build: Build,
    recorder: NetworkRecorder,
    initial: Duration,
    max: Duration,
) {
    let mut delay = initial;
    loop {
        tokio::time::sleep(delay).await;
        let Some(inner) = lifetime.upgrade() else {
            return;
        };
        match build() {
            Ok(service) => {
                if !(LifetimeMdns { inner }).install(service) {
                    return;
                }
                recorder.register_source(
                    LocalDiagnosticSource::MdnsDiscovery,
                    SourceCapability::Partial,
                    SourceCollection::Enabled,
                );
                uc_info!("local peer discovery recovered; mDNS is available");
                return;
            }
            Err(_) => delay = delay.saturating_mul(2).min(max),
        }
    }
}

#[cfg(test)]
mod tests;
