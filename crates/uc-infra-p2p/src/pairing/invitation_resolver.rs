//! 邀请解析只负责把短码或完整邀请还原为新准入邀请。
//!
//! 该适配器不建立业务 session，也不注册 ALPN。准入消息统一交给
//! `/uniclipboard/space-admission/1` transport。

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use async_trait::async_trait;
use futures_util::future::Either;
use iroh::Endpoint;

use uc_application::deps::{ResolveJoinerInvitationError, ResolveJoinerInvitationPort};
use uc_core::membership::AdmissionShortInvitationCode;
use uc_core::pairing::invitation::FullInvitation;
use uc_observability_contract::diagnostics::connectivity::{
    observe_invitation_resolution, InvitationResolution, InvitationSource, InvitationSourceOutcome,
};

use crate::rendezvous::{RendezvousClient, RendezvousHttpError};
use uc_observability_contract::uc_debug;

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

    async fn resolve(&self, code: &str) -> (anyhow::Result<FullInvitation>, InvitationResolution) {
        if code.starts_with("ucspace1_") {
            let result = FullInvitation::new(code.to_owned())
                .map_err(anyhow::Error::new)
                .context("解析完整 Space 邀请失败")
                .and_then(|invitation| {
                    validate_invitation_route(invitation.as_str())?;
                    Ok(invitation)
                });
            let resolution = if result.is_ok() {
                InvitationResolution::full_invitation()
            } else {
                InvitationResolution {
                    winner: None,
                    cloud: InvitationSourceOutcome::NotRun,
                    mdns: InvitationSourceOutcome::NotRun,
                }
            };
            return (result, resolution);
        }

        if crate::network::iroh::runtime_consts::lan_only() {
            uc_debug!("LAN-only 模式只通过 mDNS 解析邀请");
            return match self.resolve_via_mdns(code).await {
                Ok(invitation) => (
                    Ok(invitation),
                    InvitationResolution {
                        winner: Some(InvitationSource::Mdns),
                        cloud: InvitationSourceOutcome::NotRun,
                        mdns: InvitationSourceOutcome::Found,
                    },
                ),
                Err(failure) => (
                    Err(failure.error),
                    InvitationResolution {
                        winner: None,
                        cloud: InvitationSourceOutcome::NotRun,
                        mdns: failure.outcome,
                    },
                ),
            };
        }

        race_sources(self.resolve_via_cloud(code), self.resolve_via_mdns(code)).await
    }

    async fn resolve_via_cloud(&self, code: &str) -> SourceResult {
        let response = self
            .rendezvous
            .resolve_pairing(code)
            .await
            .map_err(|source| SourceFailure {
                outcome: cloud_outcome(&source),
                error: map_rendezvous_error(source),
            })?;
        let invitation = FullInvitation::new(response.sponsor_ticket)
            .map_err(anyhow::Error::new)
            .context("解析 rendezvous Space 邀请失败")
            .and_then(|invitation| {
                validate_invitation_route(invitation.as_str())?;
                Ok(invitation)
            })
            .map_err(SourceFailure::invalid_invitation)?;
        Ok(invitation)
    }

    async fn resolve_via_mdns(&self, code: &str) -> SourceResult {
        let ticket = crate::pairing::MdnsPairingResolver::resolve(
            &tokio::runtime::Handle::current(),
            &self.endpoint.id().to_string(),
            code,
            Duration::from_secs(5),
        )
        .await
        .map_err(|source| SourceFailure {
            outcome: InvitationSourceOutcome::LocalUnavailable,
            error: anyhow::Error::new(source).context("通过 mDNS 解析 Space 邀请失败"),
        })?
        .ok_or_else(|| SourceFailure {
            outcome: InvitationSourceOutcome::NotFound,
            error: anyhow::anyhow!("mDNS 未找到 Space 邀请"),
        })?;
        decode_mdns_invitation(&ticket).map_err(SourceFailure::invalid_invitation)
    }
}

fn decode_mdns_invitation(ticket: &str) -> anyhow::Result<FullInvitation> {
    let bytes = hex::decode(ticket).context("解码 mDNS Space 邀请失败")?;
    let encoded = std::str::from_utf8(&bytes).context("读取 mDNS Space 邀请失败")?;
    let invitation = FullInvitation::new(encoded.to_owned())
        .map_err(anyhow::Error::new)
        .context("解析 mDNS Space 邀请失败")?;
    validate_invitation_route(invitation.as_str())?;
    Ok(invitation)
}

/// 单条来源的失败：保留完整错误链，同时给出可记录的结果类别。
struct SourceFailure {
    outcome: InvitationSourceOutcome,
    error: anyhow::Error,
}

