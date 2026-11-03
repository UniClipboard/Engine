use std::{fmt, sync::Arc, time::Duration};

use chrono::{DateTime, TimeDelta, Utc};
use futures_util::StreamExt;
use iroh::{Endpoint, EndpointId, RelayConfig, RelayUrl, Watcher as _};
use tokio::{
    task::JoinHandle,
    time::{sleep, Instant},
};
use tokio_util::task::AbortOnDropHandle;
use tracing::{instrument::WithSubscriber, Dispatch};
use uc_core::settings::relay_routing::BUILTIN_RELAYS;
use uc_observability_contract::uc_warn;

use super::client::{ControlPlane, ControlPlaneError, Node};

pub(crate) const CLOUD_RELAY_API: &str = "https://cloud.uniclipboard.app/api/relay/v1";
pub(super) const BUILTIN_RELAY_ID: &str = BUILTIN_RELAYS[0].id;

const CATALOG_INTERVAL: Duration = Duration::from_secs(30);

struct ActiveRelay {
    id: String,
    catalog_url: String,
    url: RelayUrl,
    renew_at: Instant,
    renew_before: DateTime<Utc>,
}

pub(crate) struct ManagedRelay {
    client: ControlPlane,
    endpoint_id: String,
    pub(super) nodes: Vec<Node>,
    active: Option<ActiveRelay>,
    next_catalog: Instant,
    next_credential: Instant,
    catalog_failures: u32,
    credential_failures: u32,
    pub(crate) applied: Option<Arc<dyn Fn(Vec<String>) + Send + Sync>>,
}

impl fmt::Debug for ManagedRelay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedRelay")
            .field("node_count", &self.nodes.len())
            .field("credential_installed", &self.active.is_some())
            .finish()
    }
}

impl ManagedRelay {
    pub fn new(endpoint_id: EndpointId, base: &str) -> Result<Self, reqwest::Error> {
        let now = Instant::now();
        Ok(Self {
            client: ControlPlane::new(base)?,
            endpoint_id: hex::encode(endpoint_id.as_bytes()),
            nodes: BUILTIN_RELAYS
                .iter()
                .map(|node| Node {
                    id: node.id.to_owned(),
                    url: node.url.to_owned(),
                    region: node.region.to_owned(),
                    credential_required: true,
                })
                .collect(),
            active: None,
            next_catalog: now,
            next_credential: now,
            catalog_failures: 0,
            credential_failures: 0,
            applied: None,
        })
    }

    pub async fn tick(&mut self, endpoint: &Endpoint, now: Instant, wall: DateTime<Utc>) {
        if now >= self.next_catalog {
            match self.client.catalog().await {
                Ok(nodes) => {
                    self.nodes = nodes;
                    self.catalog_failures = 0;
                    self.next_catalog = now + CATALOG_INTERVAL;
                }
                Err(error) => {
                    self.next_catalog =
                        now + retry_delay(&error, &mut self.catalog_failures).max(CATALOG_INTERVAL);
                    uc_warn!(
                        error_kind = "managed_relay_catalog",
                        "managed relay catalog refresh failed; retaining last valid catalog"
                    );
                }
            }
        }
        // 上海仍在有效目录中时优先使用；被移除后不恢复内置候选。
        let selected = self
            .nodes
            .iter()
            .find(|node| node.id == BUILTIN_RELAY_ID)
            .or_else(|| self.nodes.first());
        let changed = self.active.as_ref().is_some_and(|active| {
            !selected.is_some_and(|node| node.id == active.id && node.url == active.catalog_url)
        });
        if changed {
            self.remove_active(endpoint).await;
        }
        let Some(selected) = self
            .nodes
            .iter()
            .find(|node| node.id == BUILTIN_RELAY_ID)
            .or_else(|| self.nodes.first())
            .cloned()
        else {
            return;
        };
        let renewal_due = self
            .active
            .as_ref()
            .is_some_and(|active| now >= active.renew_at || wall >= active.renew_before);
        if !renewal_due && (self.active.is_some() || now < self.next_credential) {
            return;
        }
        match self
            .client
            .credential(&selected.id, &self.endpoint_id, wall)
            .await
        {
            Ok(credential) => {
                let url = credential.url;
                if self.active.as_ref().is_some_and(|active| active.url != url) {
                    self.remove_active(endpoint).await;
                }
                let config = RelayConfig::from(url.clone())
                    .with_auth_token(credential.token.expose_secret())
                    .with_auth_token_expiry(credential.expires_at);
                endpoint.insert_relay(url.clone(), Arc::new(config)).await;
                self.active = Some(ActiveRelay {
                    id: selected.id,
                    catalog_url: selected.url,
                    url,
                    renew_at: now + credential.lifetime - Duration::from_secs(121),
                    renew_before: wall
                        + TimeDelta::seconds(credential.lifetime.as_secs() as i64 - 121),
                });
                self.credential_failures = 0;
                if let Some(record) = &self.applied {
                    record(
                        self.active
                            .iter()
                            .map(|active| active.url.to_string())
                            .collect(),
                    );
                }
            }
            Err(error) => {
                // 在最后两分钟前撤下无法续期的凭据，防止 HTTP 等待或重连复用过期令牌。
                self.remove_active(endpoint).await;
                self.next_credential = now + retry_delay(&error, &mut self.credential_failures);
                uc_warn!(
                    error_kind = "managed_relay_credential",
                    "managed relay credential unavailable; direct connectivity remains enabled"
                );
            }
        }
    }

    async fn remove_active(&mut self, endpoint: &Endpoint) {
        if let Some(active) = self.active.take() {
            endpoint.remove_relay(&active.url).await;
            if let Some(record) = &self.applied {
                record(Vec::new());
            }
        }
    }

    pub fn spawn(mut self, endpoint: Endpoint) -> AbortOnDropHandle<()> {
        let task: JoinHandle<()> = tokio::spawn(
            async move {
                let mut status = endpoint.home_relay_status().stream();
                loop {
                    if endpoint.is_closed() {
                        break;
                    }
                    self.tick(&endpoint, Instant::now(), Utc::now()).await;
                    let next = match &self.active {
                        Some(active) => self.next_catalog.min(active.renew_at),
                        None if self.nodes.is_empty() => self.next_catalog,
                        None => self.next_catalog.min(self.next_credential),
                    };
                    tokio::select! {
                        _ = sleep(next.saturating_duration_since(Instant::now()).max(Duration::from_secs(1))) => {},
                        changed = status.next() => { if changed.is_none() { break; } },
                    }
                }
            }
            .with_subscriber(Dispatch::none()),
        );
        AbortOnDropHandle::new(task)
    }
}

fn retry_delay(error: &ControlPlaneError, failures: &mut u32) -> Duration {
    *failures = failures.saturating_add(1);
    let exponential = Duration::from_secs((5u64 << (*failures - 1).min(6)).min(300));
    match error {
        ControlPlaneError::Rejected { status: 403, .. } => Duration::from_secs(300),
        ControlPlaneError::Rejected { retry_after, .. } => exponential.max(*retry_after),
        _ => exponential,
    }
}
