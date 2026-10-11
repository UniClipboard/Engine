//! 迁移原 lan_only_relay_mode 的来源数量检查：验证实际节点，避免镜像 builder 掩盖接线错误。

use super::{tests::identity_store, IrohNodeBuilder, IrohNodeConfig};

#[tokio::test]
async fn lan_only_node_registers_directory_without_cloud_publisher() {
    let builder = IrohNodeBuilder::bind(
        &identity_store(),
        IrohNodeConfig {
            disable_relays: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        builder
            .session_context
            .endpoint
            .address_lookup()
            .unwrap()
            .len(),
        1
    );
    builder.spawn().shutdown().await.unwrap();
}

#[tokio::test]
async fn cloud_enabled_node_registers_directory_and_existing_pkarr_publisher() {
    let builder = IrohNodeBuilder::bind(
        &identity_store(),
        IrohNodeConfig {
            relay_urls: vec!["http://127.0.0.1:1".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        builder
            .session_context
            .endpoint
            .address_lookup()
            .unwrap()
            .len(),
        2
    );
    builder.spawn().shutdown().await.unwrap();
}
