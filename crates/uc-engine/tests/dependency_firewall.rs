use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use cargo_metadata::{DependencyKind, MetadataCommand, PackageId};

const CORE_PACKAGES: [&str; 3] = ["uc-application", "uc-infra", "uc-engine"];
const DESKTOP_ONLY_PACKAGES: [&str; 7] = [
    "uc-app-paths",
    "uc-bootstrap",
    "uc-observability",
    "uc-platform",
    "uc-webserver",
    "uc-daemon-local",
    "uc-desktop",
];

#[test]
fn core_packages_do_not_declare_desktop_path_or_observability_dependencies() {
    let metadata = workspace_metadata();

    for package_name in CORE_PACKAGES {
        let package = metadata
            .packages
            .iter()
            .find(|package| package.name == package_name)
            .unwrap_or_else(|| panic!("{package_name} must be a workspace package"));
        let violations = package
            .dependencies
            .iter()
            .filter(|dependency| {
                matches!(
                    dependency.name.as_str(),
                    "uc-app-paths" | "uc-observability"
                )
            })
            .map(|dependency| dependency.name.as_str())
            .collect::<BTreeSet<_>>();

        assert!(
            violations.is_empty(),
            "{package_name} must not depend on desktop-owned packages: {violations:?}"
        );
    }
}

#[test]
fn engine_normal_dependency_closure_excludes_desktop_packages() {
    let metadata = workspace_metadata();
    let resolve = metadata
        .resolve
        .expect("workspace dependency graph is required");
    let package_names = metadata
        .packages
        .iter()
        .map(|package| (package.id.clone(), package.name.as_str()))
        .collect::<HashMap<_, _>>();
    let nodes = resolve
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node))
        .collect::<HashMap<_, _>>();
    let engine_id = package_names
        .iter()
        .find_map(|(id, name)| (*name == "uc-engine").then(|| id.clone()))
        .expect("uc-engine must be in the dependency graph");

    let mut pending = vec![engine_id];
    let mut visited = HashSet::<PackageId>::new();
    while let Some(package_id) = pending.pop() {
        if !visited.insert(package_id.clone()) {
            continue;
        }
        let node = nodes
            .get(&package_id)
            .unwrap_or_else(|| panic!("missing dependency node for {package_id}"));
        pending.extend(
            node.deps
                .iter()
                .filter(|dependency| {
                    dependency
                        .dep_kinds
                        .iter()
                        .any(|kind| kind.kind == DependencyKind::Normal)
                })
                .map(|dependency| dependency.pkg.clone()),
        );
    }

    let violations = visited
        .iter()
        .filter_map(|id| package_names.get(id).copied())
        .filter(|name| DESKTOP_ONLY_PACKAGES.contains(name))
        .collect::<BTreeSet<_>>();
    assert!(
        violations.is_empty(),
        "uc-engine normal dependency closure contains desktop packages: {violations:?}"
    );
}

#[test]
fn engine_default_dependency_contract_excludes_lan_compat_dependencies() {
    let metadata = workspace_metadata();
    let engine = package(&metadata, "uc-engine");
    let application = package(&metadata, "uc-application");
    let infra = package(&metadata, "uc-infra");
    let mobile_lan = package(&metadata, "uc-mobile-lan");

    assert_default_does_not_enable(engine, "lan-compat");
    assert_default_does_not_enable(application, "lan-compat");
    assert_default_does_not_enable(infra, "lan-compat");

    let application_dependency = normal_dependency(engine, "uc-application");
    let infra_dependency = normal_dependency(engine, "uc-infra");
    let mobile_lan_dependency = normal_dependency(engine, "uc-mobile-lan");
    assert!(!application_dependency
        .features
        .contains(&"lan-compat".to_string()));
    assert!(!infra_dependency
        .features
        .contains(&"lan-compat".to_string()));
    assert!(
        mobile_lan_dependency.optional,
        "uc-mobile-lan must remain optional in uc-engine"
    );

    // ADR-018 stage 4: the LAN workflows live in the dedicated
    // `uc-mobile-lan` crate; `uc-application` no longer carries any
    // LAN-only dependency.
    let application_has_mobile_proto = application.dependencies.iter().any(|dependency| {
        dependency.name == "uc-mobile-proto" && dependency.kind == DependencyKind::Normal
    });
    assert!(
        !application_has_mobile_proto,
        "uc-application must not depend on uc-mobile-proto (moved to uc-mobile-lan)"
    );
    let network_interface = normal_dependency(infra, "network-interface");
    assert!(
        network_interface.optional,
        "network-interface must remain optional in uc-infra"
    );

    assert_feature_enables(infra, "lan-compat", "dep:network-interface");
    assert_feature_enables(engine, "lan-compat", "dep:uc-mobile-lan");
    assert_feature_enables(engine, "lan-compat", "dep:uc-mobile-proto");
    assert_feature_enables(engine, "lan-compat", "uc-infra/lan-compat");
    assert_default_does_not_enable(mobile_lan, "lan-compat");
}

