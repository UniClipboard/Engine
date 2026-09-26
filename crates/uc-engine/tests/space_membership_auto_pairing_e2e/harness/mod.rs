//! 成员多设备真实场景的共享测试台。场景模块经入口的 `use harness::*` 使用这里的能力。

use super::*;

mod device;
mod host;
mod membership_topology;
mod ops;
mod rendezvous;
pub(crate) mod scenario;

pub(crate) use device::*;
pub(crate) use host::*;
pub(crate) use membership_topology::*;
pub(crate) use ops::*;
pub(crate) use rendezvous::*;
pub(crate) use scenario::TestScenario;

pub(crate) const PASSPHRASE: &str = "space-membership-e2e-passphrase";

pub(crate) const WAIT_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) const ADMISSION_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) const EXPIRES_AT_MS: i64 = 2_000_000_000_000;
