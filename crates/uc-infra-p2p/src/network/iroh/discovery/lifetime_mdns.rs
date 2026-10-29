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
    /// 最近一次本机发布；同一把锁串行化发布与安装，安装后补发的总是最新地址。
    published: Mutex<Option<EndpointData>>,
    retry: Mutex<Option<JoinHandle<()>>>,
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
                published: Mutex::new(None),
                retry: Mutex::new(None),
            }),
        };
        match build() {
            Ok(service) => lifetime.install(service),
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

    fn install(&self, service: MdnsAddressLookup) {
        let published = self
            .inner
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(data) = published.as_ref() {
            service.publish(data);
        }
        self.inner.service.send_replace(Some(service));
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

    /// 取消重试并等待已建立的服务结束。
    pub(in super::super) async fn shutdown(&self) -> Result<(), anyhow::Error> {
        self.abort_retry();
        match self.current() {
            Some(service) => service.shutdown().await.map_err(Into::into),
            None => Ok(()),
        }
    }

    /// 不等待地取消重试与服务；用于未完整构建或被丢弃的节点。
    pub(in super::super) fn abort(&self) {
        self.abort_retry();
        if let Some(service) = self.current() {
            service.abort();
        }
    }

    fn abort_retry(&self) {
        if let Some(task) = self
            .inner
            .retry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            task.abort();
        }
    }
}

impl AddressLookup for LifetimeMdns {
    fn publish(&self, data: &EndpointData) {
        let mut published = self
            .inner
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *published = Some(data.clone());
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
                LifetimeMdns { inner }.install(service);
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
