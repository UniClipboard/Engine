//! 邀请解析只负责把短码或完整邀请还原为新准入邀请。
//!
//! 该适配器不建立业务 session，也不注册 ALPN。准入消息统一交给
//! `/uniclipboard/space-admission/1` transport。

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use iroh::{Endpoint, EndpointAddr};
use tracing::debug;
use uc_application::deps::{ResolveJoinerInvitationError, ResolveJoinerInvitationPort};
use uc_core::membership::AdmissionShortInvitationCode;
use uc_core::pairing::invitation::FullInvitation;

use crate::rendezvous::{RendezvousClient, RendezvousHttpError};

/// 通过完整邀请、rendezvous 或局域网 mDNS 解析新准入邀请。
pub struct PairingInvitationResolverAdapter {
    endpoint: Arc<Endpoint>,
    rendezvous: Arc<RendezvousClient>,
}

impl PairingInvitationResolverAdapter {
    pub fn new(endpoint: Arc<Endpoint>, rendezvous: Arc<RendezvousClient>) -> Self {
        Self {
            endpoint,
            rendezvous,
        }
    }

    async fn resolve(&self, code: &str) -> anyhow::Result<FullInvitation> {
        if code.starts_with("ucspace1_") {
            let invitation = FullInvitation::new(code.to_owned())
                .map_err(anyhow::Error::new)
                .context("解析完整 Space 邀请失败")?;
            validate_invitation_route(invitation.as_str())?;
            return Ok(invitation);
        }

        if crate::network::iroh::runtime_consts::lan_only() {
            debug!("LAN-only 模式只通过 mDNS 解析邀请");
            return self.resolve_via_mdns(code).await;
        }

        let cloud = self.resolve_via_cloud(code);
        let lan = self.resolve_via_mdns(code);
        tokio::pin!(cloud);
        tokio::pin!(lan);

        match futures_util::future::select(cloud, lan).await {
            futures_util::future::Either::Left((Ok(invitation), _))
            | futures_util::future::Either::Right((Ok(invitation), _)) => Ok(invitation),
            futures_util::future::Either::Left((Err(_), pending)) => pending.await,
            futures_util::future::Either::Right((Err(_), pending)) => pending.await,
        }
    }

    async fn resolve_via_cloud(&self, code: &str) -> anyhow::Result<FullInvitation> {
        let response = self
            .rendezvous
            .resolve_pairing(code)
            .await
            .map_err(map_rendezvous_error)?;
        let invitation = FullInvitation::new(response.sponsor_ticket)
            .map_err(anyhow::Error::new)
            .context("解析 rendezvous Space 邀请失败")?;
        validate_invitation_route(invitation.as_str())?;
        Ok(invitation)
    }

    async fn resolve_via_mdns(&self, code: &str) -> anyhow::Result<FullInvitation> {
        let ticket = crate::pairing::MdnsPairingResolver::resolve(
            &tokio::runtime::Handle::current(),
            &self.endpoint.id().to_string(),
            code,
            Duration::from_secs(5),
        )
        .await
        .map_err(anyhow::Error::new)
        .context("通过 mDNS 解析 Space 邀请失败")?
        .ok_or_else(|| anyhow::anyhow!("mDNS 未找到 Space 邀请"))?;
        let bytes = hex::decode(ticket).context("解码 mDNS Space 邀请失败")?;
        let encoded = std::str::from_utf8(&bytes).context("读取 mDNS Space 邀请失败")?;
        let invitation = FullInvitation::new(encoded.to_owned())
            .map_err(anyhow::Error::new)
            .context("解析 mDNS Space 邀请失败")?;
        validate_invitation_route(invitation.as_str())?;
        Ok(invitation)
    }
}

#[async_trait]
impl ResolveJoinerInvitationPort for PairingInvitationResolverAdapter {
    async fn resolve_once(
        &self,
        short_code: &AdmissionShortInvitationCode,
    ) -> Result<FullInvitation, ResolveJoinerInvitationError> {
        let code = std::str::from_utf8(short_code.as_bytes()).map_err(|source| {
            ResolveJoinerInvitationError::unavailable(anyhow::Error::new(source))
        })?;
        self.resolve(code)
            .await
            .map_err(ResolveJoinerInvitationError::unavailable)
    }
}

fn validate_invitation_route(invitation: &str) -> anyhow::Result<()> {
    let decoded =
        crate::space::decode_invitation_entry(invitation, chrono::Utc::now().timestamp_millis())
            .map_err(anyhow::Error::new)
            .context("验证 Space 邀请失败")?
            .ok_or_else(|| anyhow::anyhow!("Space 邀请不可用"))?;
    serde_json::from_slice::<EndpointAddr>(decoded.route()).context("解析 Space 准入路由失败")?;
    Ok(())
}

fn map_rendezvous_error(source: RendezvousHttpError) -> anyhow::Error {
    anyhow::Error::new(source).context("通过 rendezvous 解析 Space 邀请失败")
}
