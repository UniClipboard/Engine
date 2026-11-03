use std::time::Duration;

use chrono::Utc;
use iroh::{endpoint::presets, Endpoint, RelayMap, RelayMode};
use serde_json::json;
use tokio::time::Instant;
use wiremock::{
    matchers::{body_json, method, path},
    Mock, MockServer, ResponseTemplate,
};

use super::{ManagedRelay, BUILTIN_RELAY_ID};

fn artifact(name: &str, value: serde_json::Value) {
    let directory = std::env::var_os("UC_MANAGED_RELAY_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/managed-relay-acceptance")
        });
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

async fn fixture() -> (MockServer, Endpoint, RelayMap, ManagedRelay) {
    let cloud = MockServer::start().await;
    let map = RelayMap::empty();
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(map.clone()))
        .bind()
        .await
        .unwrap();
    let manager = ManagedRelay::new(endpoint.id(), &cloud.uri()).unwrap();
    (cloud, endpoint, map, manager)
}

async fn catalog(cloud: &MockServer, nodes: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/nodes"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"nodes":nodes,"catalog_version":"v1"})),
        )
        .mount(cloud)
        .await;
}

fn node(id: &str, url: &str) -> serde_json::Value {
    json!({"id":id,"url":url,"region":"cn-shanghai","credential_required":true})
}

async fn credential(
    cloud: &MockServer,
    endpoint: &Endpoint,
    id: &str,
    url: &str,
    token: &str,
    wall: chrono::DateTime<Utc>,
) {
    Mock::given(method("POST")).and(path("/credentials"))
        .and(body_json(json!({"relay_id":id,"endpoint_id":hex::encode(endpoint.id().as_bytes())})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "relay_url":url,"access_token":token,"expires_at":(wall+chrono::Duration::minutes(10)).to_rfc3339()
        }))).mount(cloud).await;
}

#[tokio::test]
async fn cloud_catalog_credentials_renewal_and_removal_reconcile_real_endpoint() {
    let (cloud, endpoint, map, mut manager) = fixture().await;
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = recorded.clone();
    manager.applied = Some(std::sync::Arc::new(move |urls| {
        observed.lock().unwrap().push(urls)
    }));
    let now = Instant::now();
    let wall = Utc::now();
    catalog(
        &cloud,
        json!([
            node("other", "https://other.invalid"),
            node(BUILTIN_RELAY_ID, "https://catalog.invalid")
        ]),
    )
    .await;
    credential(
        &cloud,
        &endpoint,
        BUILTIN_RELAY_ID,
        "https://credential.invalid",
        "PRIVATE_FIRST",
        wall,
    )
    .await;
    manager.tick(&endpoint, now, wall).await;
    let first = "https://credential.invalid/".parse().unwrap();
    assert_eq!(map.len(), 1);
    assert_eq!(
        map.get(&first).unwrap().auth_token.as_deref(),
        Some("PRIVATE_FIRST")
    );
    assert!(!format!("{manager:?} {map:?}").contains("PRIVATE_FIRST"));
    cloud.reset().await;
    let renewed_wall = wall + chrono::Duration::seconds(479);
    catalog(
        &cloud,
        json!([node(BUILTIN_RELAY_ID, "https://catalog-updated.invalid")]),
    )
    .await;
    credential(
        &cloud,
        &endpoint,
        BUILTIN_RELAY_ID,
        "https://renewed.invalid",
        "PRIVATE_SECOND",
        renewed_wall,
    )
    .await;
    manager
        .tick(&endpoint, now + Duration::from_secs(479), renewed_wall)
        .await;
    assert!(map.get(&first).is_none());
    assert_eq!(map.len(), 1);
    assert_eq!(
        map.get(&"https://renewed.invalid/".parse().unwrap())
            .unwrap()
            .auth_token
            .as_deref(),
        Some("PRIVATE_SECOND")
    );
    cloud.reset().await;
    catalog(&cloud, json!([])).await;
    manager
        .tick(
            &endpoint,
            now + Duration::from_secs(510),
            wall + chrono::Duration::seconds(510),
        )
        .await;
    assert!(map.is_empty());
    let snapshots = recorded.lock().unwrap().clone();
    assert_eq!(
        snapshots.first().unwrap(),
        &vec!["https://credential.invalid/".to_owned()]
    );
    assert!(snapshots.last().unwrap().is_empty());
    assert_eq!(cloud.received_requests().await.unwrap().len(), 1);
    endpoint.close().await;
    artifact(
        "control-plane",
        json!({
            "shanghai_selected":true,"credential_url_authoritative":true,"renewal":true,"catalog_removal":true,"redacted":true
        }),
    );
}

