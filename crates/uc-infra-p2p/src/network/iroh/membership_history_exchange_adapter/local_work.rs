use super::super::session_generation::begin_session_local_work;
use async_trait::async_trait;
use tokio::runtime::Handle;
use uc_application::deps::{HistoryLocalWorkPort, HistoryWork};
use uc_core::membership::{MembershipHistoryExchangeError, MembershipHistoryMessage};
use uc_observability_contract::diagnostics::{connectivity::LocalWorkContext, ObservationContext};

pub struct SessionMembershipHistoryLocalWork;
#[async_trait]
impl HistoryLocalWorkPort for SessionMembershipHistoryLocalWork {
    async fn execute(
        &self,
        work: HistoryWork,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let local =
            begin_session_local_work().ok_or(MembershipHistoryExchangeError::PairingInProgress)?;
        let runtime = Handle::current();
        let context = LocalWorkContext::capture();
        let observation = ObservationContext::capture();
        tokio::task::spawn_blocking(move || {
            let _local = local;
            context.run(|| runtime.block_on(observation.scope(work)))
        })
        .await
        .map_err(MembershipHistoryExchangeError::transport_from)?
    }
}
