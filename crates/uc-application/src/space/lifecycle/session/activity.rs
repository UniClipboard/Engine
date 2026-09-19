use std::fmt;
use std::sync::Arc;
use std::sync::OnceLock;

use async_trait::async_trait;

use crate::facade::search::SearchFacade;
use crate::transfer::receive::reconciliation::EnsureReceiveReadyPort;

#[async_trait]
trait SearchSessionActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), anyhow::Error>;
    async fn resume(&self) -> Result<(), anyhow::Error>;
}

#[async_trait]
pub trait MembershipSessionActivityPort: Send + Sync {
    async fn pause(&self) -> anyhow::Result<()>;
    async fn resume(&self) -> anyhow::Result<()>;
    fn wake(&self) -> anyhow::Result<()>;
}

#[async_trait]
pub trait SpaceSessionActivityPort: Send + Sync {
    async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError>;
    async fn pause_for_lock(&self) -> Result<(), SpaceActivityError>;
    async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error>;
}

pub(crate) struct DeferredSpaceSessionActivity {
    delegate: OnceLock<Arc<dyn SpaceSessionActivityPort>>,
}

impl DeferredSpaceSessionActivity {
    pub(crate) fn new() -> Self {
        Self {
            delegate: OnceLock::new(),
        }
    }

    pub(crate) fn bind(&self, delegate: Arc<dyn SpaceSessionActivityPort>) -> bool {
        self.delegate.set(delegate).is_ok()
    }

    fn delegate(&self) -> Result<Arc<dyn SpaceSessionActivityPort>, SpaceActivityError> {
        self.delegate
            .get()
            .cloned()
            .ok_or(SpaceActivityError::Unavailable)
    }
}

#[async_trait]
impl SpaceSessionActivityPort for DeferredSpaceSessionActivity {
    async fn resume_after_session_ready(&self) -> Result<(), SpaceActivityError> {
        self.delegate()?.resume_after_session_ready().await
    }

    async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        self.delegate()?.pause_for_lock().await
    }

    async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
        let delegate = self.delegate()?;
        delegate.restore_after_failed_lock().await
    }
}

#[derive(thiserror::Error)]
pub enum SpaceActivityError {
    #[error("peer connection activity failed")]
    Connectivity(#[from] crate::space::PeerConnectionError),
    #[error("space session activity is unavailable")]
    Unavailable,
    #[error("search session activity failed")]
    Search(#[source] anyhow::Error),
    #[error("receive activation failed: {0}")]
    Receive(String),
    #[error("membership activation failed")]
    Membership(#[source] anyhow::Error),
    #[error("space session activity task failed")]
    Task(#[source] tokio::task::JoinError),
}

impl fmt::Debug for SpaceActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Connectivity(_) => "SpaceActivityError::Connectivity",
            Self::Unavailable => "SpaceActivityError::Unavailable",
            Self::Search(_) => "SpaceActivityError::Search",
            Self::Receive(_) => "SpaceActivityError::Receive",
            Self::Membership(_) => "SpaceActivityError::Membership",
            Self::Task(_) => "SpaceActivityError::Task",
        })
    }
}

pub struct SpaceSessionActivity {
    connections: Arc<crate::space::connectivity::PeerConnectionCoordinator>,
    receive: Arc<dyn EnsureReceiveReadyPort>,
    search: Arc<dyn SearchSessionActivityPort>,
}

pub(crate) fn build_space_session_activity(
    search: Arc<SearchFacade>,
    receive: Arc<dyn EnsureReceiveReadyPort>,
    connections: Arc<crate::space::connectivity::PeerConnectionCoordinator>,
) -> Arc<SpaceSessionActivity> {
    Arc::new(SpaceSessionActivity::new(receive, search, connections))
}

impl SpaceSessionActivity {
    fn new(
        receive: Arc<dyn EnsureReceiveReadyPort>,
        search: Arc<dyn SearchSessionActivityPort>,
        connections: Arc<crate::space::connectivity::PeerConnectionCoordinator>,
    ) -> Self {
        Self {
            receive,
            search,
            connections,
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
        self.connections.resume().await?;
        Ok(())
    }

    pub(crate) async fn pause_for_lock(&self) -> Result<(), SpaceActivityError> {
        self.connections.pause().await?;
        self.receive.close_receive_gate();
        self.search
            .pause()
            .await
            .map_err(SpaceActivityError::Search)
    }

    pub(crate) async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
        let search = self.search.resume().await;
        let receive = self.receive.ensure_receive_ready().await;
        let connections = self.connections.resume().await;
        restore_results(vec![
            search,
            receive.map_err(anyhow::Error::new),
            connections.map_err(anyhow::Error::new),
        ])
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
            .map_err(SpaceActivityError::Membership)?;
        self.membership
            .wake()
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

    async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
        let other = self.other.restore_after_failed_lock().await;
        let membership = self.membership.resume().await;
        restore_results(vec![other, membership])
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

    async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
        SpaceSessionActivity::restore_after_failed_lock(self).await
    }
}

