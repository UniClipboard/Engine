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
    pub(crate) service_version: String,
    pub(crate) environment: DeploymentEnvironment,
    pub(crate) os: OperatingSystem,
    pub(crate) arch: String,
    pub(crate) app_channel: String,
}

impl ObservabilityResource {
    pub fn new(
        service_version: impl Into<String>,
        environment: DeploymentEnvironment,
        os: OperatingSystem,
        app_channel: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        Ok(Self {
            service_version: validated_service_version(&service_version.into())?,
            environment,
            os,
            arch: current_architecture().to_owned(),
            app_channel: validated_app_channel(&app_channel.into())?.to_owned(),
        })
    }
}

fn validated_service_version(value: &str) -> Result<String, ConfigError> {
    let version = semver::Version::parse(value).map_err(|_| ConfigError::InvalidServiceVersion)?;
    if version.major > 99_999 || version.minor > 99_999 || version.patch > 99_999 {
        return Err(ConfigError::InvalidServiceVersion);
    }
    let prerelease = version.pre.as_str();
    if !prerelease.is_empty() {
        let mut identifiers = prerelease.split('.');
        let recognized = matches!(identifiers.next(), Some("alpha" | "beta" | "rc"));
        let number = identifiers.next();
        let number_is_valid = number.is_none_or(|identifier| {
            !identifier.is_empty()
                && identifier.len() <= 5
                && identifier.bytes().all(|byte| byte.is_ascii_digit())
        });
        if !recognized || !number_is_valid || identifiers.next().is_some() {
            return Err(ConfigError::InvalidServiceVersion);
        }
    }
    let mut normalized = format!("{}.{}.{}", version.major, version.minor, version.patch);
    if !prerelease.is_empty() {
        normalized.push('-');
        normalized.push_str(prerelease);
    }
    if normalized.len() > 32 {
        return Err(ConfigError::InvalidServiceVersion);
    }
    Ok(normalized)
}

fn validated_app_channel(value: &str) -> Result<&'static str, ConfigError> {
    match value {
        "development" => Ok("development"),
        "test" => Ok("test"),
        "alpha" => Ok("alpha"),
        "beta" => Ok("beta"),
        "stable" => Ok("stable"),
        "production" => Ok("production"),
        _ => Err(ConfigError::InvalidAppChannel),
    }
}

fn current_architecture() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "arm" => "arm",
        "x86" => "x86",
        "x86_64" => "x86_64",
        "riscv64" => "riscv64",
        "s390x" => "s390x",
        "powerpc" => "powerpc",
        "powerpc64" => "powerpc64",
        "wasm32" => "wasm32",
        _ => "other",
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
        let trace_endpoint = parse_endpoint(trace_endpoint, false)?;
        let log_endpoint = parse_endpoint(log_endpoint, false)?;
        Ok(Self {
            trace_endpoint,
            log_endpoint,
            headers: Vec::new(),
            timeout: Duration::from_secs(5),
        })
    }

    /// 只供本机 Collector、测试 receiver 和 Jaeger 使用的明文入口。
    pub fn new_loopback(trace_endpoint: &str, log_endpoint: &str) -> Result<Self, ConfigError> {
        let trace_endpoint = parse_endpoint(trace_endpoint, true)?;
        let log_endpoint = parse_endpoint(log_endpoint, true)?;
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

fn parse_endpoint(value: &str, allow_loopback_http: bool) -> Result<Url, ConfigError> {
    let endpoint = Url::parse(value).map_err(|_| ConfigError::InvalidEndpoint)?;
    let secure = endpoint.scheme() == "https";
    let local_http = allow_loopback_http
        && endpoint.scheme() == "http"
        && endpoint.host().is_some_and(|host| match host {
            url::Host::Domain(name) => name.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(address) => address.is_loopback(),
            url::Host::Ipv6(address) => address.is_loopback(),
        });
    if (!secure && !local_http)
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
    #[error("invalid service version")]
    InvalidServiceVersion,
    #[error("invalid application channel")]
    InvalidAppChannel,
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
                "http://collector.example/v1/traces",
                "https://collector.example/v1/logs"
            ),
            Err(ConfigError::InvalidEndpoint)
        ));
        assert!(OtlpHttpConfig::new_loopback(
            "http://127.0.0.1:4318/v1/traces",
            "http://localhost:4318/v1/logs"
        )
        .is_ok());
        assert!(matches!(
            OtlpHttpConfig::new(
                "https://collector.example/v1/traces?token=private",
                "https://collector.example/v1/logs"
            ),
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
        assert!(matches!(
            OtlpHttpConfig::new(
                "https://collector.example/v1/traces",
                "https://collector.example/v1/logs"
            )
            .expect("valid endpoint")
            .with_header("authorization", SecretHeaderValue::new("bad\nvalue")),
            Err(ConfigError::InvalidHeaderValue)
        ));
    }

    #[test]
    fn resource_values_use_closed_low_cardinality_domains() {
        for version in [
            "MyPhone123",
            "phc_abcdef",
            "deadbeef",
            "1.2.3-private",
            "1.2.3-alpha.1.2",
            "100000.2.3",
        ] {
            assert!(matches!(
                ObservabilityResource::new(
                    version,
                    DeploymentEnvironment::Test,
                    OperatingSystem::Other,
                    "test",
                ),
                Err(ConfigError::InvalidServiceVersion)
            ));
        }
        for channel in ["MyPhone123", "phc_abcdef", "deadbeef", "integration-test"] {
            assert!(matches!(
                ObservabilityResource::new(
                    "1.2.3",
                    DeploymentEnvironment::Test,
                    OperatingSystem::Other,
                    channel,
                ),
                Err(ConfigError::InvalidAppChannel)
            ));
        }

        let resource = ObservabilityResource::new(
            "1.2.3-rc.5+MyPhone123",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "beta",
        )
        .expect("approved resource values");
        assert_eq!(resource.service_version, "1.2.3-rc.5");
        assert_eq!(resource.arch, current_architecture());
        assert_eq!(resource.app_channel, "beta");
        assert!(!format!("{resource:?}").contains("MyPhone123"));
    }
}