#[test]
fn core_repository_engine_consumers_do_not_enable_lan_compat() {
    let metadata = workspace_metadata();

    for package_name in ["uc-engine-uniffi", "uc-ohos-napi", "uc-mobile-probe-core"] {
        let dependency = normal_dependency(package(&metadata, package_name), "uc-engine");
        assert!(
            !dependency.features.contains(&"lan-compat".to_string()),
            "{package_name} must not enable uc-engine/lan-compat"
        );
    }
}

#[test]
fn engine_dispatch_does_not_control_membership_activity_steps() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dispatch =
        std::fs::read_to_string(workspace_root.join("crates/uc-engine/src/runtime/dispatch.rs"))
            .expect("engine dispatch source must be readable");

    for forbidden in ["pause_membership_gossip", "resume_membership_gossip"] {
        assert!(
            !dispatch.contains(forbidden),
            "engine dispatch must call one application action instead of {forbidden}"
        );
    }
}

#[test]
fn engine_does_not_own_membership_gossip_or_its_runtime() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let engine_source = read_rs_sources(&workspace_root.join("crates/uc-engine/src"));

    for forbidden in [
        "MembershipConvergenceRuntime",
        "MembershipConvergenceActivity",
    ] {
        assert!(
            !engine_source.contains(forbidden),
            "application runtime must own {forbidden} instead of uc-engine"
        );
    }
}

#[test]
fn facade_directory_does_not_contain_membership_implementation() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let facade = workspace_root.join("crates/uc-application/src/facade");

    for forbidden in ["membership_gossip.rs", "membership_gossip"] {
        assert!(
            !facade.join(forbidden).exists(),
            "membership implementation must live under uc-application/src/membership, not facade/{forbidden}"
        );
    }
}

#[test]
fn engine_join_space_does_not_select_an_internal_route() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let join = std::fs::read_to_string(
        workspace_root.join("crates/uc-engine/src/operations/space/join_space.rs"),
    )
    .expect("join-space operation source must be readable");

    for forbidden in ["JoinSpaceMode", "query_setup_state", "ensure_receive_ready"] {
        assert!(
            !join.contains(forbidden),
            "application join action must own {forbidden}"
        );
    }
}

#[test]
fn engine_space_operations_do_not_reach_into_space_setup() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let operations = read_rs_sources(&workspace_root.join("crates/uc-engine/src/operations/space"));
    assert!(
        !operations.contains(".space_setup"),
        "space operations must use one AppFacade action"
    );
}

#[test]
fn engine_runtime_does_not_start_space_connectivity_maintenance() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime =
        std::fs::read_to_string(workspace_root.join("crates/uc-engine/src/runtime/mod.rs"))
            .expect("engine runtime source must be readable");
    assert!(
        !runtime.contains("spawn_peer_keepalive_task"),
        "space application runtime must start connectivity maintenance"
    );
}

#[test]
fn legacy_space_setup_types_are_removed() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = format!(
        "{}{}",
        read_rs_sources(&workspace_root.join("crates/uc-application/src")),
        read_rs_sources(&workspace_root.join("crates/uc-engine/src")),
    );
    for forbidden in [
        ["Space", "Setup", "Facade"].concat(),
        ["Space", "Setup", "Deps"].concat(),
    ] {
        assert!(
            !source.contains(&forbidden),
            "legacy type {forbidden} must be removed"
        );
    }
}

#[test]
fn encryption_facade_does_not_duplicate_space_lifecycle_actions() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let encryption = std::fs::read_to_string(
        workspace_root.join("crates/uc-application/src/space/lifecycle/encryption/mod.rs"),
    )
    .expect("encryption facade source must be readable");
    for forbidden in [
        "pub async fn initialize(",
        "pub async fn unlock(",
        "pub async fn lock(",
    ] {
        assert!(
            !encryption.contains(forbidden),
            "space lifecycle action must not remain on EncryptionFacade: {forbidden}"
        );
    }
}

