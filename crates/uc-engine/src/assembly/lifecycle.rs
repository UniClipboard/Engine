//! Daemon-lifecycle composition-root entry.
//!
//! 长期网络与当前 Space 会话在这里分别装配。仓库、设置与安全存储仍在
//! 进程启动时只装配一次，由两个生命周期共同使用。

use std::sync::Arc;

use anyhow::Context as _;

use crate::assembly::deps::SyncEngineDeps;
use crate::assembly::network::load_relay_access_tokens;
use crate::assembly::sync_engine::{prepare_sync_session, PreparedSyncSession};
#[cfg(feature = "dev-tools")]
use crate::dev::JoinerFinalConfirmationGate;
use crate::subsystems::reconcile::{reconcile_peer_addresses, reconcile_trusted_peers};
use uc_application::facade::settings::AppliedRelayRouting;
use uc_application::facade::ApplicationAssembly;
use uc_core::settings::relay_routing::RelayRouting;
use uc_infra_p2p::network::iroh::{
    IrohIdentityStore, IrohNode, IrohNodeBuilder, IrohSessionBuilder,
};
use uc_infra_profile::security::Sha256IdentityFingerprintFactory;
use uc_observability_contract::{
    error_source::io_error_kind,
    log_fields::{log_vocab, log_vocab_debug},
    uc_info, uc_warn,
};

/// 建立一次 Engine 活跃期内唯一的长期网络节点。
pub async fn build_network_runtime(
    application: &ApplicationAssembly,
    space_setup: &SyncEngineDeps,
    rendezvous_base_url: Option<String>,
    relay_fallback_override: Option<bool>,
    iroh_bind_port_override: Option<u16>,
    network_partition_gate: Option<uc_infra_p2p::network::iroh::IrohNetworkPartitionGate>,
) -> anyhow::Result<IrohNode> {
    let prepared_network = application
        .prepare_network()
        .await
        .context("network settings preparation failed")?;
    let allow_relay_fallback =
        relay_fallback_override.unwrap_or(prepared_network.allow_relay_fallback);
    let allow_overlay_network_addrs = prepared_network.allow_overlay_network_addrs;
    let relay_routing =
        RelayRouting::resolve(allow_relay_fallback, &prepared_network.custom_relay_urls);
    let relay_urls = relay_routing.effective_urls(&prepared_network.custom_relay_urls);
    // 绑定成功前不得声称任何 relay 已生效；上一个节点的记录也不再有效。
    application.record_applied_relays(None);
    let applied_relays = AppliedRelayRouting {
        routing: relay_routing,
        urls: relay_urls.clone(),
    };
    let congestion_controller = prepared_network.congestion_controller;
    let mut iroh_config = crate::assembly::network::relay_policy_to_iroh_config(
        allow_relay_fallback,
        allow_overlay_network_addrs,
        relay_urls,
        congestion_controller,
        rendezvous_base_url,
    );
    // 内置 relay 不携带凭据；只有用户自定义路由才读取安全存储。
    if relay_routing == RelayRouting::Custom {
        load_relay_access_tokens(&mut iroh_config, &prepared_network.relay_credentials);
    }
    crate::assembly::network::apply_iroh_direct_reachability_from_env(&mut iroh_config);
    if let Some(port) = iroh_bind_port_override {
        iroh_config.bind_port = Some(port);
    }
    iroh_config.network_partition_gate = network_partition_gate;
    crate::assembly::network::apply_congestion_controller_from_env(&mut iroh_config);

    uc_info!(
        target: "settings.network",
        allow_relay_fallback = allow_relay_fallback,
        disable_relays = iroh_config.disable_relays,
        allow_overlay_network_addrs = iroh_config.allow_overlay_network_addrs,
        mode = log_vocab_debug(&relay_routing),
        relay_count = iroh_config.relay_urls.len(),
        congestion_controller = log_vocab(&iroh_config.congestion_controller),
        "applying network settings"
    );

    let identity_store = IrohIdentityStore::new(
        Arc::clone(&space_setup.iroh_identity_storage),
        Arc::new(Sha256IdentityFingerprintFactory),
    );
    let builder = IrohNodeBuilder::bind(&identity_store, iroh_config)
        .await
        .context("Iroh network bind failed")?;
    application.record_applied_relays(Some(applied_relays));
    Ok(builder.spawn())
}

/// 在既有长期网络上准备当前 Space 的完整能力，但暂不发布。
pub async fn prepare_daemon_session(
    application: &ApplicationAssembly,
    space_setup: &SyncEngineDeps,
    current_app_version: &str,
    #[cfg(feature = "lan-compat")] mobile_sync_ports: uc_mobile_lan::MobileSyncPorts,
    #[cfg(feature = "dev-tools")] joiner_final_confirmation_gate: Arc<JoinerFinalConfirmationGate>,
    session_builder: IrohSessionBuilder,
) -> anyhow::Result<PreparedSyncSession> {
    prepare_sync_session(
        application,
        space_setup,
        current_app_version,
        #[cfg(feature = "lan-compat")]
        mobile_sync_ports,
        #[cfg(feature = "dev-tools")]
        joiner_final_confirmation_gate,
        session_builder,
    )
    .await
    .context("Space session assembly failed")
}

/// 资料校验通过后才整理成员关系，受限恢复不触碰这些记录。
/// 清理只删除非成员的派生地址与信任条目，而连接协调只面向成员，
/// 因此在业务会话启动后执行不会与连接竞争。
pub async fn reconcile_session_peers(space_setup: &SyncEngineDeps) {
    // 启动期 reconcile:把 peer_addr_repo / trusted_peer_repo 中
    // member_repo 已不再持有的孤儿条目清掉,恢复设计意图的不变量
    // `peer_addr ⊆ member`、`trusted_peer ⊆ member`。失败只 log 不阻断
    // 启动 —— reconcile 是治理性的。
    if let Err(err) = reconcile_peer_addresses(
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_addr_repo),
    )
    .await
    {
        uc_warn!(
            error_kind = "peer_addr_reconcile",
            io_error_kind = io_error_kind(err.as_ref()),
            "peer_addr reconcile failed at boot; daemon continues with whatever orphans remain"
        );
    }
    if let Err(err) = reconcile_trusted_peers(
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.trusted_peer_repo),
    )
    .await
    {
        uc_warn!(
            error_kind = "trusted_peer_reconcile",
            io_error_kind = io_error_kind(err.as_ref()),
            "trusted_peer reconcile failed at boot; daemon continues with whatever orphans remain"
        );
    }
}