#[tokio::test]
async fn outage_retains_catalog_but_never_reuses_a_credential_after_failed_renewal() {
    let (cloud, endpoint, map, mut manager) = fixture().await;
    let now = Instant::now();
    let wall = Utc::now();
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&cloud)
        .await;
    credential(
        &cloud,
        &endpoint,
        BUILTIN_RELAY_ID,
        "https://bootstrap.invalid",
        "PRIVATE_BOOTSTRAP",
        wall,
    )
    .await;
    manager.tick(&endpoint, now, wall).await;
    assert_eq!(map.len(), 1);
    cloud.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&cloud)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "90"))
        .mount(&cloud)
        .await;
    manager
        .tick(
            &endpoint,
            now + Duration::from_secs(479),
            wall + chrono::Duration::seconds(479),
        )
        .await;
    assert!(map.is_empty());
    let count = cloud.received_requests().await.unwrap().len();
    manager
        .tick(
            &endpoint,
            now + Duration::from_secs(480),
            wall + chrono::Duration::seconds(480),
        )
        .await;
    assert_eq!(cloud.received_requests().await.unwrap().len(), count);
    assert_eq!(manager.nodes[0].id, BUILTIN_RELAY_ID);
    endpoint.close().await;
    artifact(
        "outage",
        json!({"catalog_retained":true,"failed_renewal_withdrawn":true,"retry_after":true}),
    );
}

#[tokio::test]
async fn new_endpoint_identity_requires_its_own_credential() {
    let (cloud, first, first_map, mut manager) = fixture().await;
    let now = Instant::now();
    let wall = Utc::now();
    catalog(
        &cloud,
        json!([node(BUILTIN_RELAY_ID, "https://catalog.invalid")]),
    )
    .await;
    credential(
        &cloud,
        &first,
        BUILTIN_RELAY_ID,
        "https://relay.invalid",
        "PRIVATE_IDENTITY",
        wall,
    )
    .await;
    manager.tick(&first, now, wall).await;
    assert_eq!(first_map.len(), 1);
    first.close().await;
    let second_map = RelayMap::empty();
    let second = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(second_map.clone()))
        .bind()
        .await
        .unwrap();
    assert_ne!(first.id(), second.id());
    let mut second_manager = ManagedRelay::new(second.id(), &cloud.uri()).unwrap();
    second_manager.tick(&second, now, wall).await;
    assert!(second_map.is_empty());
    second.close().await;
    artifact(
        "identity",
        json!({"new_identity_requires_new_credential":true}),
    );
}

#[tokio::test]
async fn blocked_unavailable_redirected_and_invalid_credentials_never_authorize_relay() {
    for status in [403, 503, 302] {
        let (cloud, endpoint, map, mut manager) = fixture().await;
        catalog(
            &cloud,
            json!([node(BUILTIN_RELAY_ID, "https://catalog.invalid")]),
        )
        .await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("Location", "https://elsewhere.invalid"),
            )
            .mount(&cloud)
            .await;
        let now = Instant::now();
        manager.tick(&endpoint, now, Utc::now()).await;
        manager
            .tick(&endpoint, now + Duration::from_secs(1), Utc::now())
            .await;
        assert!(map.is_empty());
        assert_eq!(cloud.received_requests().await.unwrap().len(), 2);
        endpoint.close().await;
    }
    for url in [
        "http://relay.invalid",
        "https://relay.invalid/?token=PRIVATE",
        "https://user:PRIVATE@relay.invalid",
        "https://relay.invalid/path",
    ] {
        let (cloud, endpoint, map, mut manager) = fixture().await;
        catalog(
            &cloud,
            json!([node(BUILTIN_RELAY_ID, "https://catalog.invalid")]),
        )
        .await;
        credential(
            &cloud,
            &endpoint,
            BUILTIN_RELAY_ID,
            url,
            "PRIVATE_INVALID",
            Utc::now(),
        )
        .await;
        manager.tick(&endpoint, Instant::now(), Utc::now()).await;
        assert!(map.is_empty());
        endpoint.close().await;
    }
    artifact(
        "invalid-credentials",
        json!({"blocked":true,"unavailable":true,"redirect_rejected":true,"unsafe_urls_rejected":true}),
    );
}

#[tokio::test]
async fn malformed_catalogs_preserve_last_valid_selection_and_reject_unsafe_credentials() {
    for malformed in [
        json!({"nodes":[node("duplicate", "https://a.invalid"),node("duplicate", "https://b.invalid")],"catalog_version":"v2"}),
        json!({"nodes":[node("invalid", "http://a.invalid")],"catalog_version":"v2"}),
        json!({"nodes":vec![node("many", "https://a.invalid");33],"catalog_version":"v2"}),
    ] {
        let (cloud, endpoint, map, mut manager) = fixture().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(malformed))
            .mount(&cloud)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&cloud)
            .await;
        manager.tick(&endpoint, Instant::now(), Utc::now()).await;
        assert_eq!(manager.nodes[0].id, BUILTIN_RELAY_ID);
        assert!(map.is_empty());
        endpoint.close().await;
    }
    for (token, expires_at) in [
        (
            "".to_owned(),
            (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        ),
        (
            "PRIVATE_EXPIRED".to_owned(),
            (Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
        ),
        ("PRIVATE_BAD_TIME".to_owned(), "not-a-time".to_owned()),
        (
            "x".repeat(4097),
            (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        ),
    ] {
        let (cloud, endpoint, map, mut manager) = fixture().await;
        catalog(
            &cloud,
            json!([node(BUILTIN_RELAY_ID, "https://catalog.invalid")]),
        )
        .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "relay_url":"https://relay.invalid","access_token":token,"expires_at":expires_at
            })))
            .mount(&cloud)
            .await;
        manager.tick(&endpoint, Instant::now(), Utc::now()).await;
        assert!(map.is_empty());
        endpoint.close().await;
    }
    artifact(
        "invalid-responses",
        json!({"catalog_validation":true,"credential_validation":true}),
    );
}

