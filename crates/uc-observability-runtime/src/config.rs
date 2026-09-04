use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use url::Url;
use zeroize::Zeroize;

pub const LOCAL_LOG_RETENTION_DAYS: u64 = 7;
pub const LOCAL_LOG_MAX_BYTES: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeploymentEnvironment {
    Development,
    Test,
    Staging,
    Production,
}

impl DeploymentEnvironment {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Test => "test",
            Self::Staging => "staging",
            Self::Production => "production",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatingSystem {
    Ios,
    Android,
    Macos,
    Windows,
    Linux,
    Ohos,
    Other,
}

impl OperatingSystem {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ios => "ios",
            Self::Android => "android",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Ohos => "ohos",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservabilityResource {
    pub service_version: String,
    pub environment: DeploymentEnvironment,
    pub os: OperatingSystem,
    pub arch: String,
    pub app_channel: String,
}

impl ObservabilityResource {
    pub fn new(
        service_version: impl Into<String>,
        environment: DeploymentEnvironment,
        os: OperatingSystem,
        arch: impl Into<String>,
        app_channel: impl Into<String>,
    ) -> Self {
        Self {
            service_version: service_version.into(),
            environment,
            os,
            arch: arch.into(),
            app_channel: app_channel.into(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretHeaderValue(String);

impl SecretHeaderValue {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretHeaderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretHeaderValue(REDACTED)")
    }
}

impl Drop for SecretHeaderValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct OtlpHttpConfig {
    trace_endpoint: Url,
    log_endpoint: Url,
    headers: Vec<(String, SecretHeaderValue)>,
    timeout: Duration,
}

impl OtlpHttpConfig {
    pub fn new(trace_endpoint: &str, log_endpoint: &str) -> Result<Self, ConfigError> {
        let trace_endpoint = parse_endpoint(trace_endpoint)?;
        let log_endpoint = parse_endpoint(log_endpoint)?;
        Ok(Self {
            trace_endpoint,
            log_endpoint,
            headers: Vec::new(),
            timeout: Duration::from_secs(5),
        })
    }

    pub fn with_header(
        mut self,
        name: impl Into<String>,
        value: SecretHeaderValue,
    ) -> Result<Self, ConfigError> {
        let name = name.into();
        if reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err() {
            return Err(ConfigError::InvalidHeaderName);
        }
        if reqwest::header::HeaderValue::from_str(value.expose()).is_err() {
            return Err(ConfigError::InvalidHeaderValue);
        }
        self.headers.push((name, value));
        Ok(self)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, ConfigError> {
        if timeout.is_zero() {
            return Err(ConfigError::ZeroTimeout);
        }
        self.timeout = timeout;
        Ok(self)
    }

    pub(crate) fn trace_endpoint(&self) -> &str {
        self.trace_endpoint.as_str()
    }

    pub(crate) fn log_endpoint(&self) -> &str {
        self.log_endpoint.as_str()
    }

    pub(crate) fn headers(&self) -> &[(String, SecretHeaderValue)] {
        &self.headers
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl fmt::Debug for OtlpHttpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OtlpHttpConfig")
            .field("trace_endpoint", &"REDACTED")
            .field("log_endpoint", &"REDACTED")
            .field(
                "headers",
                &format_args!("{} configured", self.headers.len()),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

fn parse_endpoint(value: &str) -> Result<Url, ConfigError> {
    let endpoint = Url::parse(value).map_err(|_| ConfigError::InvalidEndpoint)?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(ConfigError::InvalidEndpoint);
    }
    Ok(endpoint)
}

#[derive(Clone, PartialEq, Eq)]
pub struct LocalLogConfig {
    pub(crate) directory: PathBuf,
}

impl LocalLogConfig {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }
}

impl fmt::Debug for LocalLogConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalLogConfig")
            .field("directory", &"REDACTED")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub resource: ObservabilityResource,
    pub local_logs: Option<LocalLogConfig>,
    pub remote: Option<OtlpHttpConfig>,
}

impl ObservabilityConfig {
    pub fn new(resource: ObservabilityResource) -> Self {
        Self {
            resource,
            local_logs: None,
            remote: None,
        }
    }

    pub fn with_local_logs(mut self, config: LocalLogConfig) -> Self {
        self.local_logs = Some(config);
        self
    }

    pub fn with_remote(mut self, config: OtlpHttpConfig) -> Self {
        self.remote = Some(config);
        self
    }
}

impl fmt::Debug for ObservabilityConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservabilityConfig")
            .field("resource", &self.resource)
            .field("local_logs", &self.local_logs)
            .field("remote", &self.remote)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid OTLP endpoint")]
    InvalidEndpoint,
    #[error("invalid OTLP header name")]
    InvalidHeaderName,
    #[error("invalid OTLP header value")]
    InvalidHeaderValue,
    #[error("OTLP timeout must be non-zero")]
    ZeroTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_and_endpoints_are_redacted() {
        let remote = OtlpHttpConfig::new(
            "https://collector.example/v1/traces",
            "https://collector.example/v1/logs",
        )
        .expect("valid endpoint")
        .with_header(
            "authorization",
            SecretHeaderValue::new("Bearer private-token"),
        )
        .expect("valid header");
        let output = format!("{remote:?}");
        assert!(!output.contains("collector.example"));
        assert!(!output.contains("private-token"));
        assert!(output.contains("REDACTED"));
    }

    #[test]
    fn endpoints_are_explicit_and_bounded() {
        assert!(matches!(
            OtlpHttpConfig::new("file:///tmp/output", "https://collector.example/v1/logs"),
            Err(ConfigError::InvalidEndpoint)
        ));
        assert!(matches!(
            OtlpHttpConfig::new(
                "https://collector.example/v1/traces",
                "https://collector.example/v1/logs"
            )
            .expect("valid endpoint")
            .with_timeout(Duration::ZERO),
            Err(ConfigError::ZeroTimeout)
        ));
    }
}
