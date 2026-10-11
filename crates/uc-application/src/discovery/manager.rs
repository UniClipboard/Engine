use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use futures::{
    stream::{self, BoxStream, SelectAll},
    StreamExt,
};
use tokio::{sync::broadcast, time::Instant};

use super::{
    ConnectionCandidate, DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProvider,
    PeerDirectory,
};

/// 唯一发现负责人；provider 同时查询，本地候选不等待云端，候选不持有逻辑设备绑定。
pub struct DiscoveryManager {
    providers: Vec<Arc<dyn DiscoveryProvider>>,
    directory: Arc<PeerDirectory>,
    stopped: AtomicBool,
    changes: broadcast::Sender<DiscoveryPeerId>,
}

impl DiscoveryManager {
    pub fn new(
        providers: Vec<Arc<dyn DiscoveryProvider>>,
        directory: Arc<PeerDirectory>,
    ) -> Result<Self, DiscoveryError> {
        let mut ids = HashSet::new();
        if providers.iter().any(|provider| !ids.insert(provider.id())) {
            return Err(DiscoveryError::DuplicateProvider);
        }
        Ok(Self {
            providers,
            directory,
            stopped: AtomicBool::new(false),
            changes: broadcast::channel(64).0,
        })
    }

    pub fn subscribe_changes(&self) -> broadcast::Receiver<DiscoveryPeerId> {
        self.changes.subscribe()
    }

    /// 每次返回当前候选并并行解析各来源；取消流不会产生分离查询任务。
    pub fn resolve(
        self: &Arc<Self>,
        peer: DiscoveryPeerId,
    ) -> BoxStream<'static, Result<ConnectionCandidate, DiscoveryError>> {
        let manager = self.clone();
        Box::pin(
            stream::once(async move {
                if manager.stopped.load(Ordering::Acquire) {
                    return Box::pin(stream::once(async { Err(DiscoveryError::Stopped) }))
                        as BoxStream<_>;
                }
                let cached = match manager.directory.candidates(&peer, Instant::now()) {
                    Ok(cached) => cached,
                    Err(error) => {
                        return Box::pin(stream::once(async { Err(error) })) as BoxStream<_>
                    }
                };
                let mut queries: SelectAll<
                    BoxStream<'static, Result<ConnectionCandidate, DiscoveryError>>,
                > = SelectAll::new();
                for provider in &manager.providers {
                    if let Some(events) = provider.resolve(&peer) {
                        let source = provider.id();
                        let directory = manager.directory.clone();
                        let manager = manager.clone();
                        let requested = peer.clone();
                        queries.push(Box::pin(events.filter_map(move |event| {
                            let result = (|| {
                                if manager.stopped.load(Ordering::Acquire) {
                                    return Err(DiscoveryError::Stopped);
                                }
                                let event = event?;
                                let observed = match &event {
                                    DiscoveryEvent::Found(found)
                                    | DiscoveryEvent::Updated(found) => &found.peer,
                                    DiscoveryEvent::Lost { peer } => peer,
                                };
                                if observed != &requested {
                                    return Err(DiscoveryError::InvalidObservation);
                                }
                                let changed = directory.observe(source, event, Instant::now())?;
                                let candidates =
                                    directory.candidates(&requested, Instant::now())?;
                                if changed {
                                    let _ = manager.changes.send(requested.clone());
                                }
                                Ok(candidates.into_iter().find(|item| item.source == source))
                            })();
                            std::future::ready(match result {
                                Ok(item) => item.map(Ok),
                                Err(error) => Some(Err(error)),
                            })
                        })));
                    }
                }
                let current = manager.clone();
                let cached = stream::iter(cached).filter_map(move |candidate| {
                    let candidate = if current.stopped.load(Ordering::Acquire) {
                        None
                    } else {
                        match current
                            .directory
                            .candidates(&candidate.peer, Instant::now())
                        {
                            Ok(valid) => valid
                                .into_iter()
                                .find(|valid| {
                                    valid.source == candidate.source
                                        && valid.locator == candidate.locator
                                })
                                .map(Ok),
                            Err(error) => Some(Err(error)),
                        }
                    };
                    std::future::ready(candidate)
                });
                Box::pin(cached.chain(queries)) as BoxStream<_>
            })
            .flatten(),
        )
    }

    /// 由设备网络运行时调用一次；订阅异常只使该来源失效，错误交给生命周期负责人。
    pub async fn run(&self) -> Result<(), DiscoveryError> {
        let mut streams = SelectAll::new();
        let mut failure = None;
        for provider in &self.providers {
            let source = provider.id();
            match provider.subscribe().await {
                Ok(Some(events)) => streams.push(
                    events
                        .map(move |event| (source, Some(event)))
                        .chain(stream::once(async move { (source, None) }))
                        .boxed(),
                ),
                Ok(None) => {}
                Err(error) => {
                    self.directory.remove_source(source)?;
                    failure = Some(error);
                }
            }
        }
        let mut expiry = tokio::time::interval(Duration::from_secs(5));
        let mut failed_sources = HashSet::new();
        loop {
            tokio::select! {
                event = streams.next() => {
                    let Some((source, event)) = event else { return failure.map_or(Ok(()), Err); };
                    if failed_sources.contains(&source) { continue; }
                    match event {
                        Some(Ok(event)) => {
                            let peer = match &event {
                                DiscoveryEvent::Found(found) | DiscoveryEvent::Updated(found) => found.peer.clone(),
                                DiscoveryEvent::Lost { peer } => peer.clone(),
                            };
                            match self.directory.observe(source, event, Instant::now()) {
                                Ok(true) => { let _ = self.changes.send(peer); },
                                Ok(false) | Err(DiscoveryError::InvalidObservation) => {},
                                Err(error) => return Err(error),
                            }
                        }
                        Some(Err(error)) => {
                            self.directory.remove_source(source)?;
                            failed_sources.insert(source);
                            failure = Some(error);
                        }
                        None => {
                            self.directory.remove_source(source)?;
                            failed_sources.insert(source);
                            failure = Some(DiscoveryError::ProviderStopped);
                        }
                    }
                }
                _ = expiry.tick() => { self.directory.snapshot(Instant::now())?; }
            }
        }
    }

    pub fn shutdown(&self) -> Result<(), DiscoveryError> {
        self.stopped.store(true, Ordering::Release);
        self.directory.clear()
    }
}
