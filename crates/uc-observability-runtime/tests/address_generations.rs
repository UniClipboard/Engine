use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_runtime::*;

#[test]
fn address_values_and_source_generations_remain_distinct_in_exported_files() {
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
    let summary = CandidateSummary {
        direct_count: 1,
        relay_count: 0,
        other_count: 0,
    };
    recorder.address_loaded([1; 32], Some([9; 32]), summary, 1000);
    recorder.address_used([1; 32], Some([9; 32]), AddressInputSource::Stored, summary);
    recorder.address_loaded([1; 32], Some([10; 32]), summary, 2000);
    recorder.address_used([1; 32], Some([9; 32]), AddressInputSource::Stored, summary);
    recorder.address_loaded([1; 32], Some([9; 32]), summary, 3000);
    recorder.address_used([2; 32], Some([9; 32]), AddressInputSource::Stored, summary);
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
    assert_eq!(rows.len(), 6);
    assert!(
        rows[0]["candidate_set_ref"].as_str().is_some(),
        "地址值必须以随机编号关联"
    );
    assert_eq!(rows[0]["candidate_set_ref"], rows[1]["candidate_set_ref"]);
    assert_ne!(rows[0]["candidate_set_ref"], rows[2]["candidate_set_ref"]);
    assert_eq!(rows[0]["candidate_set_ref"], rows[4]["candidate_set_ref"]);
    assert_eq!(rows[0]["candidate_generation"], 1);
    assert_eq!(rows[2]["candidate_generation"], 2);
    assert_eq!(
        rows[4]["candidate_generation"], 3,
        "旧地址值再次出现仍然是新的来源代次"
    );
    assert_eq!(
        rows[3]["candidate_generation"], 1,
        "使用旧快照不能倒改存储来源的代次"
    );
    assert_eq!(rows[1]["candidate_values_changed"], serde_json::Value::Null);
    assert_eq!(rows[3]["candidate_values_changed"], false);
    assert_ne!(rows[3]["candidate_set_ref"], rows[5]["candidate_set_ref"]);
    assert_ne!(rows[3]["peer_ref"], rows[5]["peer_ref"]);
    handle.shutdown(Duration::from_secs(2));
}
