//! 升级检测与确认——`settings` 领域的完整流程。
//!
//! 1. `detect::DetectUpgradeUseCase` —— 启动期一次性比较
//!    `AppVersionStatePort` 游标 vs 当前构建版本，输出结构化
//!    `status::UpgradeStatus`。
//! 2. `acknowledge::AcknowledgeUseCase` —— 调用方（UI / CLI）确认用户
//!    已知晓后，把游标推进到当前版本，下次启动得到 `NoChange`。
//!
//! `UpgradeFacade` 是对外入口；use case 保持 `pub(crate)`。

pub(crate) mod acknowledge;
pub(crate) mod detect;
pub(crate) mod facade;
pub(crate) mod status;

pub use facade::{
    AcknowledgeUpgradeError, DetectUpgradeError, UpgradeFacade, UpgradeFacadeDeps, UpgradeStatus,
};
