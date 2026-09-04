use std::process::ExitCode;
use std::time::Duration;

use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
    DiagnosticSpanKind, OperationCompletion, OperationContext,
};
use uc_observability_runtime::{
    DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    OtlpHttpConfig, ProcessObservabilityRuntime, SignalResult,
};

fn main() -> ExitCode {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let Some(trace_endpoint) = arguments.next() else {
        return ExitCode::from(2);
    };
    let Some(log_endpoint) = arguments.next() else {
        return ExitCode::from(2);
    };
    let Ok(remote) = OtlpHttpConfig::new(&trace_endpoint, &log_endpoint) else {
        return ExitCode::from(2);
    };
    let config = ObservabilityConfig::new(ObservabilityResource::new(
        env!("CARGO_PKG_VERSION"),
        DeploymentEnvironment::Development,
        current_os(),
        std::env::consts::ARCH,
        "local-probe",
    ))
    .with_remote(remote);
    let Ok(installed) = ProcessObservabilityRuntime::install(config) else {
        return ExitCode::FAILURE;
    };
    let handle = installed.handle();

    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
        flow: None,
    });
    {
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(8),
        ));
    }
    drop(span);

    let flushed = handle.force_flush(Duration::from_secs(10));
    let _ = handle.shutdown(Duration::from_secs(10));
    if flushed.traces == SignalResult::Completed && flushed.logs == SignalResult::Completed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn current_os() -> OperatingSystem {
    if cfg!(target_os = "ios") {
        OperatingSystem::Ios
    } else if cfg!(target_os = "android") {
        OperatingSystem::Android
    } else if cfg!(target_os = "macos") {
        OperatingSystem::Macos
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else if cfg!(target_os = "linux") {
        OperatingSystem::Linux
    } else {
        OperatingSystem::Other
    }
}
