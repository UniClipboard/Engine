use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use futures_util::StreamExt;
use iroh::{
    address_lookup::{AddressLookup, AddressLookupBuilderError, EndpointData},
    SecretKey, TransportAddr,
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use uc_observability_contract::diagnostics::connectivity::NetworkRecorder;

use super::LifetimeMdns;

fn service_name() -> String {
    format!("uni-test-{}", uuid::Uuid::new_v4().simple())
}

/// 启动时无法建立的 mDNS 在后台重试；建立前的本机发布在建立后补发，建立前的订阅开始收到观察。
#[tokio::test]
async fn failed_start_is_retried_and_replays_the_latest_publish() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let service = service_name();
    let id = SecretKey::generate().public();
    let attempts = Arc::new(AtomicUsize::new(0));
    let lifetime = LifetimeMdns::start_with_backoff(
        {
            let service = service.clone();
            let attempts = attempts.clone();
            move || {
                if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                    return Err(AddressLookupBuilderError::from_err(
                        "mdns",
                        std::io::Error::other("PRIVATE 192.0.2.7:5353"),
                    ));
                }
                MdnsAddressLookup::builder()
                    .service_name(service.clone())
                    .build(id)
            }
        },
        NetworkRecorder::current(),
        Duration::from_millis(20),
        Duration::from_millis(40),
    );
    assert!(!lifetime.is_available());
    assert!(lifetime.resolve(id).is_none(), "建立前没有解析能力");
    let announced = "192.0.2.10:4242".parse().unwrap();
    lifetime.publish(&EndpointData::from_iter([TransportAddr::Ip(announced)]));

    let observer = MdnsAddressLookup::builder()
        .service_name(service)
        .advertise(false)
        .build(SecretKey::generate().public())
        .unwrap();
    let mut discovered = observer.subscribe().await;
    let mut own_events = lifetime.events();

    tokio::time::timeout(Duration::from_secs(5), async {
        while !lifetime.is_available() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("后台重试必须建立 mDNS");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);

    let event = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match discovered.next().await {
                Some(DiscoveryEvent::Discovered { endpoint_info, .. })
                    if endpoint_info.endpoint_id == id =>
                {
                    return endpoint_info;
                }
                Some(_) => {}
                None => panic!("观察流提前结束"),
            }
        }
    })
    .await
    .expect("建立前的发布必须在建立后补发");
    assert!(event
        .to_endpoint_addr()
        .ip_addrs()
        .any(|addr| addr == &announced));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), own_events.next())
            .await
            .is_err(),
        "建立前创建的订阅保持打开，不因等待建立而结束"
    );

    lifetime.shutdown().await.unwrap();
    observer.shutdown().await.unwrap();
    assert_eq!(
        logs.count(
            "local peer discovery unavailable; continuing without mDNS and retrying in the background"
        ),
        1
    );
    assert_eq!(
        logs.count("local peer discovery recovered; mDNS is available"),
        1
    );
    assert!(!logs.output().contains("192.0.2.7"));
    assert!(!logs.output().contains("PRIVATE"));
}

/// 关闭取消尚未成功的重试，之后不再尝试建立。
#[tokio::test]
async fn shutdown_cancels_a_pending_retry() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let lifetime = LifetimeMdns::start_with_backoff(
        {
            let attempts = attempts.clone();
            move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(AddressLookupBuilderError::from_err(
                    "mdns",
                    std::io::Error::other("unavailable"),
                ))
            }
        },
        NetworkRecorder::current(),
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    lifetime.shutdown().await.unwrap();
    let after_shutdown = attempts.load(Ordering::SeqCst);
    assert!(after_shutdown >= 2, "关闭前应已重试");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(attempts.load(Ordering::SeqCst), after_shutdown);
    assert!(!lifetime.is_available());
}
