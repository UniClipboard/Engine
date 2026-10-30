//! 已放弃的 Engine 登记不能被进程恢复或后续登记绕过。
#![cfg(feature = "test-process-release")]

use std::time::Duration;

use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime, ProcessReleaseResult, ProcessResumeResult,
    SignalResult,
};

#[test]
fn abandoned_engine_keeps_terminal_fence_and_rejects_registration() {
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    );
    let process = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (activity, _) = EngineLogActivity::begin(Duration::from_secs(1));
    assert!(activity.is_admitted());
    drop(activity);

    let (report, _) = process
        .release_process(Duration::from_secs(5))
        .expect("release");
    assert!(matches!(report.result, ProcessReleaseResult::Failed { .. }));
    assert!(!report.explicit_retry_supported);
    for _ in 0..2 {
        let (resume, _) = process.resume_process(Duration::ZERO).expect("resume");
        assert!(matches!(resume, ProcessResumeResult::Failed { .. }));
        let (registration, result) = EngineLogActivity::begin(Duration::from_secs(1));
        assert!(!registration.is_admitted());
        assert_eq!(result, Some(SignalResult::Failed));
    }
}
