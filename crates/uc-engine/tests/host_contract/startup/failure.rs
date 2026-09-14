use std::future::{poll_fn, Future};
use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use uc_engine::{
    Engine, EngineConfig, HostCapabilityError, HostSecureStorage, StartupLifecycle, StartupProgress,
};

use super::{host, MemorySecureStorage};

struct ObservedStorage {
    storage: MemorySecureStorage,
    dropped: Arc<AtomicBool>,
}

impl Drop for ObservedStorage {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

impl HostSecureStorage for ObservedStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        self.storage.get(key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.storage.set(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.storage.delete(key)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_service_start_releases_every_host_owner_before_returning() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = ipv6.local_addr().unwrap().port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let (progress, _) = StartupProgress::channel();
    let (lifecycle, control) = StartupLifecycle::channel();
    let mut pause = Box::pin(control.suspend());
    poll_fn(|context| {
        assert!(pause.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let result = Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host(
            root.path(),
            Box::new(ObservedStorage {
                storage: storage.clone(),
                dropped: Arc::clone(&dropped),
            }),
        ),
        progress,
        lifecycle,
    )
    .await;
    assert!(result.is_err(), "occupied network port must reject startup");
    assert_eq!(
        pause.await.unwrap_err().category(),
        result.err().unwrap().category()
    );
    assert!(
        dropped.load(Ordering::SeqCst),
        "failed startup retained a host owner"
    );
    drop(ipv4);
    drop(ipv6);
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0").with_test_iroh_bind_port(port),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}
