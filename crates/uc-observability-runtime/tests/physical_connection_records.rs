use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_runtime::*;

#[test]
fn physical_connection_and_path_aliases_are_local_and_end_with_the_connection() {
    let directory = tempfile::tempdir().expect("logs");
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
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("install")
    .handle();
    handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("capture");
    let recorder = NetworkRecorder::current();
    let first = recorder.connection_established(
        [5; 32],
        42,
        ConnectionDirection::Inbound,
        NetworkPathKind::Direct,
    );
    first.path(
        [1; 32],
        PathObservationKind::Snapshot,
        NetworkPathKind::Direct,
    );
    first.path(
        [2; 32],
        PathObservationKind::Selected,
        NetworkPathKind::Relay,
    );
    first.lagged(3);
    first.closed(ConnectionCloseReason::RemoteReset);
    recorder
        .connection_established(
            [5; 32],
            42,
            ConnectionDirection::Inbound,
            NetworkPathKind::Relay,
        )
        .closed(ConnectionCloseReason::LocalClosed);
    assert_eq!(
        handle.force_flush(Duration::from_secs(2)).logs,
        SignalResult::Completed
    );
    let rows: Vec<serde_json::Value> = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .flat_map(|file| {
            std::fs::read_to_string(file)
                .expect("content")
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON"))
                .filter(|row: &serde_json::Value| row["target"] != "uc.diagnostics")
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(rows.len(), 7);
    assert!(rows[0]["connection_id"].as_str().is_some());
    assert!(rows[..5]
        .iter()
        .all(|r| r["connection_id"] == rows[0]["connection_id"]));
    assert_ne!(
        rows[0]["connection_id"], rows[5]["connection_id"],
        "底层编号复用不能误连已结束的连接"
    );
    assert_eq!(rows[0]["peer_ref"], rows[5]["peer_ref"]);
    assert!(rows[1]["path_ref"].as_str().is_some());
    assert_ne!(rows[1]["path_ref"], rows[2]["path_ref"]);
    assert_eq!(rows[3]["fields"]["missed"], 3);
    assert!(rows
        .iter()
        .all(|r| r.get("connection_key").is_none() && r.get("path_key").is_none()));
    handle.shutdown(Duration::from_secs(2));
}
