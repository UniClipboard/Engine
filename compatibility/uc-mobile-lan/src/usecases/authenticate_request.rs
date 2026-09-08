//! 完整负责验证一次手机请求并记录服务端观察到的活动。
//! 活动是辅助信息，写入失败仍返回已验证设备，由后续有效请求重试。

use super::authenticate_basic::{
    AuthenticateBasicAuthError, AuthenticateBasicAuthInput, AuthenticateBasicAuthUseCase,
    AuthenticatedDevice,
};
use crate::deps::RecordMobileDeviceActivityPort;
use std::sync::Arc;
use uc_core::ports::ClockPort;

pub(crate) struct AuthenticateMobileRequestUseCase {
    credentials: AuthenticateBasicAuthUseCase,
    activity: Arc<dyn RecordMobileDeviceActivityPort>,
    clock: Arc<dyn ClockPort>,
}

impl AuthenticateMobileRequestUseCase {
    pub(crate) fn new(
        credentials: AuthenticateBasicAuthUseCase,
        activity: Arc<dyn RecordMobileDeviceActivityPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            credentials,
            activity,
            clock,
        }
    }

    pub(crate) async fn execute(
        &self,
        input: AuthenticateBasicAuthInput,
    ) -> Result<AuthenticatedDevice, AuthenticateBasicAuthError> {
        let authenticated = self.credentials.execute(input).await?;
        if let Err(error) = self
            .activity
            .record_activity(&authenticated.device.device_id, self.clock.now_ms())
            .await
        {
            // Display 仅含固定分类；绝不输出 Debug 或原始 source 正文。
            tracing::warn!(error_category = %error, "mobile request activity recording failed");
        }
        Ok(authenticated)
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{CapturingAnalyticsSink, MockHasher};
    use super::*;
    use async_trait::async_trait;
    use base64::Engine;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use uc_core::mobile_sync::{MobileClientType, MobileDevice, MobileDeviceError, MobileDeviceId};
    use uc_core::ports::mobile_sync::MobileActivityError;
    use uc_core::ports::FindMobileDeviceByUsernamePort;

    #[derive(Clone, Default)]
    struct LogBuffer(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Repo {
        device: tokio::sync::Mutex<Option<MobileDevice>>,
        fail: AtomicBool,
    }

    #[async_trait]
    impl FindMobileDeviceByUsernamePort for Repo {
        async fn find_by_username(
            &self,
            username: &str,
        ) -> Result<Option<MobileDevice>, MobileDeviceError> {
            Ok(self
                .device
                .lock()
                .await
                .as_ref()
                .filter(|d| d.username == username)
                .cloned())
        }
    }

    #[async_trait]
    impl RecordMobileDeviceActivityPort for Repo {
        async fn record_activity(
            &self,
            id: &MobileDeviceId,
            at_ms: i64,
        ) -> Result<bool, MobileActivityError> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(MobileActivityError::WriteFailed {
                    source: std::io::Error::other(
                        "sensitive-device-name secret-password /private/path",
                    )
                    .into(),
                });
            }
            let mut guard = self.device.lock().await;
            let Some(device) = guard.as_mut().filter(|d| d.device_id == *id) else {
                return Ok(false);
            };
            if device.last_seen_at_ms.is_some_and(|last| last >= at_ms) {
                return Ok(false);
            }
            device.last_seen_at_ms = Some(at_ms);
            Ok(true)
        }
    }

    struct Clock(AtomicI64);
    impl ClockPort for Clock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn request(username: &str, password: &str) -> AuthenticateBasicAuthInput {
        AuthenticateBasicAuthInput {
            authorization_header: format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
            ),
        }
    }

    async fn setup() -> (
        AuthenticateMobileRequestUseCase,
        Arc<Repo>,
        Arc<Clock>,
        MobileDevice,
    ) {
        let repo = Arc::new(Repo::default());
        let device = MobileDevice {
            device_id: MobileDeviceId::new("device"),
            label: "phone".into(),
            username: "alice".into(),
            password_hash: "phc:password".into(),
            client_type: MobileClientType::IosShortcut,
            created_at_ms: 1,
            last_seen_at_ms: None,
            last_seen_ip: None,
            reported_name: None,
            reported_os: None,
        };
        *repo.device.lock().await = Some(device.clone());
        let mut hasher = MockHasher::new();
        hasher
            .expect_verify()
            .returning(|password, phc| Ok(phc == format!("phc:{password}")));
        let clock = Arc::new(Clock(AtomicI64::new(1_000)));
        let uc = AuthenticateMobileRequestUseCase::new(
            AuthenticateBasicAuthUseCase::new(
                repo.clone(),
                Arc::new(hasher),
                Arc::new(CapturingAnalyticsSink::default()),
            ),
            repo.clone(),
            clock.clone(),
        );
        (uc, repo, clock, device)
    }

    #[tokio::test]
    async fn activity_uses_server_clock_and_never_regresses() {
        let (uc, repo, clock, device) = setup().await;
        assert_eq!(
            repo.device.lock().await.as_ref().unwrap().last_seen_at_ms,
            None
        );
        for (now, expected) in [(1_000, 1_000), (2_000, 2_000), (500, 2_000), (2_000, 2_000)] {
            clock.0.store(now, Ordering::SeqCst);
            uc.execute(request("alice", "password")).await.unwrap();
            let mut expected_device = device.clone();
            expected_device.last_seen_at_ms = Some(expected);
            assert_eq!(*repo.device.lock().await, Some(expected_device));
        }
    }

    #[tokio::test]
    async fn rejected_and_revoked_credentials_do_not_record_activity() {
        let (uc, repo, _, device) = setup().await;
        for input in [
            request("alice", "wrong"),
            request("unknown", "password"),
            request("", ""),
        ] {
            assert!(matches!(
                uc.execute(input).await,
                Err(AuthenticateBasicAuthError::InvalidCredentials)
            ));
            assert_eq!(*repo.device.lock().await, Some(device.clone()));
        }
        let mut updated = device.clone();
        updated.password_hash = "phc:changed".into();
        *repo.device.lock().await = Some(updated.clone());
        assert!(matches!(
            uc.execute(request("alice", "password")).await,
            Err(AuthenticateBasicAuthError::InvalidCredentials)
        ));
        assert_eq!(*repo.device.lock().await, Some(updated));
        *repo.device.lock().await = None;
        assert!(matches!(
            uc.execute(request("alice", "password")).await,
            Err(AuthenticateBasicAuthError::InvalidCredentials)
        ));
        assert!(repo.device.lock().await.is_none());
    }

    #[tokio::test]
    async fn activity_failure_keeps_authentication_successful_and_next_request_retries() {
        let (uc, repo, clock, device) = setup().await;
        repo.fail.store(true, Ordering::SeqCst);
        let error = repo
            .record_activity(&device.device_id, 1_000)
            .await
            .unwrap_err();
        assert!(std::error::Error::source(&error)
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .is_some());
        assert_eq!(error.to_string(), "activity_write_failed");
        use tracing::instrument::WithSubscriber;
        let logs = LogBuffer::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        assert_eq!(
            uc.execute(request("alice", "password"))
                .with_subscriber(subscriber)
                .await
                .unwrap()
                .device,
            device
        );
        let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        assert!(output.contains("activity_write_failed"));
        for secret in [
            "sensitive-device-name",
            "secret-password",
            "/private/path",
            "alice",
            "phc:",
        ] {
            assert!(
                !output.contains(secret),
                "sensitive information in diagnostic output"
            );
        }
        assert_eq!(
            repo.device.lock().await.as_ref().unwrap().last_seen_at_ms,
            None
        );
        repo.fail.store(false, Ordering::SeqCst);
        clock.0.store(2_000, Ordering::SeqCst);
        uc.execute(request("alice", "password")).await.unwrap();
        assert_eq!(
            repo.device.lock().await.as_ref().unwrap().last_seen_at_ms,
            Some(2_000)
        );
    }
}
