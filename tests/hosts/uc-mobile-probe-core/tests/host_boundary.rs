use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
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
fn ios_and_android_share_one_probe_core() {
    let root = workspace_root();
    let workspace = read(root.join("Cargo.toml"));
    let manifest = read(root.join("tests/hosts/uc-mobile-probe-core/Cargo.toml"));

    assert!(workspace.contains("\"tests/hosts/uc-mobile-probe-core\""));
    assert!(!workspace.contains("\"tests/hosts/ios-probe-core\""));
    assert!(manifest.contains("name = \"uc-mobile-probe-core\""));
    assert!(manifest.contains("crate-type = [\"lib\", \"staticlib\", \"cdylib\"]"));
}

#[test]
fn ios_probe_links_the_static_engine_archive() {
    let root = workspace_root();
    let project = read(root.join("tests/hosts/ios/project.rb"));

    assert!(project.contains("-force_load"));
    assert!(project.contains("libuc_mobile_probe_core.a"));
    assert!(!project.contains("-luc_mobile_probe_core"));
}

#[test]
fn ios_probe_supports_device_and_simulator_archives() {
    let root = workspace_root();
    let project = read(root.join("tests/hosts/ios/project.rb"));
    let simulator_build = read(root.join("tests/hosts/ios/build-simulator.sh"));

    assert!(project.contains("aarch64-apple-ios-sim"));
    assert!(project.contains("iphonesimulator"));
    assert!(simulator_build.contains("--target aarch64-apple-ios-sim"));
    assert!(simulator_build.contains("CODE_SIGNING_ALLOWED=YES"));
    assert!(!simulator_build.contains("CODE_SIGNING_ALLOWED=NO"));
}

#[test]
fn ios_simulator_commands_publish_pollable_redacted_evidence() {
    let root = workspace_root();
    let model = read(root.join("tests/hosts/ios/EngineProbe/ProbeModel.swift"));
    let app = read(root.join("tests/hosts/ios/EngineProbe/EngineProbeApp.swift"));
    let info = read(root.join("tests/hosts/ios/EngineProbe/Info.plist"));
    let command = read(root.join("tests/hosts/ios/probe-command-simulator.sh"));

    assert!(app.contains(".onOpenURL"));
    assert!(model.contains("probe-result.json"));
    assert!(model.contains("handleCommandURL"));
    assert!(model.contains("private func runURLCommand"));
    assert!(model.contains("resultKind: \"text_received\""));
    assert!(model.contains("activeRequestID"));
    assert!(info.contains("ucengineprobe"));
    assert!(command.contains("simctl openurl"));
    assert!(command.contains("probe-result.json"));
    assert!(command.contains("uuidgen"));
    assert!(command.contains("request_id"));
    assert!(!model.contains("invitation_code\", \"device_ids"));
}

#[test]
fn android_probe_scripts_require_an_explicit_emulator() {
    let root = workspace_root();
    let command = read(root.join("tests/hosts/android/probe-command.sh"));
    let install = read(root.join("tests/hosts/android/install-emulator.sh"));
    let build = read(root.join("tests/hosts/android/build-emulator.sh"));

    assert!(command.contains("ANDROID_SERIAL"));
    assert!(command.contains("adb -s \"$ANDROID_SERIAL\""));
    assert!(install.contains("ANDROID_SERIAL"));
    assert!(install.contains("adb -s \"$ANDROID_SERIAL\""));
    assert!(build.contains("mktemp -d"));
}

#[test]
fn shared_probe_selects_each_platform_secure_storage() {
    let root = workspace_root();
    let source = read(root.join("tests/hosts/uc-mobile-probe-core/src/lib.rs"));

    assert!(source.contains(
        "#[cfg(target_vendor = \"apple\")]\nfn host_secure_storage() -> Box<dyn HostSecureStorage> {\n    Box::new(KeychainStorage)\n}"
    ));
    assert!(source.contains(
        "#[cfg(not(any(target_vendor = \"apple\", target_os = \"android\")))]\nfn host_secure_storage() -> Box<dyn HostSecureStorage> {\n    Box::new(UnavailableSecureStorage)\n}"
    ));
}

#[test]
fn android_probe_uses_android_keystore_for_persisted_secrets() {
    let root = workspace_root();
    let bridge = read(root.join(
        "tests/hosts/android/app/src/main/java/app/uniclipboard/engineprobe/ProbeBridge.java",
    ));

    assert!(bridge.contains("AndroidKeyStore"));
    assert!(bridge.contains("AES/GCM/NoPadding"));
    assert!(bridge.contains("KeyGenParameterSpec"));
    assert!(bridge.contains("nativeInstallHost(this, context.getApplicationContext())"));
    assert!(!bridge.contains("putString(key, new String(value"));
}

#[test]
fn android_shell_only_forwards_commands_to_the_shared_probe() {
    let root = workspace_root();
    let receiver = read(root.join(
        "tests/hosts/android/app/src/main/java/app/uniclipboard/engineprobe/ProbeReceiver.java",
    ));
    let android_bridge = read(root.join("tests/hosts/uc-mobile-probe-core/src/android.rs"));

    assert!(receiver.contains("bridge.command(command)"));
    assert!(android_bridge.contains("crate::probe_command"));
    assert!(android_bridge.contains("ndk_context::initialize_android_context"));
    assert!(android_bridge.contains("static ANDROID_CONTEXT: OnceLock<GlobalRef>"));
    assert!(!receiver.contains("createSpace"));
    assert!(!receiver.contains("sendText"));
}

#[test]
fn ios_and_android_probe_member_removal_through_the_shared_contract() {
    let root = workspace_root();
    let source = read(root.join("tests/hosts/uc-mobile-probe-core/src/lib.rs"));
    let ios_model = read(root.join("tests/hosts/ios/EngineProbe/ProbeModel.swift"));
    let ios_view = read(root.join("tests/hosts/ios/EngineProbe/ProbeView.swift"));

    for command in ["RemoveMember", "QueryWorkspaceConvergence"] {
        assert!(source.contains(command), "missing command: {command}");
    }
    for field in [
        "phase",
        "revision",
        "change_count",
        "removal_intent_count",
        "effective_member_count",
        "confirmed_member_count",
        "waiting_member_count",
        "convergence_digest",
        "updated_at_ms",
        "last_workspace_phase",
    ] {
        assert!(source.contains(field), "missing field: {field}");
    }
    assert!(ios_model.contains("query_workspace_convergence"));
    assert!(ios_view.contains("Member removal"));
}

#[test]
fn android_pairing_keeps_the_probe_alive_with_a_data_sync_service() {
    let root = workspace_root();
    let manifest = read(root.join("tests/hosts/android/app/src/main/AndroidManifest.xml"));
    let activity = read(root.join(
        "tests/hosts/android/app/src/main/java/app/uniclipboard/engineprobe/ProbeActivity.java",
    ));
    let receiver = read(root.join(
        "tests/hosts/android/app/src/main/java/app/uniclipboard/engineprobe/ProbeReceiver.java",
    ));
    let service = read(root.join(
        "tests/hosts/android/app/src/main/java/app/uniclipboard/engineprobe/ProbeService.java",
    ));

    assert!(manifest.contains("android:foregroundServiceType=\"dataSync\""));
    assert!(manifest.contains("android:name=\".ProbeActivity\""));
    assert!(activity.contains("startForegroundService"));
    assert!(!receiver.contains("startForegroundService"));
    assert!(service.contains("startForeground"));
    assert!(!service.contains("ProbeBridge"));
}
