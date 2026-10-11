use std::{io, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::{stream, StreamExt};
use tokio::time::Instant;

use super::{
    BackendId, DiscoveredPeer, DiscoveryError, DiscoveryEvent, DiscoveryManager, DiscoveryPeerId,
    DiscoveryProvider, DiscoveryProviderId, DiscoveryStream, PeerDirectory,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const MDNS: DiscoveryProviderId = DiscoveryProviderId("mdns");
const CLOUD: DiscoveryProviderId = DiscoveryProviderId("cloud");

fn peer() -> DiscoveryPeerId {
    DiscoveryPeerId::new(BackendId("test"), vec![42])
}

fn found(locator: u8, ttl: u64) -> DiscoveryEvent {
    DiscoveryEvent::Found(DiscoveredPeer {
        peer: peer(),
        locator: vec![locator],
        ttl: Duration::from_secs(ttl),
    })
}

#[tokio::test(start_paused = true)]
async fn sources_coexist_update_and_expire_independently() {
    let directory = PeerDirectory::new(8);
    directory
        .observe(MDNS, found(1, 5), Instant::now())
        .unwrap();
    directory
        .observe(CLOUD, found(2, 20), Instant::now())
        .unwrap();
    directory
        .observe(MDNS, found(3, 5), Instant::now())
        .unwrap();
    let candidates = directory.candidates(&peer(), Instant::now()).unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().any(|item| item.locator == vec![3]));
    assert!(!candidates.iter().any(|item| item.locator == vec![1]));
    tokio::time::advance(Duration::from_secs(5)).await;
    let candidates = directory.candidates(&peer(), Instant::now()).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].source, CLOUD);
    directory
        .observe(MDNS, DiscoveryEvent::Lost { peer: peer() }, Instant::now())
        .unwrap();
    assert_eq!(
        directory.candidates(&peer(), Instant::now()).unwrap().len(),
        1
    );
    tokio::time::advance(Duration::from_secs(15)).await;
    assert!(directory
        .candidates(&peer(), Instant::now())
        .unwrap()
        .is_empty());
}

#[test]
fn unknown_peers_are_bounded_and_debug_does_not_expose_material() {
    let directory = PeerDirectory::new(2);
    for value in 0..3 {
        directory
            .observe(
                MDNS,
                DiscoveryEvent::Found(DiscoveredPeer {
                    peer: DiscoveryPeerId::new(BackendId("test"), vec![value]),
                    locator: b"PRIVATE_ADDRESS".to_vec(),
                    ttl: Duration::from_secs(20),
                }),
                Instant::now(),
            )
            .unwrap();
    }
    assert_eq!(directory.snapshot(Instant::now()).unwrap().len(), 2);
    assert!(
        !format!("{:?}", directory.snapshot(Instant::now()).unwrap()).contains("PRIVATE_ADDRESS")
    );
    directory.remove_source(MDNS).unwrap();
    assert!(directory.snapshot(Instant::now()).unwrap().is_empty());
}

struct FailingCloud;

#[async_trait]
impl DiscoveryProvider for FailingCloud {
    fn id(&self) -> DiscoveryProviderId {
        CLOUD
    }
    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        Ok(None)
    }
    fn resolve(&self, _: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        Some(Box::pin(stream::once(async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Err(DiscoveryError::Provider {
                source: io::Error::other("test cloud unavailable").into(),
            })
        })))
    }
}

#[tokio::test(start_paused = true)]
async fn cached_local_candidate_precedes_slow_cloud_and_survives_its_failure() {
    let directory = Arc::new(PeerDirectory::new(8));
    directory
        .observe(MDNS, found(1, 30), Instant::now())
        .unwrap();
    let manager =
        Arc::new(DiscoveryManager::new(vec![Arc::new(FailingCloud)], directory.clone()).unwrap());
    let mut results = manager.resolve(peer());
    assert_eq!(results.next().await.unwrap().unwrap().source, MDNS);
    assert!(results.next().await.unwrap().is_err());
    assert_eq!(
        directory.candidates(&peer(), Instant::now()).unwrap().len(),
        1
    );
    manager.shutdown().unwrap();
    assert!(directory.snapshot(Instant::now()).unwrap().is_empty());
    assert!(manager.resolve(peer()).next().await.unwrap().is_err());
}

#[test]
fn duplicate_provider_ids_are_rejected() {
    let result = DiscoveryManager::new(
        vec![Arc::new(FailingCloud), Arc::new(FailingCloud)],
        Arc::new(PeerDirectory::new(8)),
    );
    assert!(matches!(result, Err(DiscoveryError::DuplicateProvider)));
}

struct DropProbe(Arc<AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct PendingProvider(Arc<AtomicUsize>);
#[async_trait]
impl DiscoveryProvider for PendingProvider {
    fn id(&self) -> DiscoveryProviderId {
        CLOUD
    }
    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        Ok(None)
    }
    fn resolve(&self, _: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let guard = DropProbe(self.0.clone());
        Some(Box::pin(stream::once(async move {
            let _guard = guard;
            std::future::pending::<Result<DiscoveryEvent, DiscoveryError>>().await
        })))
    }
}

