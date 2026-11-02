//! Stable cross-platform interface owned by the UniClipboard engine.

mod assembly;
mod compatibility;
mod contract;
#[cfg(feature = "dev-tools")]
mod dev;
mod engine;
mod operations;
mod runtime;
mod subsystems;

#[cfg(test)]
mod testing;

pub use compatibility::mobile_lan::*;
pub use contract::*;
#[cfg(feature = "dev-tools")]
pub use dev::*;
pub use engine::{Engine, EventStream};
pub use engine::{StartupLifecycle, StartupLifecycleInput};
pub use engine::{StartupProgress, StartupProgressInput};

/// 仅独立验收构建启用的 Infra 网络诊断控制。
#[cfg(feature = "network-test-probe")]
pub mod testing_network_probe {
    pub use uc_infra_p2p::network::iroh::testing_network_probe::{
        clear, install, set_expected, snapshot, AddressRefreshEvent, AddressRefreshSnapshot,
    };

    pub mod subnets {
        pub use uc_infra_p2p::network::iroh::testing_network_probe::subnets::{
            arm_read, clear, install, mark, release_read, snapshot, SubnetEvent, SubnetMarker,
            SubnetReadFault, SubnetSnapshot,
        };
    }
}

/// 仅独立验收构建启用的内容流故障控制。
#[cfg(feature = "clipboard-stream-fault")]
pub mod testing_stream_fault {
    pub use uc_application::deps::{
        acquisition_snapshot, arm_acquisition_hold, release_acquisition_hold,
        reset_acquisition_faults, AcquisitionControlError, AcquisitionEvent, AcquisitionEventKind,
        AcquisitionHold, AcquisitionSnapshot,
    };
    pub use uc_application::deps::{
        arm_active_convergence_hold, begin_receive_observation, receive_snapshot,
        release_active_convergence_hold, reset_receive_faults, ConvergenceHold,
        ReceiveCommitEvidence, ReceiveEvent, ReceiveHoldError, ReceiveObservationError,
        ReceivePath, ReceiveSnapshot, ReceiveStage,
    };
    pub use uc_application::deps::{
        begin_recovery_observation, recovery_snapshot, RecoveryEvent, RecoveryEventKind,
        RecoveryEventRef, RecoveryObservationError, RecoverySnapshot,
    };
    pub use uc_infra_p2p::network::clipboard::stream_fault::{
        arm, release, release_hold, reset, snapshot, AttemptEvent, AttemptEventKind, FaultAction,
        FaultControlError, FaultPhase, FaultSnapshot, HeldStream,
    };
}
