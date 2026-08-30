//! Engine-level config-migration round trip: real `export_bundle` →
//! `stage_import` → boot-time `apply_pending_import` → landed-state assertions.
//!
//! The per-layer unit tests exercise each step against synthetic fakes; this
//! test wires the *real* pipeline end to end through the seam the rework units
//! own:
//!
//! * the source is a genuinely initialized installation — a real `KeySlot` and a
//!   matching KEK produced by `DefaultSpaceAccessAdapter::initialize`. The bundle
//!   is sealed with that KEK (no export password), so opening it requires the
//!   space passphrase that derives the KEK — the contract this exercises;
//! * the iroh device identity migrates as 0600 *files* (not a credential-store
//!   secret), so the same network identity survives a portable→installer move
//!   without re-pairing;
//! * the KEK travels in `secrets.json` and is bridged into whatever
//!   secure-storage backend the target boot wiring selected.
//!
//! Source and target each use their own temp data root and their own
//! `FileSecureStorage`-backed keyring so the two backends are genuinely
//! distinct — the test proves the KEK crosses from one backend to the other,
//! and the identity file's exact bytes survive the trip.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use diesel::sql_query;
use diesel::RunQueryDsl;

use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{ProfileId, SpaceId};
use uc_core::ports::config_migration::{
    ConfigMigrationError, ConfigSourceMode, ExportConfigBundlePort, StageConfigImportPort,
};
use uc_core::ports::space::SpaceAccessStore;
use uc_core::ports::{LocalIdentityPort, SecureStorageError, SecureStoragePort};
use uc_infra::config_migration::staging::apply_pending_import;
use uc_infra::config_migration::staging::StagingLayout;
use uc_infra::config_migration::{ConfigMigrationAdapter, ConfigMigrationPaths};
use uc_infra::db::pool::{init_db_pool, DbPool};
use uc_infra::fs::key_slot_store::JsonKeySlotStore;
use uc_infra::network::iroh::IrohIdentityStore;
use uc_infra::security::{DefaultCurrentProfile, Sha256IdentityFingerprintFactory};
use uc_infra::space::{DefaultSpaceAccessAdapter, InMemorySession, KeyMaterialStore};
use uc_infra::SystemClock;
#[derive(Default)]
struct TestSecureStorage {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl SecureStoragePort for TestSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

fn test_storage() -> Arc<dyn SecureStoragePort> {
    Arc::new(TestSecureStorage::default())
}

/// The single secure-storage key carried as a secret for the `default` profile.
const KEK_KEY: &str = "kek:v1:profile:default";

/// A device-identity file living in the source `iroh-identity/` directory. Its
/// exact bytes must reappear in the target identity dir (proof the identity
/// migrates as a file, not a re-mint).
const IROH_IDENTITY_FILE: &str = "iroh-identity_v1.bin";
const IROH_IDENTITY_BYTES: [u8; 32] = [0x5A; 32];

const DEVICE_ID_TXT: &[u8] = b"550e8400-e29b-41d4-a716-446655440000";
const CURRENT_SPACE_ID_BYTES: &[u8] = b"encrypted-current-space-id";
const SETTINGS_JSON: &[u8] = b"{\"schema_version\":1}";

/// A unique value committed into the source db so the round-trip can prove the
/// snapshot — not just *a* db file — actually travelled.
const DB_PROBE_VALUE: &str = "round-trip-marker-7f3a";

/// Source-side filesystem layout under one data root.
struct Source {
    _dir: tempfile::TempDir,
    data_root: std::path::PathBuf,
    db_path: std::path::PathBuf,
    vault_dir: std::path::PathBuf,
    settings_path: std::path::PathBuf,
    iroh_identity_dir: std::path::PathBuf,
    /// Source keyring backend; holds the real KEK after init so the test can
    /// compare it to what bridges into the target backend.
    kek_storage: Arc<dyn SecureStoragePort>,
    adapter: ConfigMigrationAdapter,
}

/// Build the source installation: a real sqlite db with a committed probe row,
/// a *real* initialized keyslot + KEK (via `DefaultSpaceAccessAdapter`), real
/// vault/settings files, and an iroh identity file. Returns a fully-wired export
/// adapter. `passphrase` is the space passphrase the KEK is derived from — the
/// same passphrase later opens the exported bundle.
async fn build_source(passphrase: &Passphrase) -> Source {
    let dir = tempfile::tempdir().unwrap();
    let data_root = dir.path().join("source");
    let vault_dir = data_root.join("vault");
    let iroh_identity_dir = data_root.join("iroh-identity");
    std::fs::create_dir_all(&vault_dir).unwrap();
    std::fs::create_dir_all(&iroh_identity_dir).unwrap();

    // Real db pool: creates uniclipboard.db and runs the embedded migrations,
    // then commit a probe row so the snapshot carries recoverable data.
    let db_path = data_root.join("uniclipboard.db");
    let pool: DbPool = init_db_pool(db_path.to_str().unwrap()).unwrap();
    {
        let mut conn = pool.get().unwrap();
        sql_query("CREATE TABLE round_trip_probe (id INTEGER PRIMARY KEY, v TEXT NOT NULL)")
            .execute(&mut conn)
            .unwrap();
        sql_query(format!(
            "INSERT INTO round_trip_probe (v) VALUES ('{DB_PROBE_VALUE}')"
        ))
        .execute(&mut conn)
        .unwrap();
    }

    // The test owns a separate secure-storage backend for each installation.
    // The real initialization flow writes the KEK here and the keyslot into
    // the vault dir, so the two are mutually consistent.
    let kek_storage = test_storage();

    // Real initialization: derives the KEK from `passphrase`, generates and wraps
    // a master key, writes `vault/keyslot.json`, and stores the KEK in the
    // keyring. This is what makes "open the bundle with the passphrase" hold.
    {
        let keyslot_store = Arc::new(JsonKeySlotStore::new(vault_dir.clone()));
        let key_material = Arc::new(KeyMaterialStore::new(kek_storage.clone(), keyslot_store));
        let session = Arc::new(InMemorySession::new());
        let space_access = DefaultSpaceAccessAdapter::new(
            key_material,
            Arc::new(DefaultCurrentProfile::new()),
            session,
        );
        SpaceAccessStore::initialize(&space_access, &SpaceId::from("space"), passphrase)
            .await
            .unwrap();
    }

    // Remaining vault + settings + identity files (carried verbatim).
    std::fs::write(vault_dir.join("device_id.txt"), DEVICE_ID_TXT).unwrap();
    std::fs::write(
        vault_dir.join(".current-space-id-v1"),
        CURRENT_SPACE_ID_BYTES,
    )
    .unwrap();
    std::fs::write(data_root.join("settings.json"), SETTINGS_JSON).unwrap();
    std::fs::write(
        iroh_identity_dir.join(IROH_IDENTITY_FILE),
        IROH_IDENTITY_BYTES,
    )
    .unwrap();

    let identity_storage = test_storage();
    let local_identity: Arc<dyn LocalIdentityPort> = Arc::new(IrohIdentityStore::new(
        identity_storage,
        Arc::new(Sha256IdentityFingerprintFactory),
    ));

    let paths = ConfigMigrationPaths {
        db_path: db_path.clone(),
        vault_dir: vault_dir.clone(),
        iroh_identity_dir: iroh_identity_dir.clone(),
        settings_path: data_root.join("settings.json"),
        app_data_root: data_root.clone(),
    };

    let settings_path = data_root.join("settings.json");
    let adapter = ConfigMigrationAdapter::new(
        kek_storage.clone(),
        pool,
        local_identity,
        Arc::new(SystemClock),
        paths,
        ProfileId::from("default".to_string()),
        ConfigSourceMode::Installed,
    );

    Source {
        _dir: dir,
        data_root,
        db_path,
        vault_dir,
        settings_path,
        iroh_identity_dir,
        kek_storage,
        adapter,
    }
}

/// Target installation: a pristine, empty data root plus an independent
/// secure-storage backend. Live destinations for the apply step are derived
/// from this root.
struct Target {
    _dir: tempfile::TempDir,
    data_root: std::path::PathBuf,
    db_path: std::path::PathBuf,
    vault_dir: std::path::PathBuf,
    settings_path: std::path::PathBuf,
    iroh_identity_dir: std::path::PathBuf,
    core_secure_storage: Arc<dyn SecureStoragePort>,
    secure_storage: Arc<dyn SecureStoragePort>,
    adapter: ConfigMigrationAdapter,
}

/// Build the (empty) target installation + a stage adapter pointed at it. The
/// target db pool is created so the adapter is well-formed, but the apply step
/// overwrites the db file from the staged snapshot.
fn build_target() -> Target {
    let dir = tempfile::tempdir().unwrap();
    let data_root = dir.path().join("target");
    let vault_dir = data_root.join("vault");
    let iroh_identity_dir = data_root.join("iroh-identity");
    std::fs::create_dir_all(&data_root).unwrap();

    let db_path = data_root.join("uniclipboard.db");
    // Throwaway pool only to satisfy the adapter's constructor: `stage_import`
    // never touches the db pool (only `export_bundle` does). Critically, it is
    // pointed at a *scratch* path, NOT the real `db_path`: opening a pool
    // creates WAL/SHM sidecars, and in production `apply_pending_import` runs on
    // boot *before* any pool opens the target db — so the apply target must have
    // no live sidecars. Isolating the scratch pool reproduces that ordering.
    let pool: DbPool =
        init_db_pool(data_root.join("adapter-scratch.db").to_str().unwrap()).unwrap();

    // Independent target backend. The boot apply step writes the bridged KEK
    // here, so it must be distinct from the source backend.
    let core_secure_storage = test_storage();
    let secure_storage = Arc::clone(&core_secure_storage);

    let local_identity: Arc<dyn LocalIdentityPort> = Arc::new(IrohIdentityStore::new(
        core_secure_storage.clone(),
        Arc::new(Sha256IdentityFingerprintFactory),
    ));

    let paths = ConfigMigrationPaths {
        db_path: db_path.clone(),
        vault_dir: vault_dir.clone(),
        iroh_identity_dir: iroh_identity_dir.clone(),
        settings_path: data_root.join("settings.json"),
        app_data_root: data_root.clone(),
    };

    let adapter = ConfigMigrationAdapter::new(
        core_secure_storage.clone(),
        pool,
        local_identity,
        Arc::new(SystemClock),
        paths,
        ProfileId::from("default".to_string()),
        ConfigSourceMode::Installed,
    );

    Target {
        _dir: dir,
        data_root: data_root.clone(),
        db_path,
        vault_dir,
        settings_path: data_root.join("settings.json"),
        iroh_identity_dir,
        core_secure_storage,
        secure_storage,
        adapter,
    }
}

/// Open `db_path` as a standalone sqlite db and read the probe row back.
fn read_db_probe(db_path: &std::path::Path) -> Vec<String> {
    let pool: DbPool = init_db_pool(db_path.to_str().unwrap()).unwrap();
    let mut conn = pool.get().unwrap();

    #[derive(diesel::QueryableByName)]
    struct Row {
        #[diesel(sql_type = diesel::sql_types::Text)]
        v: String,
    }
    let rows: Vec<Row> = sql_query("SELECT v FROM round_trip_probe")
        .load(&mut conn)
        .unwrap();
    rows.into_iter().map(|r| r.v).collect()
}

#[tokio::test]
async fn export_stage_apply_round_trip_lands_db_vault_identity_and_kek() {
    let passphrase = Passphrase::from("correct horse battery staple");
    let src = build_source(&passphrase).await;
    let tgt = build_target();

    // Capture the source's real keyslot + KEK so the target can be compared
    // byte-for-byte (they are generated, not fixed constants).
    let source_keyslot = std::fs::read(src.vault_dir.join("keyslot.json")).unwrap();
    let source_kek = src
        .kek_storage
        .get(KEK_KEY)
        .unwrap()
        .expect("source must hold a KEK after init");

    // 1. Export the source installation into a sealed `.ucbundle` (no password).
    let bundle_path = src.data_root.join("exports").join("config.ucbundle");
    let written = src
        .adapter
        .export_bundle(&bundle_path)
        .await
        .expect("export should succeed");
    assert_eq!(written, bundle_path);
    assert!(bundle_path.exists(), "bundle file must be written");

    // 2. Stage the bundle into the (empty) target data root, opening it with the
    //    space passphrase that derives the KEK.
    let staged = tgt
        .adapter
        .stage_import(&passphrase, &bundle_path)
        .await
        .expect("stage should succeed");

    // KEK rode along, so no post-apply unlock is required.
    assert!(
        !staged.unlock_required_after_apply,
        "bundle carried the KEK, so apply must not require a later unlock"
    );

    // Staging artifacts exist under the target root before apply.
    let layout = StagingLayout::new(&tgt.data_root);
    assert!(
        layout.staging_dir().exists(),
        "import-staging/ must exist after stage"
    );
    assert!(
        layout.marker_path().exists(),
        "pending-import.json marker must exist after stage"
    );

    // 3. Apply the staged import the way boot does.
    apply_pending_import(
        &tgt.data_root,
        &tgt.db_path,
        &tgt.vault_dir,
        &tgt.settings_path,
        &tgt.iroh_identity_dir,
        tgt.core_secure_storage.as_ref(),
    )
    .expect("apply_pending_import should succeed");

    // 4a. The db snapshot round-tripped: target db opens and holds the probe row.
    let rows = read_db_probe(&tgt.db_path);
    assert_eq!(
        rows,
        vec![DB_PROBE_VALUE.to_string()],
        "target db must hold exactly the source's committed probe row"
    );

    // 4b. Vault members are byte-identical to the source.
    assert_eq!(
        std::fs::read(tgt.vault_dir.join("keyslot.json")).unwrap(),
        source_keyslot,
        "target keyslot must be byte-identical to the source's real keyslot"
    );
    assert_eq!(
        std::fs::read(tgt.vault_dir.join("device_id.txt")).unwrap(),
        DEVICE_ID_TXT
    );
    assert_eq!(
        std::fs::read(tgt.vault_dir.join(".current-space-id-v1")).unwrap(),
        CURRENT_SPACE_ID_BYTES
    );
    assert_eq!(std::fs::read(&tgt.settings_path).unwrap(), SETTINGS_JSON);

    // 4c. CORE: the iroh device-identity file landed with byte-identical
    // content — the same network identity, migrated as a file, not re-minted.
    let landed_identity = tgt.iroh_identity_dir.join(IROH_IDENTITY_FILE);
    assert!(
        landed_identity.exists(),
        "iroh identity file must land in the target identity dir"
    );
    assert_eq!(
        std::fs::read(&landed_identity).unwrap(),
        IROH_IDENTITY_BYTES,
        "iroh identity bytes must be byte-identical to the source"
    );

    // 4d. CORE: the KEK bridged into the *target* secure-storage backend with
    // byte-identical content (distinct backend dir from the source keyring).
    let landed_kek = tgt
        .secure_storage
        .get(KEK_KEY)
        .unwrap()
        .expect("KEK must be present in the target backend after apply");
    assert_eq!(
        landed_kek, source_kek,
        "KEK bytes must be byte-identical after bridging into the target backend"
    );

    // 4e. Staging area + marker cleaned up after a successful apply.
    assert!(
        !layout.staging_dir().exists(),
        "import-staging/ must be removed after apply"
    );
    assert!(
        !layout.marker_path().exists(),
        "pending-import.json marker must be removed after apply"
    );

    // Touch source-only fields so the helper struct stays meaningful.
    assert!(src.db_path.exists());
    assert!(src.iroh_identity_dir.exists());
    assert!(src.settings_path.exists());
}

#[tokio::test]
async fn stage_import_with_wrong_passphrase_is_invalid_or_corrupt() {
    let src = build_source(&Passphrase::from("right-passphrase")).await;
    let tgt = build_target();

    let bundle_path = src.data_root.join("exports").join("config.ucbundle");
    src.adapter
        .export_bundle(&bundle_path)
        .await
        .expect("export should succeed");

    let err = tgt
        .adapter
        .stage_import(&Passphrase::from("wrong-passphrase"), &bundle_path)
        .await
        .expect_err("staging with the wrong passphrase must fail");

    assert!(
        matches!(err, ConfigMigrationError::InvalidPasswordOrCorrupt),
        "expected InvalidPasswordOrCorrupt, got {err:?}"
    );

    // A failed stage must not leave a marker that boot would later try to apply.
    let layout = StagingLayout::new(&tgt.data_root);
    assert!(
        !layout.marker_path().exists(),
        "a failed stage must not leave a pending-import marker"
    );
}