#[tokio::test]
async fn cancelling_one_query_drops_its_provider_stream_without_stopping_another() {
    let active = Arc::new(AtomicUsize::new(0));
    let manager = Arc::new(
        DiscoveryManager::new(
            vec![Arc::new(PendingProvider(active.clone()))],
            Arc::new(PeerDirectory::new(8)),
        )
        .unwrap(),
    );
    let mut first = manager.resolve(peer());
    let mut second = manager.resolve(peer());
    assert!(tokio::time::timeout(Duration::from_millis(1), first.next())
        .await
        .is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(1), second.next())
            .await
            .is_err()
    );
    assert_eq!(active.load(Ordering::SeqCst), 2);
    drop(first);
    assert_eq!(active.load(Ordering::SeqCst), 1);
    drop(second);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[test]
fn rejected_observations_and_shutdown_cannot_repopulate_directory() {
    let directory = PeerDirectory::new(8);
    assert!(directory
        .observe(MDNS, found(1, 0), Instant::now())
        .is_err());
    directory.clear().unwrap();
    assert!(matches!(
        directory.observe(MDNS, found(1, 30), Instant::now()),
        Err(DiscoveryError::Stopped)
    ));
    assert!(directory.snapshot(Instant::now()).unwrap().is_empty());
}

#[test]
fn cloud_failure_preserves_original_source_chain() {
    let original = io::Error::new(io::ErrorKind::ConnectionRefused, "test cloud refused");
    let error = DiscoveryError::Provider {
        source: original.into(),
    };
    assert!(std::error::Error::source(&error)
        .unwrap()
        .downcast_ref::<io::Error>()
        .is_some());
}

struct PassiveProvider {
    id: DiscoveryProviderId,
    receiver: tokio::sync::Mutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<Result<DiscoveryEvent, DiscoveryError>>>,
    >,
}

#[async_trait]
impl DiscoveryProvider for PassiveProvider {
    fn id(&self) -> DiscoveryProviderId {
        self.id
    }
    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        let receiver = self.receiver.lock().await.take().unwrap();
        Ok(Some(Box::pin(stream::unfold(
            receiver,
            |mut receiver| async { receiver.recv().await.map(|item| (item, receiver)) },
        ))))
    }
    fn resolve(&self, _: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        None
    }
}

#[tokio::test]
async fn one_passive_source_ending_does_not_stop_another_and_invalid_input_is_ignored() {
    let (local_tx, local_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cloud_tx, cloud_rx) = tokio::sync::mpsc::unbounded_channel();
    let directory = Arc::new(PeerDirectory::new(8));
    let manager = Arc::new(
        DiscoveryManager::new(
            vec![
                Arc::new(PassiveProvider {
                    id: MDNS,
                    receiver: tokio::sync::Mutex::new(Some(local_rx)),
                }),
                Arc::new(PassiveProvider {
                    id: CLOUD,
                    receiver: tokio::sync::Mutex::new(Some(cloud_rx)),
                }),
            ],
            directory.clone(),
        )
        .unwrap(),
    );
    let mut changes = manager.subscribe_changes();
    let running = tokio::spawn({
        let manager = manager.clone();
        async move { manager.run().await }
    });
    local_tx.send(Ok(found(1, 20))).unwrap();
    cloud_tx.send(Ok(found(2, 20))).unwrap();
    for _ in 0..2 {
        tokio::time::timeout(Duration::from_secs(1), changes.recv())
            .await
            .unwrap()
            .unwrap();
    }
    drop(cloud_tx);
    tokio::time::timeout(Duration::from_secs(1), async {
        while directory.candidates(&peer(), Instant::now()).unwrap().len() != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    local_tx.send(Ok(found(3, 0))).unwrap();
    local_tx.send(Ok(found(4, 20))).unwrap();
    tokio::time::timeout(Duration::from_secs(1), changes.recv())
        .await
        .unwrap()
        .unwrap();
    let candidates = directory.candidates(&peer(), Instant::now()).unwrap();
    assert_eq!(candidates[0].locator, vec![4]);
    assert!(!running.is_finished());
    drop(local_tx);
    assert!(tokio::time::timeout(Duration::from_secs(1), running)
        .await
        .unwrap()
        .unwrap()
        .is_err());
    assert!(directory.snapshot(Instant::now()).unwrap().is_empty());
    manager.shutdown().unwrap();
}

#[test]
fn a_full_local_bucket_does_not_evict_cloud_observations() {
    let directory = PeerDirectory::new(1);
    directory
        .observe(CLOUD, found(1, 30), Instant::now())
        .unwrap();
    for value in 0..5 {
        directory
            .observe(
                MDNS,
                DiscoveryEvent::Found(DiscoveredPeer {
                    peer: DiscoveryPeerId::new(BackendId("test"), vec![value]),
                    locator: vec![value],
                    ttl: Duration::from_secs(30),
                }),
                Instant::now(),
            )
            .unwrap();
    }
    assert_eq!(directory.snapshot(Instant::now()).unwrap().len(), 2);
    assert!(directory
        .candidates(&peer(), Instant::now())
        .unwrap()
        .iter()
        .any(|item| item.source == CLOUD));
}
