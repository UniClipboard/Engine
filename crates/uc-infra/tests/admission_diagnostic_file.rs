//! 真实资料读取、真实本机协议与实际导出文件共同验证一条认证失败。
use async_trait::async_trait;
use iroh::{endpoint::presets, protocol::Router, Endpoint, RelayMode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uc_application::deps::*;
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, AdmissionPeerBinding,
    SpaceAdmissionId, SpaceAdmissionRoute,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::{executor::DieselSqliteExecutor, pool::init_db_pool};
use uc_infra::network::iroh::{
    encode_space_admission_route, IrohSpaceAdmissionHandler, IrohSpaceAdmissionTransport,
    SPACE_ADMISSION_ALPN,
};
use uc_infra::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};
use uc_infra::space::{SqliteSpaceAdmissionCredentials, SqliteSpaceAdmissionState};
use uc_observability_runtime::*;

#[derive(Default)]
struct MemoryKeys(Mutex<HashMap<String, Vec<u8>>>);
impl SecureStoragePort for MemoryKeys {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.0.lock().expect("keys").get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .expect("keys")
            .insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().expect("keys").remove(key);
        Ok(())
    }
}
struct EmptyLedger;
#[async_trait]
impl LoadMembershipLedgerPort for EmptyLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(LoadedMembershipLedger::no_current_space())
    }
}
struct MustNotHandle;
#[async_trait]
impl HandleAuthenticatedSpaceAdmissionMessagePort for MustNotHandle {
    async fn handle(
        &self,
        _: AuthenticatedSpaceAdmissionMessage,
    ) -> Result<SpaceAdmissionMessageReply, HandleAuthenticatedSpaceAdmissionMessageError> {
        panic!("an unauthenticated request must not reach business handling")
    }
}
async fn endpoint() -> Arc<Endpoint> {
    let endpoint = Arc::new(
        Endpoint::builder(presets::N0)
            .alpns(vec![SPACE_ADMISSION_ALPN.to_vec()])
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .expect("endpoint"),
    );
    for _ in 0..100 {
        if !endpoint.addr().addrs.is_empty() {
            return endpoint;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("direct address unavailable")
}
fn read_logs(directory: &std::path::Path) -> Vec<serde_json::Value> {
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    std::fs::read_dir(directory)
        .expect("files")
        .flat_map(|entry| {
            let content = std::fs::read_to_string(entry.expect("file").path()).expect("content");
            content
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON"))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test]
async fn missing_stored_credential_is_one_detailed_unassociated_file_record() {
    let profile = tempfile::tempdir().expect("profile");
    let logs = tempfile::tempdir().expect("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(logs.path())),
    )
    .expect("runtime")
    .handle();
    let pool = init_db_pool(
        profile
            .path()
            .join("profile.sqlite")
            .to_str()
            .expect("path"),
    )
    .expect("database");
    let executor = Arc::new(DieselSqliteExecutor::new(pool));
    let keys = Arc::new(AdmissionKeyManager::new(
        Arc::new(MemoryKeys::default()),
        [0x31; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        profile.path().join("vault"),
        keys.clone(),
    ));
    let admissions = Arc::new(SqliteSpaceAdmissionState::new(
        executor.clone(),
        keys.clone(),
        manifests.clone(),
        Arc::new(EmptyLedger),
    ));
    let credentials = Arc::new(SqliteSpaceAdmissionCredentials::new(
        executor,
        keys,
        manifests,
        Arc::new(EmptyLedger),
        admissions,
    ));
    let sponsor = endpoint().await;
    let joiner = endpoint().await;
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(&sponsor, Arc::new(MustNotHandle), credentials)
            .expect("handler"),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler)
        .spawn();
    let route = SpaceAdmissionRoute::from_bytes(
        encode_space_admission_route(&sponsor.addr(), None).expect("route"),
    )
    .expect("route value");
    let binding = AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes(*joiner.id().as_bytes()).expect("joiner"),
        AdmissionChannelPeerId::from_bytes(*sponsor.id().as_bytes()).expect("sponsor"),
    )
    .expect("binding");
    let transport = IrohSpaceAdmissionTransport::new(joiner.clone());
    let exchange = transport
        .resume(
            SpaceAdmissionId::from_bytes([0x42; 32]).expect("attempt"),
            &route,
            binding,
            &AdmissionContinuationCredential::from_bytes(vec![0x51; 64]).expect("credential"),
        )
        .await;
    let mut captured = Vec::new();
    for _ in 0..100 {
        captured = read_logs(logs.path());
        if captured
            .iter()
            .any(|r| r["fields"]["error.reason"] == "record_missing")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let failures: Vec<_> = captured
        .iter()
        .filter(|r| r["fields"]["uc.role"] == "sponsor")
        .collect();
    assert_eq!(
        failures.len(),
        1,
        "storage and conversion must not add duplicate failure records"
    );
    assert_eq!(
        failures[0]["fields"]["event.name"],
        "uc.operation.completed"
    );
    assert_eq!(
        failures[0]["fields"]["error.phase"],
        "continuation_credential"
    );
    assert_eq!(failures[0]["fields"]["error.reason"], "record_missing");
    assert_eq!(failures[0]["fields"]["error.type"], "authentication_failed");
    assert!(failures[0].get("trace_id").is_none());
    assert!(captured.iter().all(|r| r["target"] != "uc.connectivity"));
    drop(exchange);
    router.shutdown().await.expect("router");
    joiner.close().await;
    sponsor.close().await;
    handle.shutdown(Duration::from_secs(5));
}
