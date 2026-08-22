use std::sync::Arc;

use async_trait::async_trait;

use crate::facade::search::SearchFacade;
use crate::space::workspace_membership::discovery::MembershipConvergenceActivityPort;
use crate::transfer::receive::reconciliation::EnsureReceiveReadyPort;

#[async_trait]
trait SearchSessionActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceActivityError {
    #[error("search session activation failed: {0}")]
    Search(String),
    #[error("receive activation failed: {0}")]
    Receive(String),
    #[error("membership activation failed: {0}")]
    Membership(String),
}

pub struct SpaceSessionActivity {
    membership: Arc<dyn MembershipConvergenceActivityPort>,
    receive: Arc<dyn EnsureReceiveReadyPort>,
    search: Arc<dyn SearchSessionActivityPort>,
}

pub struct SpaceSessionActivityDeps {
    pub membership: crate::space::runtime::SpaceMembershipActivity,
    pub receive: Arc<dyn EnsureReceiveReadyPort>,
}

pub fn build_space_session_activity(
    search: Arc<SearchFacade>,
    deps: SpaceSessionActivityDeps,
) -> Arc<SpaceSessionActivity> {
    Arc::new(SpaceSessionActivity::new(
        Arc::new(deps.membership),
        deps.receive,
        search,
    ))
}

impl SpaceSessionActivity {
    pub(crate) fn new(
        membership: Arc<dyn MembershipConvergenceActivityPort>,
        receive: Arc<dyn EnsureReceiveReadyPort>,
        search: Arc<dyn SearchSessionActivityPort>,
    ) -> Self {
        Self {
            membership,
            receive,
            search,
        }
    }

    pub(crate) async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
        self.search
            .resume()
            .await
            .map_err(SpaceActivityError::Search)?;
        self.receive
            .ensure_receive_ready()
            .await
            .map_err(|error| SpaceActivityError::Receive(error.to_string()))?;
        self.membership
            .resume()
            .await
            .map_err(SpaceActivityError::Membership)
    }

    pub(crate) async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        self.membership
            .pause()
            .await
            .map_err(SpaceActivityError::Membership)?;
        self.receive.close_receive_gate();
        self.search
            .pause()
            .await
            .map_err(SpaceActivityError::Search)
    }

    pub(crate) async fn restore_after_failed_lock(&self) -> Result<(), String> {
        let search = self.search.resume().await;
        let receive = self.receive.ensure_receive_ready().await;
        let membership = self.membership.resume().await;
        match (search, receive, membership) {
            (Ok(()), Ok(()), Ok(())) => Ok(()),
            (search, receive, membership) => Err(format!(
                "search={}, receive={}, membership={}",
                search.err().unwrap_or_else(|| "restored".to_string()),
                receive
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "restored".to_string()),
                membership.err().unwrap_or_else(|| "restored".to_string()),
            )),
        }
    }
}

#[async_trait]
impl SearchSessionActivityPort for SearchFacade {
    async fn pause(&self) -> Result<(), String> {
        self.pause_background_activity().await;
        Ok(())
    }

    async fn resume(&self) -> Result<(), String> {
        self.on_session_ready().await;
        Ok(())
    }
}
