use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should exist")
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path).unwrap_or_else(|error| {
        panic!("failed to read {}: {error}", path.display());
    })
}

#[test]
fn android_binding_installs_the_jni_context_before_engine_start() {
    let root = workspace_root();
    let manifest = read(root.join("bindings/uc-engine-uniffi/Cargo.toml"));
    let library = read(root.join("bindings/uc-engine-uniffi/src/lib.rs"));
    let android = read(root.join("bindings/uc-engine-uniffi/src/android.rs"));
    let runtime_manifest = read(root.join("crates/uc-observability-runtime/Cargo.toml"));
    let runtime_android = read(root.join("crates/uc-observability-runtime/src/android.rs"));
    let gradle = read(root.join("bindings/uc-engine-uniffi/android/build.gradle"));
    let packaging = read(root.join("bindings/uc-engine-uniffi/scripts/build-android-aar.sh"));
    let proguard = read(root.join("bindings/uc-engine-uniffi/android/consumer-rules.pro"));

    assert!(manifest.contains("ndk-context"));
    assert!(manifest.contains("jni"));
    assert!(library.contains("#[cfg(target_os = \"android\")]\nmod android;"));
    assert!(android.contains("ndk_context::initialize_android_context"));
    assert!(android.contains("static ANDROID_CONTEXT: OnceLock<GlobalRef>"));
    assert!(
        android.contains("Java_expo_modules_ucengine_UcEngineModule_nativeInstallAndroidContext")
    );
    assert!(android.contains("ensure_android_context_installed"));
    assert!(android.contains("initialize_android_tls"));
    assert!(runtime_manifest.contains("rustls-platform-verifier"));
    assert!(runtime_android.contains("rustls_platform_verifier::android::init_with_env"));
    assert!(packaging.contains("rustls-platform-verifier-android"));
    assert!(packaging.contains("cargo metadata --locked"));
    assert!(packaging.contains("unzip -p \"$RUSTLS_ANDROID_AAR\" classes.jar"));
    assert!(packaging.contains("libs/rustls-platform-verifier-classes.jar"));
    assert!(packaging.contains("org/rustls/platformverifier/CertificateVerifier.class"));
    assert!(gradle.contains("UC_ENGINE_UNIFFI_RUSTLS_VERIFIER_JAR"));
    assert!(proguard.contains("org.rustls.platformverifier"));
    assert!(observability_installer(&root)
        .contains("crate::android::ensure_android_context_installed()"));
    assert!(!android.contains("set_global_default"));
    assert!(!android.contains("tracing_android::layer"));
}

fn observability_installer(root: &Path) -> String {
    read(root.join("bindings/uc-engine-uniffi/src/observability.rs"))
}