#[async_trait]
impl SearchSessionActivityPort for SearchFacade {
    async fn pause(&self) -> Result<(), anyhow::Error> {
        self.pause_background_activity().await?;
        Ok(())
    }

    async fn resume(&self) -> Result<(), anyhow::Error> {
        self.on_session_ready().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn search_failure_preserves_source_and_redacts_details() {
        let error = SpaceActivityError::Search(
            std::io::Error::other("private-search-storage-detail").into(),
        );
        assert!(error.source().unwrap().is::<std::io::Error>());
        assert!(!error.to_string().contains("private-search-storage-detail"));
    }

    #[tokio::test]
    async fn membership_task_failure_keeps_its_source_out_of_default_summaries() {
        let source = tokio::spawn(async { panic!("PRIVATE_MEMBERSHIP_DETAIL") })
            .await
            .unwrap_err();
        let error = SpaceActivityError::Membership(source.into());
        assert!(error.source().unwrap().is::<tokio::task::JoinError>());
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
    }

    struct RecordingMembership {
        pauses: AtomicUsize,
        wakes: AtomicUsize,
        resumes: AtomicUsize,
    }

    #[async_trait]
    impl MembershipSessionActivityPort for RecordingMembership {
        async fn pause(&self) -> anyhow::Result<()> {
            self.pauses.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn resume(&self) -> anyhow::Result<()> {
            self.resumes.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn wake(&self) -> anyhow::Result<()> {
            self.wakes.fetch_add(1, Ordering::SeqCst);
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
            Err(SpaceActivityError::Search(anyhow::anyhow!("pause failed")))
        }

        async fn restore_after_failed_lock(&self) -> Result<(), anyhow::Error> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn failed_application_pause_restores_membership_before_returning() {
        let membership = Arc::new(RecordingMembership {
            pauses: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
        });
        let application = Arc::new(FailingApplicationActivity(AtomicUsize::new(0)));
        let activity = combine_space_session_activity(membership.clone(), application.clone());

        assert!(activity.pause_for_lock().await.is_err());

        assert_eq!(membership.pauses.load(Ordering::SeqCst), 1);
        assert_eq!(membership.resumes.load(Ordering::SeqCst), 1);
        assert_eq!(application.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn remote_membership_recovery_is_woken_after_local_activity_resumes() {
        let membership = Arc::new(RecordingMembership {
            pauses: AtomicUsize::new(0),
            wakes: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
        });
        let application = Arc::new(FailingApplicationActivity(AtomicUsize::new(0)));
        let activity = combine_space_session_activity(membership.clone(), application);

        activity.resume_after_session_ready().await.unwrap();

        assert_eq!(membership.wakes.load(Ordering::SeqCst), 1);
        assert_eq!(membership.resumes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn deferred_activity_binds_once_and_delegates_inside_application() {
        let activity = DeferredSpaceSessionActivity::new();
        assert!(matches!(
            activity.resume_after_session_ready().await,
            Err(SpaceActivityError::Unavailable)
        ));

        let delegate = Arc::new(FailingApplicationActivity(AtomicUsize::new(0)));
        assert!(activity.bind(delegate.clone()));
        assert!(!activity.bind(delegate));
        assert!(matches!(
            activity.pause_for_lock().await,
            Err(SpaceActivityError::Search(error)) if error.to_string() == "pause failed"
        ));
        activity
            .restore_after_failed_lock()
            .await
            .expect("bound activity should delegate failed-lock restoration");
    }
}

// 回滚尝试所有活动并保留全部失败，显示文本只包含固定分类。
#[derive(Debug)]
struct ActivityRestoreFailures(Vec<anyhow::Error>);
impl std::fmt::Display for ActivityRestoreFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("space activity restoration failed")
    }
}
impl std::error::Error for ActivityRestoreFailures {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.first().map(|error| error.as_ref())
    }
}
fn restore_results(results: Vec<Result<(), anyhow::Error>>) -> Result<(), anyhow::Error> {
    let failures: Vec<_> = results.into_iter().filter_map(Result::err).collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow::Error::new(ActivityRestoreFailures(failures)))
    }
}
