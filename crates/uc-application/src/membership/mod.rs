mod convergence;

use async_trait::async_trait;

pub mod errors;
pub mod usecases;

pub use convergence::{
    build_membership_convergence, MembershipConvergence, MembershipConvergenceActivity,
    MembershipConvergenceDeps, MembershipConvergenceError, MembershipConvergenceRuntime,
};

#[async_trait]
pub(crate) trait MembershipConvergenceActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}