impl SourceFailure {
    fn invalid_invitation(error: anyhow::Error) -> Self {
        Self {
            outcome: InvitationSourceOutcome::InvalidInvitation,
            error,
        }
    }
}

type SourceResult = Result<FullInvitation, SourceFailure>;

/// 云端与局域网并行解析：先成功的来源胜出；先失败的一方不终止竞速，最终失败返回后到一方的错误。
async fn race_sources(
    cloud: impl Future<Output = SourceResult>,
    lan: impl Future<Output = SourceResult>,
) -> (anyhow::Result<FullInvitation>, InvitationResolution) {
    tokio::pin!(cloud);
    tokio::pin!(lan);
    let won = |winner, cloud, mdns, invitation| {
        (
            Ok(invitation),
            InvitationResolution {
                winner: Some(winner),
                cloud,
                mdns,
            },
        )
    };
    let lost = |cloud, mdns, error| {
        (
            Err(error),
            InvitationResolution {
                winner: None,
                cloud,
                mdns,
            },
        )
    };
    match futures_util::future::select(cloud, lan).await {
        Either::Left((Ok(invitation), _)) => won(
            InvitationSource::Cloud,
            InvitationSourceOutcome::Found,
            InvitationSourceOutcome::Pending,
            invitation,
        ),
        Either::Right((Ok(invitation), _)) => won(
            InvitationSource::Mdns,
            InvitationSourceOutcome::Pending,
            InvitationSourceOutcome::Found,
            invitation,
        ),
        Either::Left((Err(cloud_failure), pending)) => match pending.await {
            Ok(invitation) => won(
                InvitationSource::Mdns,
                cloud_failure.outcome,
                InvitationSourceOutcome::Found,
                invitation,
            ),
            Err(lan_failure) => lost(
                cloud_failure.outcome,
                lan_failure.outcome,
                lan_failure.error,
            ),
        },
        Either::Right((Err(lan_failure), pending)) => match pending.await {
            Ok(invitation) => won(
                InvitationSource::Cloud,
                InvitationSourceOutcome::Found,
                lan_failure.outcome,
                invitation,
            ),
            Err(cloud_failure) => lost(
                cloud_failure.outcome,
                lan_failure.outcome,
                cloud_failure.error,
            ),
        },
    }
}

fn cloud_outcome(source: &RendezvousHttpError) -> InvitationSourceOutcome {
    match source {
        RendezvousHttpError::Transport { .. } => InvitationSourceOutcome::TransportFailed,
        RendezvousHttpError::NotFound => InvitationSourceOutcome::NotFound,
        RendezvousHttpError::Gone => InvitationSourceOutcome::Expired,
        RendezvousHttpError::Conflict => InvitationSourceOutcome::Conflict,
        RendezvousHttpError::ServiceUnavailable(_) => InvitationSourceOutcome::ServiceUnavailable,
        RendezvousHttpError::Unexpected { .. } | RendezvousHttpError::Parse { .. } => {
            InvitationSourceOutcome::InvalidResponse
        }
    }
}

#[async_trait]
impl ResolveJoinerInvitationPort for PairingInvitationResolverAdapter {
    async fn resolve_once(
        &self,
        short_code: &AdmissionShortInvitationCode,
    ) -> Result<FullInvitation, ResolveJoinerInvitationError> {
        observe_invitation_resolution(async {
            let code = match std::str::from_utf8(short_code.as_bytes()) {
                Ok(code) => code,
                Err(source) => {
                    return (
                        Err(ResolveJoinerInvitationError::unavailable(
                            anyhow::Error::new(source),
                        )),
                        InvitationResolution {
                            winner: None,
                            cloud: InvitationSourceOutcome::NotRun,
                            mdns: InvitationSourceOutcome::NotRun,
                        },
                    )
                }
            };
            let (result, resolution) = self.resolve(code).await;
            (
                result.map_err(ResolveJoinerInvitationError::unavailable),
                resolution,
            )
        })
        .await
    }
}

pub(crate) fn validate_invitation_route(invitation: &str) -> anyhow::Result<()> {
    let decoded = uc_sync_protocol::full_invitation::decode_invitation_entry(
        invitation,
        chrono::Utc::now().timestamp_millis(),
    )
    .map_err(anyhow::Error::new)
    .context("验证 Space 邀请失败")?
    .ok_or_else(|| anyhow::anyhow!("Space 邀请不可用"))?;
    crate::network::iroh::space_admission::decode_space_admission_route(decoded.route())
        .map_err(anyhow::Error::new)
        .context("解析 Space 准入路由失败")?;
    Ok(())
}

