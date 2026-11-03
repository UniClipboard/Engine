use super::{tests::identity_store, IrohNodeBuilder, IrohNodeConfig};

#[tokio::test]
async fn custom_and_lan_only_nodes_do_not_start_managed_relay_runtime() {
    for config in [
        IrohNodeConfig {
            disable_relays: true,
            ..Default::default()
        },
        IrohNodeConfig {
            managed_relays: false,
            relay_urls: vec!["http://127.0.0.1:1".into()],
            ..Default::default()
        },
    ] {
        let builder = IrohNodeBuilder::bind(&identity_store(), config)
            .await
            .unwrap();
        assert!(builder.managed_relay.is_none());
        let node = builder.spawn();
        assert!(node.managed_relay.is_none());
        node.shutdown().await.unwrap();
    }
    artifact("routing-isolation");
}

#[tokio::test]
async fn managed_runtime_is_owned_and_joined_by_node_shutdown() {
    let builder = IrohNodeBuilder::bind(&identity_store(), IrohNodeConfig::default())
        .await
        .unwrap();
    assert!(builder.managed_relay.is_some());
    let node = builder.spawn();
    assert!(node.managed_relay.is_some());
    node.shutdown().await.unwrap();
    artifact("lifecycle");
}

#[tokio::test]
#[ignore = "requires live anonymous Cloud issuance and the managed public relay"]
async fn live_managed_relay_accepts_current_endpoint_credential() {
    use iroh::Watcher as _;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let installed = Arc::new(AtomicBool::new(false));
    let observed = installed.clone();
    let mut builder = IrohNodeBuilder::bind(&identity_store(), IrohNodeConfig::default())
        .await
        .unwrap();
    builder.track_managed_relays(Arc::new(move |urls| {
        if !urls.is_empty() {
            observed.store(true, Ordering::SeqCst);
        }
    }));
    let node = builder.spawn();
    let connected = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        node.session_context.endpoint.online(),
    )
    .await
    .is_ok();
    let status = node.session_context.endpoint.home_relay_status().get();
    let relay_status_count = status.len();
    let auth_denied = status
        .iter()
        .any(|status| status.auth_denied_reason().is_some());
    let relay_error = status.iter().any(|status| status.last_error().is_some());
    node.shutdown().await.unwrap();
    let directory = std::env::var_os("UC_MANAGED_RELAY_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/managed-relay-acceptance")
        });
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("live.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "anonymous_credential_and_relay_connection":connected,"credential_installed":installed.load(Ordering::SeqCst),"relay_status_count":relay_status_count,"auth_denied":auth_denied,"relay_error":relay_error,"shutdown_joined":true
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(
        connected,
        "live managed relay did not accept a connection within the acceptance budget"
    );
}

fn artifact(name: &str) {
    let directory = std::env::var_os("UC_MANAGED_RELAY_ARTIFACT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/managed-relay-acceptance")
        });
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{name}.json")),
        b"{\"passed\":true}\n",
    )
    .unwrap();
}