#[tokio::test]
async fn catalog_replaces_bootstrap_identity_and_successful_empty_catalog_stays_empty_during_outage(
) {
    let (cloud, endpoint, map, mut manager) = fixture().await;
    let now = Instant::now();
    let wall = Utc::now();
    catalog(
        &cloud,
        json!([node("replacement", "https://replacement.invalid")]),
    )
    .await;
    credential(
        &cloud,
        &endpoint,
        "replacement",
        "https://replacement.invalid",
        "PRIVATE_REPLACEMENT",
        wall,
    )
    .await;
    manager.tick(&endpoint, now, wall).await;
    assert_eq!(manager.nodes[0].id, "replacement");
    assert_eq!(map.len(), 1);
    cloud.reset().await;
    catalog(&cloud, json!([])).await;
    manager
        .tick(
            &endpoint,
            now + Duration::from_secs(30),
            wall + chrono::Duration::seconds(30),
        )
        .await;
    assert!(map.is_empty());
    cloud.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&cloud)
        .await;
    manager
        .tick(
            &endpoint,
            now + Duration::from_secs(60),
            wall + chrono::Duration::seconds(60),
        )
        .await;
    assert!(manager.nodes.is_empty());
    assert!(map.is_empty());
    assert_eq!(cloud.received_requests().await.unwrap().len(), 1);
    endpoint.close().await;
    artifact(
        "catalog-replacement",
        json!({"bootstrap_replaced":true,"empty_catalog_retained_during_outage":true}),
    );
}

#[tokio::test]
async fn cloud_outage_preserves_real_direct_data_exchange() {
    let (cloud, endpoint, map, mut manager) = fixture().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&cloud)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&cloud)
        .await;
    manager.tick(&endpoint, Instant::now(), Utc::now()).await;
    assert!(map.is_empty());
    let peer = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![b"managed-relay/direct/1".to_vec()])
        .bind()
        .await
        .unwrap();
    let (outgoing, incoming) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            endpoint.connect(peer.addr(), b"managed-relay/direct/1"),
            async { peer.accept().await.unwrap().await.unwrap() }
        )
    })
    .await
    .unwrap();
    let outgoing = outgoing.unwrap();
    let (sent, received) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            async {
                let mut stream = outgoing.open_uni().await.unwrap();
                stream
                    .write_all(b"direct path remains available")
                    .await
                    .unwrap();
                stream.finish().unwrap();
                stream.stopped().await.unwrap();
            },
            async {
                incoming
                    .accept_uni()
                    .await
                    .unwrap()
                    .read_to_end(128)
                    .await
                    .unwrap()
            }
        )
    })
    .await
    .unwrap();
    assert_eq!(sent, ());
    assert_eq!(received, b"direct path remains available");
    endpoint.close().await;
    peer.close().await;
    artifact(
        "direct-outage",
        json!({"cloud_unavailable":true,"direct_authenticated_exchange":true,"payload_verified":true}),
    );
}

#[tokio::test]
async fn wall_clock_after_suspend_forces_credential_renewal_before_monotonic_deadline() {
    let (cloud, endpoint, map, mut manager) = fixture().await;
    let now = Instant::now();
    let wall = Utc::now();
    catalog(
        &cloud,
        json!([node(BUILTIN_RELAY_ID, "https://catalog.invalid")]),
    )
    .await;
    credential(
        &cloud,
        &endpoint,
        BUILTIN_RELAY_ID,
        "https://relay.invalid",
        "PRIVATE_BEFORE_SLEEP",
        wall,
    )
    .await;
    manager.tick(&endpoint, now, wall).await;
    cloud.reset().await;
    let resumed = wall + chrono::Duration::minutes(15);
    credential(
        &cloud,
        &endpoint,
        BUILTIN_RELAY_ID,
        "https://relay.invalid",
        "PRIVATE_AFTER_SLEEP",
        resumed,
    )
    .await;
    manager
        .tick(&endpoint, now + Duration::from_secs(1), resumed)
        .await;
    assert_eq!(
        map.get(&"https://relay.invalid/".parse().unwrap())
            .unwrap()
            .auth_token
            .as_deref(),
        Some("PRIVATE_AFTER_SLEEP")
    );
    endpoint.close().await;
    artifact("suspend-expiry", json!({"wall_clock_renewal":true}));
}