#[test]
fn factory_reset_is_one_application_action() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let reset = std::fs::read_to_string(
        workspace_root.join("crates/uc-engine/src/operations/space/factory_reset.rs"),
    )
    .expect("factory-reset operation source must be readable");
    assert!(
        !reset.contains("EnsureReceiveReadyPort") && !reset.contains("close_receive_gate"),
        "application session owner must quiet activities for factory reset"
    );
}

#[test]
fn create_space_is_one_application_action() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let create = std::fs::read_to_string(
        workspace_root.join("crates/uc-engine/src/operations/space/create_space.rs"),
    )
    .expect("create-space operation source must be readable");
    assert!(
        !create.contains("EnsureReceiveReadyPort") && !create.contains("ensure_receive_ready"),
        "application session owner must activate all activities after create"
    );
}

#[test]
fn app_facade_does_not_expose_internal_objects_or_half_ready_slots() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = std::fs::read_to_string(
        workspace_root.join("crates/uc-application/src/facade/app_facade.rs"),
    )
    .expect("AppFacade source must be readable");
    let start = source
        .find("pub struct AppFacade {")
        .expect("AppFacade declaration must exist");
    let end = source[start..]
        .find("\n}\n\nimpl AppFacade")
        .map(|offset| start + offset)
        .expect("AppFacade declaration must have an impl");
    let fields = &source[start..end];

    for forbidden in ["    pub ", "OnceLock<", "Option<Arc<"] {
        assert!(
            !fields.contains(forbidden),
            "AppFacade must be complete and keep internal objects private: {forbidden}"
        );
    }
}

#[test]
fn app_facade_runtime_assembly_requires_every_production_capability() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assembly =
        std::fs::read_to_string(workspace_root.join("crates/uc-engine/src/assembly/facade.rs"))
            .expect("AppFacade assembly source must be readable");
    let runtime =
        std::fs::read_to_string(workspace_root.join("crates/uc-engine/src/runtime/mod.rs"))
            .expect("Engine runtime source must be readable");

    assert!(
        assembly.contains("struct RuntimeAppFacadeAssembly"),
        "the unique runtime assembly must have an explicit complete input"
    );
    for forbidden in ["AppFacadeAssemblyOptions", "SearchFacadeAssemblyMode"] {
        assert!(
            !assembly.contains(forbidden),
            "runtime assembly must not retain optional mode {forbidden}"
        );
    }
    assert!(
        !runtime.contains("..Default::default()"),
        "production AppFacade construction must not fill missing capabilities from defaults"
    );
}

#[test]
fn app_facade_is_the_only_application_path_used_by_engine_operations() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let operations = read_rs_sources(&workspace_root.join("crates/uc-engine/src/operations"));
    let compact = operations.split_whitespace().collect::<String>();

    for forbidden in [
        "facade.clipboard_restore.",
        "facade.config_migration.",
        "facade.settings.",
        "facade.encryption.",
        "facade.search.",
        "facade.device.",
        "facade.member_roster.",
        "facade.blob_transfer.",
    ] {
        assert!(
            !compact.contains(forbidden),
            "Engine operation must call one AppFacade action instead of {forbidden}"
        );
    }
}

#[test]
fn obsolete_application_shells_are_removed() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lifecycle = workspace_root.join("crates/uc-application/src/facade/lifecycle/mod.rs");
    let application = read_rs_sources(&workspace_root.join("crates/uc-application/src"));

    assert!(
        !lifecycle.exists(),
        "the unused generic lifecycle shell and its tests must be deleted"
    );
    for forbidden in ["pub fn read_only(", "pub async fn rebuild_search_now("] {
        assert!(
            !application.contains(forbidden),
            "obsolete application entry must be deleted: {forbidden}"
        );
    }
}

#[test]
fn engine_does_not_reassemble_clipboard_inbound_or_transfer_sessions() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let engine = read_rs_sources(&workspace_root.join("crates/uc-engine/src"));

    for forbidden in [
        "subscribe_inbound_clipboard_notices",
        "InboundNoticeSubscription",
        "spawn_ingest_loop",
        "FileTransferSession",
        "BeginReceiverTransfer",
        ".report_progress(",
    ] {
        assert!(
            !engine.contains(forbidden),
            "Engine must not reassemble application workflow step {forbidden}"
        );
    }
}

