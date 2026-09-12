//! 真实资料读取、真实本机协议与实际导出文件共同验证一条认证失败。
use async_trait::async_trait;
use iroh::{endpoint::presets, protocol::Router, Endpoint, RelayMode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uc_application::deps::*;
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, AdmissionMessageId,
    AdmissionPeerBinding, AdmissionRole, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1,
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
use uc_observability_contract::diagnostics::{
    SpaceAdmissionObservation, SpaceAdmissionObservationOutcome,
};
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
async fn missing_stored_credential_produces_diagnosable_client_and_server_records() {
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
    handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("detailed capture");
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
    let admission_id = SpaceAdmissionId::from_bytes([0x42; 32]).expect("attempt");
    let observation = SpaceAdmissionObservation::begin(admission_id.as_bytes());
    let exchange = observation
        .scope(async {
            let exchange = transport
                .resume(
                    admission_id,
                    &route,
                    binding.clone(),
                    &AdmissionContinuationCredential::from_bytes(vec![0x51; 64])
                        .expect("credential"),
                )
                .await
                .expect(
                    "the physical connection should be established before authentication fails",
                );
            let request = SpaceAdmissionEnvelopeV1::new(
                admission_id,
                AdmissionRole::Joiner,
                1,
                AdmissionMessageId::from_bytes([0x43; 32]).expect("message"),
                Some(AdmissionMessageId::from_bytes([0x44; 32]).expect("predecessor")),
                SpaceAdmissionBodyV1::CancelRequested,
            )
            .expect("request");
            exchange.exchange(&request).await
        })
        .await;
    assert!(matches!(
        exchange,
        Err(SpaceAdmissionTransportError::AuthenticationRejected)
    ));
    observation.finish(SpaceAdmissionObservationOutcome::Deferred);
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
    let client_failure = captured
        .iter()
        .find(|record| {
            record["fields"]["uc.role"] == "joiner"
                && record["fields"]["uc.operation"] == "network_transport"
                && record["fields"]["uc.outcome"] == "error"
        })
        .expect("the joiner package must explain the remote authentication rejection");
    assert_eq!(
        client_failure["fields"]["error.type"],
        "authentication_failed"
    );
    let connection_records: Vec<_> = captured
        .iter()
        .filter(|r| {
            r["fields"]["event.name"] == "connection.started"
                || r["fields"]["event.name"] == "connection.finished"
        })
        .collect();
    assert_eq!(
        connection_records.len(),
        2,
        "导出必须区分连接成功和之后的认证失败"
    );
    assert_eq!(
        connection_records[0]["fields"]["event.name"],
        "connection.started"
    );
    assert_eq!(connection_records[1]["fields"]["outcome"], "connected");
    assert_eq!(
        connection_records[0]["fields"]["connect_id"],
        connection_records[1]["fields"]["connect_id"]
    );
    assert_eq!(
        connection_records[0]["peer_ref"],
        connection_records[1]["peer_ref"]
    );
    assert!(connection_records[0]["peer_ref"].as_str().is_some());
    assert!(connection_records[0]["run_id"].as_str().is_some());
    assert_eq!(connection_records[0]["fields"]["purpose"], "admission");
    assert_eq!(connection_records[1]["fields"]["attempt_count"], 1);
    assert_eq!(connection_records[0]["capture_mode"], "detailed");
    let connect_id = &connection_records[0]["fields"]["connect_id"];
    let connection_story: Vec<_> = captured
        .iter()
        .filter(|record| record["fields"]["connect_id"] == *connect_id)
        .collect();
    for event in [
        "connection.started",
        "address.used",
        "connection.attempt.started",
        "connection.attempt.finished",
        "connection.finished",
    ] {
        assert!(
            connection_story
                .iter()
                .any(|record| record["fields"]["event.name"] == event),
            "detailed capture must preserve {event}"
        );
    }
    let used = connection_story
        .iter()
        .find(|record| record["fields"]["event.name"] == "address.used")
        .expect("address source");
    assert_eq!(used["fields"]["source"], "admission_route");
    assert!(used["candidate_set_ref"].as_str().is_some());
    let attempt = connection_story
        .iter()
        .find(|record| record["fields"]["event.name"] == "connection.attempt.finished")
        .expect("attempt result");
    assert_eq!(attempt["fields"]["outcome"], "connected");
    let trace_id = connection_records[0]["trace_id"].clone();
    assert!(trace_id.as_str().is_some());
    assert_eq!(client_failure["trace_id"], trace_id);
    let serialized = serde_json::to_string(&captured).expect("records");
    assert!(!serialized.contains(&sponsor.id().to_string()));
    assert!(!serialized.contains(&joiner.id().to_string()));
    router.shutdown().await.expect("router");
    joiner.close().await;
    sponsor.close().await;
    let closed = transport
        .resume(
            SpaceAdmissionId::from_bytes([0x42; 32]).expect("attempt"),
            &route,
            binding,
            &AdmissionContinuationCredential::from_bytes(vec![0x51; 64]).expect("credential"),
        )
        .await;
    assert!(closed.is_err());
    let records = read_logs(logs.path());
    let failed = records
        .iter()
        .find(|r| {
            r["fields"]["event.name"] == "connection.finished" && r["fields"]["outcome"] == "failed"
        })
        .expect("失败连接必须保留终态");
    assert_eq!(failed["fields"]["error.phase"], "establish");
    assert_eq!(failed["fields"]["error.reason"], "endpoint_closed");
    assert_eq!(failed["peer_ref"], connection_records[0]["peer_ref"]);
    assert_ne!(
        failed["fields"]["connect_id"],
        connection_records[0]["fields"]["connect_id"]
    );
    handle.shutdown(Duration::from_secs(5));
}
