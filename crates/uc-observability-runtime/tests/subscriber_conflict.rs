use uc_observability_runtime::{
    DeploymentEnvironment, InstallError, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime,
};

#[test]
fn an_existing_process_subscriber_is_reported_explicitly() {
    tracing_subscriber::fmt()
        .try_init()
        .expect("fixture subscriber install");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.2.3",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("approved resource"),
    );

    assert!(matches!(
        ProcessObservabilityRuntime::install(config),
        Err(InstallError::SubscriberAlreadyInstalled)
    ));
}
