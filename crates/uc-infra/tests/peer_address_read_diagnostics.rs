use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
use diesel::{connection::SimpleConnection, RunQueryDsl};
use opentelemetry::logs::AnyValue;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
use serde_json::{json, Value};
use tempfile::TempDir;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::{layer::SubscriberExt, Registry};
use uc_core::{
    ids::{DeviceId, ProfileId},
    ports::{
        security::current_profile::{CurrentProfileError, CurrentProfilePort},
        space::{DeriveSpaceSubkeyPort, SpaceAccessError},
        PeerAddressRecord, PeerAddressRepositoryPort,
    },
};
use uc_infra::db::{
    executor::DieselSqliteExecutor,
    pool::init_db_pool,
    ports::DbExecutor,
    repositories::{DieselPeerAddressRepository, EncryptedRelationshipStore},
};

const KEY: [u8; 32] = [0x61; 32];

struct Profile;

#[async_trait]
impl CurrentProfilePort for Profile {
    async fn current_profile(&self) -> Result<ProfileId, CurrentProfileError> {
        Ok(ProfileId::from("diagnostic-fixture"))
    }
}

#[derive(Clone, Copy)]
enum SubkeyMode {
    Ready,
    Locked,
    Unknown,
}

struct Subkey(SubkeyMode);

#[async_trait]
impl DeriveSpaceSubkeyPort for Subkey {
    async fn derive_subkey(&self, _: &[u8], _: &[u8]) -> Result<[u8; 32], SpaceAccessError> {
        match self.0 {
            SubkeyMode::Ready => Ok(KEY),
            SubkeyMode::Locked => Err(SpaceAccessError::NotUnlocked),
            SubkeyMode::Unknown => Err(SpaceAccessError::CorruptedKeyMaterial),
        }
    }
}

struct Fixture {
    _temp: TempDir,
    executor: Arc<DieselSqliteExecutor>,
    repo: DieselPeerAddressRepository<Arc<DieselSqliteExecutor>>,
}

fn fixture(mode: SubkeyMode) -> Fixture {
    let temp = tempfile::tempdir().expect("temporary profile");
    let database = temp.path().join("profile.sqlite");
    let pool = init_db_pool(database.to_str().expect("database path")).expect("database");
    let executor = Arc::new(DieselSqliteExecutor::new(pool));
    let store = Arc::new(EncryptedRelationshipStore::new(
        Arc::clone(&executor),
        Arc::new(Subkey(mode)),
        Arc::new(Profile),
    ));
    Fixture {
        _temp: temp,
        executor,
        repo: DieselPeerAddressRepository::new(store),
    }
}

fn record() -> PeerAddressRecord {
    PeerAddressRecord {
        device_id: DeviceId::new("fictional-peer"),
        addr_blob: vec![1, 2, 3],
        observed_at: chrono::Utc::now(),
    }
}

fn replace_envelope(executor: &DieselSqliteExecutor, envelope: Vec<u8>) {
    executor
        .run(move |conn| {
            diesel::sql_query(
                "UPDATE encrypted_relationship SET payload_ciphertext = ? WHERE kind = 'peer_address'",
            )
            .bind::<diesel::sql_types::Binary, _>(envelope)
            .execute(conn)?;
            Ok(())
        })
        .expect("replace envelope");
}

