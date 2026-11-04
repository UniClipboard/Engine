use std::{
    collections::BTreeSet,
    fmt,
    time::{Duration, SystemTime},
};

use chrono::{DateTime, Utc};
use iroh::RelayUrl;
use reqwest::{redirect::Policy, Client, Response, Url};
use serde::{de::DeserializeOwned, Deserialize};
use zeroize::Zeroizing;

use super::super::node::{IrohNodeError, IrohRelayAccessToken};

const MAX_CATALOG_BYTES: usize = 64 * 1024;
const MAX_CREDENTIAL_BYTES: usize = 16 * 1024;

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub(super) struct Node {
    pub id: String,
    pub url: String,
    pub region: String,
    pub credential_required: bool,
}

#[derive(Deserialize)]
struct Catalog {
    nodes: Vec<Node>,
    catalog_version: String,
}

#[derive(Deserialize)]
struct CredentialResponse {
    relay_url: String,
    access_token: String,
    expires_at: String,
}

pub(super) struct Credential {
    pub url: RelayUrl,
    pub token: IrohRelayAccessToken,
    pub lifetime: Duration,
    pub expires_at: SystemTime,
}

pub(super) struct ControlPlane {
    client: Client,
    base: String,
}

#[derive(thiserror::Error)]
pub(super) enum ControlPlaneError {
    #[error("relay control plane HTTP failure")]
    Transport(#[from] reqwest::Error),
    #[error("relay control plane response decoding failed")]
    Decode(#[from] serde_json::Error),
    #[error("relay control plane returned an invalid expiry")]
    Timestamp(#[from] chrono::ParseError),
    #[error("relay control plane returned an invalid credential")]
    Token(#[from] IrohNodeError),
    #[error("relay control plane request rejected")]
    Rejected { status: u16, retry_after: Duration },
    #[error("relay control plane response exceeds its size limit")]
    TooLarge,
    #[error("relay control plane response violates its contract")]
    Invalid,
}

impl fmt::Debug for ControlPlaneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl ControlPlane {
    pub fn new(base: &str) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(5))
                .redirect(Policy::none())
                .user_agent("UniClipboard-Engine/managed-relay-v1")
                .build()?,
            base: base.trim_end_matches('/').to_owned(),
        })
    }

    pub async fn catalog(&self) -> Result<Vec<Node>, ControlPlaneError> {
        let response = self
            .client
            .get(format!("{}/nodes", self.base))
            .send()
            .await?;
        let catalog: Catalog = read_response(response, MAX_CATALOG_BYTES).await?;
        if catalog.nodes.len() > 32 || catalog.catalog_version.len() > 128 {
            return Err(ControlPlaneError::Invalid);
        }
        let mut ids = BTreeSet::new();
        for node in &catalog.nodes {
            if node.id.is_empty()
                || node.id.len() > 128
                || !node
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
                || node.region.is_empty()
                || node.region.len() > 64
                || !ids.insert(&node.id)
            {
                return Err(ControlPlaneError::Invalid);
            }
            https_origin(&node.url)?;
        }
        Ok(catalog.nodes)
    }

    pub async fn credential(
        &self,
        relay_id: &str,
        endpoint_id: &str,
        now: DateTime<Utc>,
    ) -> Result<Credential, ControlPlaneError> {
        let response = self
            .client
            .post(format!("{}/credentials", self.base))
            .json(&serde_json::json!({"relay_id":relay_id,"endpoint_id":endpoint_id}))
            .send()
            .await?;
        let response: CredentialResponse = read_response(response, MAX_CREDENTIAL_BYTES).await?;
        // 先接管令牌，后续任何字段校验失败都必须清零。
        let token = IrohRelayAccessToken::new(response.access_token)?;
        let url = https_origin(&response.relay_url)?;
        let expires_at = DateTime::parse_from_rfc3339(&response.expires_at)?;
        let lifetime = (expires_at.with_timezone(&Utc) - now)
            .to_std()
            .map_err(|_| ControlPlaneError::Invalid)?; // discarded-source[input-validation]: 负时长表示凭据已过期
                                                       // 保守限制为协议的十分钟；拒绝没有足够续期窗口的凭据。
        if lifetime <= Duration::from_secs(121) || lifetime > Duration::from_secs(630) {
            return Err(ControlPlaneError::Invalid);
        }
        Ok(Credential {
            url,
            token,
            lifetime: lifetime.min(Duration::from_secs(600)),
            expires_at: expires_at.with_timezone(&Utc).into(),
        })
    }
}

fn https_origin(raw: &str) -> Result<RelayUrl, ControlPlaneError> {
    if raw.len() > 2048 {
        return Err(ControlPlaneError::Invalid);
    }
    // discarded-source[input-validation]: 不保留可能回显外部地址的校验错误
    let url = Url::parse(raw).map_err(|_| ControlPlaneError::Invalid)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(ControlPlaneError::Invalid);
    }
    // discarded-source[input-validation]: RelayUrl 使用相同 URL 语法，不保留外部输入
    raw.parse().map_err(|_| ControlPlaneError::Invalid)
}

async fn read_response<T: DeserializeOwned>(
    mut response: Response,
    limit: usize,
) -> Result<T, ControlPlaneError> {
    if !response.status().is_success() {
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .map(Duration::from_secs)
                    .or_else(|| {
                        DateTime::parse_from_rfc2822(value)
                            .ok()
                            .and_then(|date| (date.with_timezone(&Utc) - Utc::now()).to_std().ok())
                    })
            })
            .unwrap_or_default()
            .min(Duration::from_secs(300));
        return Err(ControlPlaneError::Rejected {
            status: response.status().as_u16(),
            retry_after,
        });
    }
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(ControlPlaneError::TooLarge);
    }
    let mut body = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(ControlPlaneError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&body)?)
}
