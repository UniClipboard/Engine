//! UniClipboard Engine 测试场景的通用控制与诊断基础。

mod budget;
mod event;
mod failure;
mod identity;
mod process;
mod report;
mod resource;
mod scenario;

pub use budget::{ScenarioBudget, StageGuard, StageTiming};
pub use event::{EventRecorder, ScenarioEvent};
pub use failure::{FailureKind, FailureReport, ScenarioFailure};
pub use identity::ScenarioIdentity;
pub use report::{CleanupStatus, ResourceReport, ScenarioReport};
pub use resource::{TcpPortLease, TempDirLease};
pub use scenario::{Scenario, ScenarioCompletion, ScenarioCompletionFailure, ScenarioConfig};
