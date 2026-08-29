use std::sync::Arc;

use async_trait::async_trait;

use crate::facade::search::SearchFacade;
use crate::transfer::receive::reconciliation::EnsureReceiveReadyPort;

#[async_trait]
trait SearchSessionActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}

#[async_trait]
pub trait MembershipSessionActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}

#[async_trait]
pub trait SpaceSessionActivityPort: Send + Sync {
    async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError>;
    async fn pause_for_lock(&self) -> Result<(), SpaceActivityError>;
    async fn restore_after_failed_lock(&self) -> Result<(), String>;
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceActivityError {
    #[error("space session activity is unavailable")]
    Unavailable,
    #[error("search session activation failed: {0}")]
    Search(String),
    #[error("receive activation failed: {0}")]
    Receive(String),
    #[error("membership activation failed: {0}")]
    Membership(String),
}

pub struct SpaceSessionActivity {
    receive: Arc<dyn EnsureReceiveReadyPort>,
    search: Arc<dyn SearchSessionActivityPort>,
}

pub struct SpaceSessionActivityDeps {
    pub receive: Arc<dyn EnsureReceiveReadyPort>,
}

pub fn build_space_session_activity(
    search: Arc<SearchFacade>,
    deps: SpaceSessionActivityDeps,
) -> Arc<SpaceSessionActivity> {
    Arc::new(SpaceSessionActivity::new(deps.receive, search))
}

impl SpaceSessionActivity {
    fn new(
        receive: Arc<dyn EnsureReceiveReadyPort>,
        search: Arc<dyn SearchSessionActivityPort>,
    ) -> Self {
        Self { receive, search }
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
        Ok(())
    }

    pub(crate) async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        self.receive.close_receive_gate();
        self.search
            .pause()
            .await
            .map_err(SpaceActivityError::Search)
    }

    pub(crate) async fn restore_after_failed_lock(&self) -> Result<(), String> {
        let search = self.search.resume().await;
        let receive = self.receive.ensure_receive_ready().await;
        match (search, receive) {
            (Ok(()), Ok(())) => Ok(()),
            (search, receive) => Err(format!(
                "search={}, receive={}",
                search.err().unwrap_or_else(|| "restored".to_string()),
                receive
                    .err()
                    .map(|error| error.to_string())
                    .unwrap_or_else(|| "restored".to_string()),
            )),
        }
    }
}

pub(crate) fn combine_space_session_activity(
    membership: Arc<dyn MembershipSessionActivityPort>,
    other: Arc<dyn SpaceSessionActivityPort>,
) -> Arc<dyn SpaceSessionActivityPort> {
    Arc::new(CombinedSpaceSessionActivity { membership, other })
}

struct CombinedSpaceSessionActivity {
    membership: Arc<dyn MembershipSessionActivityPort>,
    other: Arc<dyn SpaceSessionActivityPort>,
}

#[async_trait]
impl SpaceSessionActivityPort for CombinedSpaceSessionActivity {
    async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
        self.other.resume_after_session_ready().await?;
        self.membership
            .resume()
            .await
            .map_err(SpaceActivityError::Membership)
    }

    async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        self.membership
            .pause()
            .await
            .map_err(SpaceActivityError::Membership)?;
        if let Err(error) = self.other.pause_for_lock().await {
            let _ = self.other.restore_after_failed_lock().await;
            let _ = self.membership.resume().await;
            return Err(error);
        }
        Ok(())
    }

    async fn restore_after_failed_lock(&self) -> Result<(), String> {
        let other = self.other.restore_after_failed_lock().await;
        let membership = self.membership.resume().await;
        match (other, membership) {
            (Ok(()), Ok(())) => Ok(()),
            (other, membership) => Err(format!(
                "application={}, membership={}",
                other.err().unwrap_or_else(|| "restored".to_owned()),
                membership.err().unwrap_or_else(|| "restored".to_owned()),
            )),
        }
    }
}

#[async_trait]
impl SpaceSessionActivityPort for SpaceSessionActivity {
    async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
        SpaceSessionActivity::resume_after_session_ready(self).await
    }

    async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        SpaceSessionActivity::pause_for_lock(self).await
    }

    async fn restore_after_failed_lock(&self) -> Result<(), String> {
        SpaceSessionActivity::restore_after_failed_lock(self).await
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct RecordingMembership {
        pauses: AtomicUsize,
        resumes: AtomicUsize,
    }

    #[async_trait]
    impl MembershipSessionActivityPort for RecordingMembership {
        async fn pause(&self) -> Result<(), String> {
            self.pauses.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn resume(&self) -> Result<(), String> {
            self.resumes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingApplicationActivity(AtomicUsize);

    #[async_trait]
    impl SpaceSessionActivityPort for FailingApplicationActivity {
        async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
            Ok(())
        }

        async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
            Err(SpaceActivityError::Search("pause failed".to_owned()))
        }

        async fn restore_after_failed_lock(&self) -> Result<(), String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_application_pause_restores_membership_before_returning() {
        let membership = Arc::new(RecordingMembership {
            pauses: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
        });
        let application = Arc::new(FailingApplicationActivity(AtomicUsize::new(0)));
        let activity = combine_space_session_activity(membership.clone(), application.clone());

        assert!(activity.pause_for_lock().await.is_err());

        assert_eq!(membership.pauses.load(Ordering::SeqCst), 1);
        assert_eq!(membership.resumes.load(Ordering::SeqCst), 1);
        assert_eq!(application.0.load(Ordering::SeqCst), 1);
    }
}