fn map_rendezvous_error(source: RendezvousHttpError) -> anyhow::Error {
    anyhow::Error::new(source).context("通过 rendezvous 解析 Space 邀请失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::logs::AnyValue;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;
    use uc_observability_contract::diagnostics::connectivity::decode_local_record;

    fn invitation() -> FullInvitation {
        FullInvitation::new("ucspace1_test").expect("valid invitation")
    }

    fn failure(outcome: InvitationSourceOutcome) -> SourceFailure {
        SourceFailure {
            outcome,
            error: anyhow::anyhow!("source failed"),
        }
    }

    /// 在真实观测路径上跑一次竞速，返回结果与解析完成记录的字段。
    async fn observed(
        cloud: impl Future<Output = SourceResult>,
        lan: impl Future<Output = SourceResult>,
    ) -> (bool, serde_json::Map<String, serde_json::Value>, usize) {
        let exporter = InMemoryLogExporter::default();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            OpenTelemetryTracingBridge::new(&logs)
                .with_filter(filter_fn(|metadata| metadata.target() == "uc.connectivity")),
        );
        let _guard = tracing::subscriber::set_default(subscriber);
        let result = observe_invitation_resolution(race_sources(cloud, lan)).await;
        logs.force_flush().expect("log flush");
        let mut finished = Vec::new();
        for entry in exporter.get_emitted_logs().expect("logs") {
            let text = |name: &str| {
                entry
                    .record
                    .attributes_iter()
                    .find_map(|(key, value)| match value {
                        AnyValue::String(value) if key.as_str() == name => {
                            Some(value.as_str().to_owned())
                        }
                        _ => None,
                    })
            };
            if text("event.name").as_deref() != Some("runtime.work.finished") {
                continue;
            }
            let level = if result.is_ok() { "INFO" } else { "WARN" };
            let fields = decode_local_record(
                "runtime.work.finished",
                &text("payload").expect("payload"),
                level,
            )
            .expect("approved record");
            finished.push(fields);
        }
        let count = finished.len();
        (result.is_ok(), finished.remove(0), count)
    }

    #[tokio::test]
    async fn cloud_wins_while_mdns_is_still_searching() {
        let (ok, fields, count) = observed(
            async { Ok(invitation()) },
            std::future::pending::<SourceResult>(),
        )
        .await;
        assert!(ok);
        assert_eq!(count, 1);
        assert_eq!(fields["winner"], "cloud");
        assert_eq!(fields["cloud_outcome"], "found");
        assert_eq!(fields["mdns_outcome"], "pending");
        assert_eq!(fields["step"], "joiner_resolve_invitation");
        assert!(fields.contains_key("duration_ms"));
    }

    #[tokio::test]
    async fn mdns_wins_after_cloud_transport_failure_with_the_real_client() {
        // 真实客户端连接一个已关闭的本地端口，得到真实的传输错误。
        let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", closed.local_addr().expect("addr"));
        drop(closed);
        let client = RendezvousClient::with_base_url(base);
        let cloud = async {
            client
                .resolve_pairing("000-001")
                .await
                .map(|_| invitation())
                .map_err(|source| SourceFailure {
                    outcome: cloud_outcome(&source),
                    error: map_rendezvous_error(source),
                })
        };
        let lan = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(invitation())
        };
        let (ok, fields, _) = observed(cloud, lan).await;
        assert!(ok);
        assert_eq!(fields["winner"], "mdns");
        assert_eq!(fields["cloud_outcome"], "transport_failed");
        assert_eq!(fields["mdns_outcome"], "found");
    }

    #[tokio::test]
    async fn both_sources_failing_records_each_class_and_no_winner() {
        let (ok, fields, _) = observed(
            async { Err(failure(InvitationSourceOutcome::Expired)) },
            async { Err(failure(InvitationSourceOutcome::NotFound)) },
        )
        .await;
        assert!(!ok);
        assert!(!fields.contains_key("winner"));
        assert_eq!(fields["cloud_outcome"], "expired");
        assert_eq!(fields["mdns_outcome"], "not_found");
        assert_eq!(fields["uc.outcome"], "error");
    }

    #[test]
    fn rendezvous_errors_map_to_closed_classes() {
        assert_eq!(
            cloud_outcome(&RendezvousHttpError::NotFound),
            InvitationSourceOutcome::NotFound
        );
        assert_eq!(
            cloud_outcome(&RendezvousHttpError::Gone),
            InvitationSourceOutcome::Expired
        );
        assert_eq!(
            cloud_outcome(&RendezvousHttpError::Conflict),
            InvitationSourceOutcome::Conflict
        );
    }
}