fn valid_envelope(plaintext: &[u8]) -> Vec<u8> {
    let mut lookup_input = b"peer_address\0".to_vec();
    lookup_input.extend_from_slice(b"fictional-peer");
    let lookup = blake3::keyed_hash(&KEY, &lookup_input);
    let mut aad = b"uniclipboard-relationship/v1\0peer_address\0".to_vec();
    aad.extend_from_slice(lookup.as_bytes());
    let nonce = [7u8; 24];
    let ciphertext = XChaCha20Poly1305::new_from_slice(&KEY)
        .expect("key")
        .encrypt(
            XNonce::from_slice(&nonce),
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .expect("encrypt fixture");
    let mut envelope = b"UCRL".to_vec();
    envelope.push(1);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    envelope
}

async fn diagnostic(mut fixture: Fixture, mutate: impl FnOnce(&mut Fixture)) -> Value {
    let exporter = InMemoryLogExporter::default();
    let provider = SdkLoggerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = Registry::default().with(OpenTelemetryTracingBridge::new(&provider));
    let _ = fixture.repo.upsert(&record()).await;
    mutate(&mut fixture);
    let error = fixture
        .repo
        .get(&record().device_id)
        .with_subscriber(subscriber)
        .await
        .expect_err("injected failure");
    let mut source = std::error::Error::source(&error);
    let mut source_depth = 0;
    while let Some(current) = source {
        source_depth += 1;
        source = current.source();
    }
    assert!(
        source_depth >= 2,
        "repository source chain must be preserved"
    );
    let rows = exporter.get_emitted_logs().expect("logs");
    let payload = rows
        .iter()
        .find_map(|row| {
            let is_failure = row.record.attributes_iter().any(|(key, value)| {
                key.as_str() == "event.name"
                    && matches!(value, AnyValue::String(value) if value.as_str() == "address.record.read_failed")
            });
            is_failure.then(|| {
                row.record.attributes_iter().find_map(|(key, value)| match value {
                    AnyValue::String(value) if key.as_str() == "payload" => {
                        uc_observability_contract::diagnostics::connectivity::decode_local_record(
                            "address.record.read_failed",
                            value.as_str(),
                            "WARN",
                        )
                    }
                    _ => None,
                })
            })?
        })
        .expect("encoded failure payload");
    let payload = Value::Object(payload);
    assert!(!payload.to_string().contains("fictional-peer"));
    assert!(!payload.to_string().contains("profile.sqlite"));
    assert!(!payload.to_string().contains("/Users/"));
    payload
}

#[tokio::test]
async fn peer_address_failures_export_stable_categories_and_real_stack_symbols() {
    let locked = diagnostic(fixture(SubkeyMode::Locked), |_| {}).await;
    let unknown = diagnostic(fixture(SubkeyMode::Unknown), |_| {}).await;

    let authentication = diagnostic(fixture(SubkeyMode::Ready), |fixture| {
        let mut envelope = valid_envelope(b"{}");
        let last = envelope.len() - 1;
        envelope[last] ^= 1;
        replace_envelope(&fixture.executor, envelope);
    })
    .await;

    let unsupported = diagnostic(fixture(SubkeyMode::Ready), |fixture| {
        let mut envelope = valid_envelope(b"{}");
        envelope[4] = 2;
        replace_envelope(&fixture.executor, envelope);
    })
    .await;

    let decode = diagnostic(fixture(SubkeyMode::Ready), |fixture| {
        replace_envelope(&fixture.executor, valid_envelope(b"not-json"));
    })
    .await;

    let storage = diagnostic(fixture(SubkeyMode::Ready), |fixture| {
        fixture
            .executor
            .run(|conn| {
                conn.batch_execute("DROP TABLE encrypted_relationship")?;
                Ok(())
            })
            .expect("drop relation table");
    })
    .await;

    let artifact = json!({
        "locked": locked,
        "authentication": authentication,
        "unsupported_version": unsupported,
        "payload_decode": decode,
        "storage": storage,
        "unknown": unknown,
    });
    for (name, category, stage) in [
        ("locked", "locked", "key_derivation"),
        (
            "authentication",
            "authentication",
            "ciphertext_authentication",
        ),
        (
            "unsupported_version",
            "unsupported_version",
            "envelope_decode",
        ),
        ("payload_decode", "payload_decode", "payload_decode"),
        ("storage", "storage", "database_read"),
        ("unknown", "unknown", "key_derivation"),
    ] {
        let row = &artifact[name];
        assert_eq!(row["error.category"], category);
        assert_eq!(row["error.stage"], stage);
        assert_eq!(
            row["error.chain"],
            json!(["peer_address_repository", "relationship_store", category])
        );
        assert_eq!(row["error.stack_status"], "captured");
        assert!(row["error.stack"]
            .as_array()
            .is_some_and(|stack| !stack.is_empty()));
    }
    if let Ok(path) = std::env::var("UC_PEER_ADDRESS_DIAGNOSTIC_ARTIFACT") {
        std::fs::write(
            PathBuf::from(path),
            serde_json::to_vec_pretty(&artifact).expect("artifact json"),
        )
        .expect("write artifact");
    }
}