#[test]
fn engine_does_not_own_mobile_upload_state() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let upload = std::fs::read_to_string(
        workspace_root.join("crates/uc-engine/src/runtime/mobile_upload.rs"),
    )
    .expect("mobile upload operation source must be readable");

    for forbidden in [
        "HashMap<",
        "ActiveMobileUpload",
        "staging_handle",
        "bytes_written",
        "last_progress",
    ] {
        assert!(
            !upload.contains(forbidden),
            "application upload coordinator must own {forbidden}"
        );
    }
}

#[test]
fn engine_does_not_restore_unrelated_search_or_membership_activity_steps() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let runtime = read_rs_sources(&workspace_root.join("crates/uc-engine/src/runtime"));

    for forbidden in [
        "on_session_ready",
        "pause_background_activity",
        "resume_membership_gossip",
        "pause_membership_gossip",
        "resume_legacy_bootstraps",
    ] {
        assert!(
            !runtime.contains(forbidden),
            "space application runtime must own activity step {forbidden}"
        );
    }
}

#[test]
fn engine_only_imports_application_facade_and_deps() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let engine = read_rs_sources(&workspace_root.join("crates/uc-engine/src"));
    let mobile_lan = read_rs_sources(&workspace_root.join("compatibility/uc-mobile-lan/src"));

    for source in [&engine, &mobile_lan] {
        for line in source.lines() {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("use uc_application::") {
                continue;
            }
            assert!(
                trimmed.starts_with("use uc_application::facade::")
                    || trimmed.starts_with("use uc_application::deps::"),
                "external crate must only import uc_application::facade or ::deps: {line}"
            );
        }
    }
}

#[test]
fn application_root_only_exposes_facade_and_deps() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let lib = std::fs::read_to_string(workspace_root.join("crates/uc-application/src/lib.rs"))
        .expect("application lib.rs must be readable");

    for line in lib.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub mod ") || trimmed.starts_with("pub use ") {
            assert!(
                trimmed.starts_with("pub mod deps;")
                    || trimmed.starts_with("pub mod facade;")
                    || trimmed.starts_with("pub use deps::"),
                "crate root must only expose facade and deps: {line}"
            );
        }
    }
}

#[test]
fn no_central_or_nested_usecases_directories_remain() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let application = workspace_root.join("crates/uc-application/src");
    for forbidden in ["usecases", "space/roster/usecases", "runtime", "membership"] {
        assert!(
            !application.join(forbidden).exists(),
            "forbidden legacy directory must be removed: {forbidden}"
        );
    }
}

fn read_rs_sources(root: &Path) -> String {
    let mut pending = vec![root.to_path_buf()];
    let mut source = String::new();
    while let Some(path) = pending.pop() {
        let entries = std::fs::read_dir(path).expect("engine source directory must be readable");
        for entry in entries {
            let path = entry.expect("engine source entry must be readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                source.push_str(
                    &std::fs::read_to_string(path).expect("engine source file must be readable"),
                );
            }
        }
    }
    source
}

fn package<'a>(
    metadata: &'a cargo_metadata::Metadata,
    package_name: &str,
) -> &'a cargo_metadata::Package {
    metadata
        .packages
        .iter()
        .find(|package| package.name == package_name)
        .unwrap_or_else(|| panic!("{package_name} must be a workspace package"))
}

fn normal_dependency<'a>(
    package: &'a cargo_metadata::Package,
    dependency_name: &str,
) -> &'a cargo_metadata::Dependency {
    package
        .dependencies
        .iter()
        .find(|dependency| {
            dependency.name == dependency_name && dependency.kind == DependencyKind::Normal
        })
        .unwrap_or_else(|| {
            panic!(
                "{} must declare a normal dependency on {dependency_name}",
                package.name
            )
        })
}

fn assert_default_does_not_enable(package: &cargo_metadata::Package, feature: &str) {
    let default_features = package.features.get("default");
    assert!(
        default_features.is_none_or(|features| !features.iter().any(|item| item == feature)),
        "{} default feature must not enable {feature}",
        package.name
    );
}

fn assert_feature_enables(package: &cargo_metadata::Package, feature: &str, expected: &str) {
    let enabled = package
        .features
        .get(feature)
        .unwrap_or_else(|| panic!("{} must declare feature {feature}", package.name));
    assert!(
        enabled.iter().any(|item| item == expected),
        "{}/{} must enable {expected}",
        package.name,
        feature
    );
}

fn workspace_metadata() -> cargo_metadata::Metadata {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    MetadataCommand::new()
        .manifest_path(workspace_root.join("Cargo.toml"))
        .exec()
        .expect("workspace metadata must be readable")
}
