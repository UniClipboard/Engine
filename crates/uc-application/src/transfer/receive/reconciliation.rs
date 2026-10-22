use super::work::ReceiveWorkRegistry;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tracing::instrument;
use uc_core::ids::EntryId;
use uc_core::ports::{
    directory_staging_area_for_root, AttemptState, BeginReceiveFailureOutcome,
    BeginReceiveFailurePort, CleanupReceiveArtifactsPort, ClockPort, CommitInboundReceivePort,
    FinalizeProvisionalReceivePort, GetDirectoryPublishRecordPort, GetEntryAttemptPort,
    InboundReceiveSettlement, ListNonTerminalAttemptsPort, ListProvisionalReceivesPort,
    ListUnsettledReceiveArtifactsPort, NoEntryReceiveArtifacts, PartialReceiveTerminal,
    ProvisionalReceiveAction, PublishPhase, ReceiveArtifact, ReceiveArtifactOwnership,
    RecordDirectoryPublishPort,
};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_error, uc_info, uc_warn,
};

pub struct ReceiveReadinessCoordinator {
    work: Arc<ReceiveWorkRegistry>,
    degraded_reason: RwLock<Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiveReadinessStatus {
    pub ready: bool,
    pub degraded_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveReadinessError {
    #[error("receive recovery failed")]
    Recovery(#[source] anyhow::Error),
}

#[async_trait]
pub trait EnsureReceiveReadyPort: Send + Sync {
    async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError>;
    fn close_receive_gate(&self);
    fn receive_readiness_status(&self) -> ReceiveReadinessStatus;
}

impl ReceiveReadinessCoordinator {
    pub fn new() -> Self {
        let work = Arc::new(ReceiveWorkRegistry::default());
        work.close();
        Self {
            work,
            degraded_reason: RwLock::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_ready(&self) {
        let generation = self.work.generation();
        self.work.open(&generation);
        self.publish_ready();
    }

    fn publish_ready(&self) {
        *self
            .degraded_reason
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.work.changed.notify_waiters();
    }

    pub fn mark_not_ready(&self) {
        self.work.close();
    }

    pub fn is_ready(&self) -> bool {
        self.work.is_open()
    }

    pub(crate) fn work_registry(&self) -> Arc<ReceiveWorkRegistry> {
        self.work.clone()
    }

    pub async fn wait_ready(&self) {
        while !self.is_ready() {
            let notified = self.work.changed.notified();
            if self.is_ready() {
                return;
            }
            notified.await;
        }
    }

    pub fn degraded_reason(&self) -> Option<String> {
        self.degraded_reason
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn status(&self) -> ReceiveReadinessStatus {
        ReceiveReadinessStatus {
            ready: self.is_ready(),
            degraded_reason: self.degraded_reason(),
        }
    }

    pub async fn ensure_ready<F, Fut>(&self, recover: F) -> Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        if self.is_ready() {
            return Ok(());
        }

        let _guard = self.work.recovery_lease().await;
        if self.is_ready() {
            return Ok(());
        }

        let generation = self.work.generation();
        match recover().await {
            Ok(()) => {
                if !self.work.open(&generation) {
                    return Err(anyhow!("receive recovery was superseded by gate closure"));
                }
                self.publish_ready();
                Ok(())
            }
            Err(error) => {
                uc_warn!(
                    error_kind = "receive_recovery",
                    io_error_kind = io_error_kind(error.as_ref()),
                    "receive readiness recovery failed; receive gate stays closed"
                );
                if self.work.close_if_current(&generation) {
                    *self
                        .degraded_reason
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.to_string());
                }
                Err(error)
            }
        }
    }
}

impl Default for ReceiveReadinessCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ReconcileReceiveAttemptsUseCase {
    get_attempt: Arc<dyn GetEntryAttemptPort>,
    list_attempts: Arc<dyn ListNonTerminalAttemptsPort>,
    list_artifacts: Arc<dyn ListUnsettledReceiveArtifactsPort>,
    get_directory_publish: Arc<dyn GetDirectoryPublishRecordPort>,
    record_directory_publish: Arc<dyn RecordDirectoryPublishPort>,
    begin_failure: Arc<dyn BeginReceiveFailurePort>,
    cleanup_artifacts: Arc<dyn CleanupReceiveArtifactsPort>,
    list_provisional: Arc<dyn ListProvisionalReceivesPort>,
    finalize_provisional: Arc<dyn FinalizeProvisionalReceivePort>,
    commit: Arc<dyn CommitInboundReceivePort>,
    clock: Arc<dyn ClockPort>,
}

impl ReconcileReceiveAttemptsUseCase {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        get_attempt: Arc<dyn GetEntryAttemptPort>,
        list_attempts: Arc<dyn ListNonTerminalAttemptsPort>,
        list_artifacts: Arc<dyn ListUnsettledReceiveArtifactsPort>,
        get_directory_publish: Arc<dyn GetDirectoryPublishRecordPort>,
        record_directory_publish: Arc<dyn RecordDirectoryPublishPort>,
        begin_failure: Arc<dyn BeginReceiveFailurePort>,
        cleanup_artifacts: Arc<dyn CleanupReceiveArtifactsPort>,
        list_provisional: Arc<dyn ListProvisionalReceivesPort>,
        finalize_provisional: Arc<dyn FinalizeProvisionalReceivePort>,
        commit: Arc<dyn CommitInboundReceivePort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            get_attempt,
            list_attempts,
            list_artifacts,
            get_directory_publish,
            record_directory_publish,
            begin_failure,
            cleanup_artifacts,
            list_provisional,
            finalize_provisional,
            commit,
            clock,
        }
    }

    #[instrument(name = "usecase.reconcile_receive_attempts.execute", skip_all)]
    pub async fn execute(&self) -> Result<u32> {
        let mut reconciled = 0u32;
        let mut provisional_discarded = 0u32;
        let mut failed = 0u32;
        let mut cancelled = 0u32;
        let mut rolled_back = 0u32;
        for provisional in self
            .list_provisional
            .list_provisional_receives()
            .await
            .map_err(anyhow::Error::new)?
        {
            if let Some(uri) = provisional.cached_path {
                let path = url::Url::parse(&uri)
                    .ok()
                    .and_then(|uri| uri.to_file_path().ok())
                    .ok_or_else(|| anyhow!("provisional receive path is not a valid file URI"))?;
                self.cleanup_artifacts
                    .cleanup_receive_artifacts(&[ReceiveArtifact {
                        item_id: provisional.transfer_id.clone(),
                        staged_path: path.clone(),
                        final_path: path,
                        ownership: ReceiveArtifactOwnership::ManagedStaging,
                    }])
                    .await
                    .map_err(anyhow::Error::new)?;
            }
            self.finalize_provisional
                .finalize_provisional_receive(
                    &provisional.transfer_id,
                    ProvisionalReceiveAction::DiscardAsFullyHeld,
                    self.clock.now_ms(),
                )
                .await
                .map_err(anyhow::Error::new)?;
            reconciled = reconciled.saturating_add(1);
            provisional_discarded = provisional_discarded.saturating_add(1);
        }

        let mut artifacts = self
            .list_artifacts
            .list_unsettled_receive_artifacts()
            .await
            .map_err(anyhow::Error::new)?
            .into_iter()
            .map(|record| ((record.entry_id.clone(), record.attempt_id.clone()), record))
            .collect::<HashMap<_, _>>();

        for (entry_id, attempt_id) in artifacts.keys() {
            let current = self
                .get_attempt
                .get_entry_attempt(entry_id)
                .await
                .map_err(anyhow::Error::new)?
                .ok_or_else(|| anyhow!("unsettled receive artifacts have no attempt authority"))?;
            if current.current_attempt_id != *attempt_id || current.state.is_terminal() {
                uc_error!(
                    entry_id = log_id(&entry_id),
                    attempt_id = log_id(&attempt_id),
                    "reconcile: unsettled artifact metadata belongs to a terminal or superseded receive"
                );
                return Err(anyhow!(
                    "terminal or superseded receive has unsettled artifact metadata"
                ));
            }
        }

        let attempts = self
            .list_attempts
            .list_non_terminal_attempts()
            .await
            .map_err(anyhow::Error::new)?;
        for attempt in attempts {
            let terminal = if attempt.state == AttemptState::Cancelling {
                PartialReceiveTerminal::Cancelled
            } else {
                if matches!(
                    attempt.state,
                    AttemptState::Receiving | AttemptState::Committing
                ) {
                    let outcome = self
                        .begin_failure
                        .begin_receive_failure(
                            &attempt.entry_id,
                            &attempt.current_attempt_id,
                            self.clock.now_ms(),
                        )
                        .await
                        .map_err(anyhow::Error::new)?;
                    if outcome != BeginReceiveFailureOutcome::Begun {
                        uc_error!(
                            entry_id = log_id(&attempt.entry_id),
                            attempt_id = log_id(&attempt.current_attempt_id),
                            "reconcile: interrupted receive lost its failure claim during recovery"
                        );
                        return Err(anyhow!("interrupted receive failure claim was lost"));
                    }
                }
                PartialReceiveTerminal::Failed
            };

            match terminal {
                PartialReceiveTerminal::Cancelled => cancelled = cancelled.saturating_add(1),
                PartialReceiveTerminal::Failed => failed = failed.saturating_add(1),
            }
            let artifact_record =
                artifacts.remove(&(attempt.entry_id.clone(), attempt.current_attempt_id.clone()));
            if artifact_record.is_some() {
                rolled_back = rolled_back.saturating_add(1);
            }
            let mut cleanup = artifact_record
                .as_ref()
                .map(|record| record.artifacts.clone())
                .unwrap_or_default();
            let mut staging_areas = HashSet::new();
            if let Some(directory) = self
                .get_directory_publish
                .get_publish_record(&attempt.entry_id, &attempt.current_attempt_id)
                .await
                .map_err(anyhow::Error::new)?
            {
                // 所有已撤回根的staging证明必须保留到零可见根持久化成功。
                // 否则NoEntry失败后的恢复会把用户新建的同名目标误认作旧输出。
                for (staged_path, final_path) in &directory.root_map {
                    let area = directory_staging_area_for_root(staged_path)?;
                    staging_areas.insert(area.clone());
                    if directory.partial_visible_roots != Some(0)
                        && directory.phase != PublishPhase::Staging
                        && !tokio::fs::try_exists(staged_path).await?
                    {
                        tokio::fs::create_dir_all(&area).await?;
                        if tokio::fs::try_exists(final_path).await? {
                            // 目标在原尝试独占的私有staging中，尚未释放给其他动作。
                            tokio::fs::rename(final_path, staged_path).await?;
                        } else {
                            // 空标记也须跨失败保留，防止后续重用的目标被重新认领。
                            tokio::fs::create_dir(staged_path).await?;
                        }
                    }
                    for artifact in &mut cleanup {
                        if artifact.final_path == *final_path {
                            artifact.ownership = ReceiveArtifactOwnership::UserDestination;
                        }
                    }
                }
                if directory.partial_visible_roots != Some(0) {
                    self.record_directory_publish
                        .record_partial(
                            &attempt.entry_id,
                            &attempt.current_attempt_id,
                            0,
                            self.clock.now_ms(),
                        )
                        .await?;
                }
                for (index, (staged_path, final_path)) in directory.root_map.into_iter().enumerate()
                {
                    cleanup.push(ReceiveArtifact {
                        item_id: format!("directory-root-{index}"),
                        staged_path,
                        final_path,
                        ownership: ReceiveArtifactOwnership::UserDestination,
                    });
                }
            }
            self.cleanup_artifacts
                .cleanup_receive_artifacts(&cleanup)
                .await
                .map_err(anyhow::Error::new)?;

            let areas = staging_areas
                .into_iter()
                .enumerate()
                .map(|(index, path)| ReceiveArtifact {
                    item_id: format!("directory-area-{index}"),
                    staged_path: path.clone(),
                    final_path: path,
                    ownership: ReceiveArtifactOwnership::ManagedStaging,
                })
                .collect::<Vec<_>>();
            self.cleanup_artifacts
                .cleanup_receive_artifacts(&areas)
                .await
                .map_err(anyhow::Error::new)?;

            self.commit
                .commit_inbound_receive(&InboundReceiveSettlement::NoEntry {
                    entry_id: EntryId::from(attempt.entry_id.as_str()),
                    attempt_id: attempt.current_attempt_id,
                    terminal,
                    artifacts: if artifact_record.is_some() {
                        NoEntryReceiveArtifacts::RolledBack
                    } else {
                        NoEntryReceiveArtifacts::None
                    },
                    now_ms: self.clock.now_ms(),
                })
                .await
                .map_err(anyhow::Error::new)?;
            reconciled = reconciled.saturating_add(1);
        }

        if !artifacts.is_empty() {
            uc_error!(
                orphaned = artifacts.len(),
                "reconcile: unsettled receive artifacts had no matching non-terminal attempt"
            );
            return Err(anyhow!("unsettled receive artifacts were not reconciled"));
        }
        if reconciled > 0 {
            uc_info!(
                reconciled = reconciled,
                provisional_discarded = provisional_discarded,
                failed = failed,
                cancelled = cancelled,
                rolled_back = rolled_back,
                "receive attempts reconciled after restart"
            );
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;
    use uc_core::ports::{
        AttemptError, DirectoryPublishRecord, EntryReceiveAttempt, InboundReceiveCommitError,
        ProvisionalReceiveError, ProvisionalReceiveRecovery, PublishLogError, PublishPhase,
        ReceiveArtifactLogError, ReceiveArtifactPhase, ReceiveArtifactRecord,
        ReceiveArtifactResolution, DIRECTORY_RECEIVE_STAGING_PREFIX,
    };

    use super::*;

    #[tokio::test]
    async fn recovery_does_not_run_inside_an_accepted_history_clear() {
        let readiness = ReceiveReadinessCoordinator::new();
        readiness.mark_ready();
        let registry = readiness.work_registry();
        let mut scope = registry.clear_scope().await.unwrap();
        let called = AtomicUsize::new(0);
        let mut waiter = Box::pin(readiness.wait_ready());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), waiter.as_mut())
                .await
                .is_err()
        );
        let mut recovery = Box::pin(readiness.ensure_ready(|| async {
            called.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), recovery.as_mut())
                .await
                .is_err()
        );
        assert_eq!(called.load(Ordering::SeqCst), 0);
        scope.stop().await.unwrap();
        scope.complete();
        drop(scope);
        recovery.await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap();
        assert_eq!(called.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn history_clear_seals_prepares_before_waiting_for_an_existing_recovery() {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let registry = readiness.work_registry();
        let preparing = registry.action();
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let running = {
            let readiness = readiness.clone();
            tokio::spawn(async move {
                readiness
                    .ensure_ready(|| async {
                        entered.send(()).unwrap();
                        released.await.unwrap();
                        Ok(())
                    })
                    .await
            })
        };
        started.await.unwrap();
        let mut clearing = Box::pin(registry.clear_scope());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), clearing.as_mut())
                .await
                .is_err()
        );
        assert!(preparing.cancellation().is_cancelled());
        release.send(()).unwrap();
        running.await.unwrap().unwrap();
        assert!(!readiness.is_ready());
        let mut scope = clearing.await.unwrap();
        drop(preparing);
        scope.stop().await.unwrap();
        scope.complete();
        drop(scope);
        assert!(readiness.is_ready());
    }

    #[tokio::test]
    async fn readiness_waiters_open_only_after_reconciliation_succeeds() {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let waiter = {
            let readiness = Arc::clone(&readiness);
            tokio::spawn(async move { readiness.wait_ready().await })
        };

        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        readiness.mark_ready();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter must be released")
            .expect("waiter task must succeed");
    }

    #[tokio::test]
    async fn failed_recovery_is_recorded_with_a_fixed_kind_and_no_error_text() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let readiness = ReceiveReadinessCoordinator::new();

        readiness
            .ensure_ready(|| async { Err(anyhow!("PRIVATE_ARTIFACT_PATH cannot be decrypted")) })
            .await
            .expect_err("failed recovery must remain observable");

        assert_eq!(logs.count("receive readiness recovery failed"), 1);
        assert!(logs.output().contains("error_kind=\"receive_recovery\""));
        assert!(!logs.output().contains("PRIVATE"));
    }

    #[tokio::test]
    async fn readiness_failure_stays_closed_and_successful_retry_opens_gate() {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let waiter = {
            let readiness = Arc::clone(&readiness);
            tokio::spawn(async move { readiness.wait_ready().await })
        };

        let error = readiness
            .ensure_ready(|| async { Err(anyhow!("artifact metadata cannot be decrypted")) })
            .await
            .expect_err("failed recovery must remain observable");

        assert_eq!(error.to_string(), "artifact metadata cannot be decrypted");
        assert_eq!(
            readiness.degraded_reason().as_deref(),
            Some("artifact metadata cannot be decrypted")
        );
        assert!(!readiness.is_ready());
        assert!(!waiter.is_finished());

        readiness
            .ensure_ready(|| async { Ok(()) })
            .await
            .expect("recovery retry should succeed");

        assert!(readiness.is_ready());
        assert!(readiness.degraded_reason().is_none());
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("waiter must be released after successful retry")
            .expect("waiter task must succeed");
    }

    #[tokio::test]
    async fn closing_gate_during_recovery_cannot_reopen_it_or_readmit_old_actions() {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let work = readiness.work_registry();
        let old = work.action();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let recovering = {
            let readiness = readiness.clone();
            tokio::spawn(async move {
                readiness
                    .ensure_ready(|| async {
                        entered.send(()).unwrap();
                        released.await.unwrap();
                        Ok(())
                    })
                    .await
            })
        };
        waiting.await.unwrap();
        readiness.mark_not_ready();
        release.send(()).unwrap();
        assert!(recovering.await.unwrap().is_err());
        assert!(!readiness.is_ready());
        assert!(old.register(&EntryId::from("entry"), "late").is_err());
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        assert!(readiness.is_ready());
        assert!(old.register(&EntryId::from("entry"), "still-late").is_err());
        let current = work.action();
        assert!(current.register(&EntryId::from("entry"), "current").is_ok());
        assert!(!current.cancellation().is_cancelled());
        current.finish(Ok(()));
    }

    #[tokio::test]
    async fn superseded_recovery_failure_cannot_cancel_the_next_generations_waiting_action() {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let recovering = {
            let readiness = readiness.clone();
            tokio::spawn(async move {
                readiness
                    .ensure_ready(|| async {
                        entered.send(()).unwrap();
                        released.await.unwrap();
                        Err(anyhow!("old recovery failure"))
                    })
                    .await
            })
        };
        waiting.await.unwrap();
        readiness.mark_not_ready();
        let current = readiness.work_registry().action();
        release.send(()).unwrap();
        assert!(recovering.await.unwrap().is_err());
        assert!(!current.cancellation().is_cancelled());
        assert!(readiness.degraded_reason().is_none());
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        assert!(current.register(&EntryId::from("entry"), "current").is_ok());
        current.finish(Ok(()));
    }

    struct Store {
        attempt: EntryReceiveAttempt,
        artifact: ReceiveArtifactRecord,
        include_artifact: bool,
        directory: Mutex<Option<DirectoryPublishRecord>>,
        fail_commit_once: std::sync::atomic::AtomicBool,
        fail_partial_once: std::sync::atomic::AtomicBool,
        provisional: Mutex<Vec<ProvisionalReceiveRecovery>>,
        finalized_provisional: AtomicUsize,
        commit_calls: AtomicUsize,
    }

    #[async_trait]
    impl GetEntryAttemptPort for Store {
        async fn get_entry_attempt(
            &self,
            _entry_id: &str,
        ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
            Ok(Some(self.attempt.clone()))
        }
    }

    #[async_trait]
    impl ListNonTerminalAttemptsPort for Store {
        async fn list_non_terminal_attempts(
            &self,
        ) -> Result<Vec<EntryReceiveAttempt>, AttemptError> {
            Ok((!self.attempt.state.is_terminal())
                .then(|| self.attempt.clone())
                .into_iter()
                .collect())
        }
    }

    #[async_trait]
    impl ListUnsettledReceiveArtifactsPort for Store {
        async fn list_unsettled_receive_artifacts(
            &self,
        ) -> Result<Vec<ReceiveArtifactRecord>, ReceiveArtifactLogError> {
            Ok(self
                .include_artifact
                .then(|| self.artifact.clone())
                .into_iter()
                .collect())
        }
    }

    #[async_trait]
    impl ListProvisionalReceivesPort for Store {
        async fn list_provisional_receives(
            &self,
        ) -> Result<Vec<ProvisionalReceiveRecovery>, ProvisionalReceiveError> {
            Ok(self.provisional.lock().unwrap().clone())
        }
    }

    #[async_trait]
    impl FinalizeProvisionalReceivePort for Store {
        async fn finalize_provisional_receive(
            &self,
            _provisional_transfer_id: &str,
            _action: ProvisionalReceiveAction,
            _now_ms: i64,
        ) -> Result<(), ProvisionalReceiveError> {
            self.finalized_provisional.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl GetDirectoryPublishRecordPort for Store {
        async fn get_publish_record(
            &self,
            _entry_id: &str,
            _attempt_id: &str,
        ) -> Result<Option<DirectoryPublishRecord>, PublishLogError> {
            Ok(self.directory.lock().unwrap().clone())
        }
    }

    #[async_trait]
    impl RecordDirectoryPublishPort for Store {
        async fn record_phase(
            &self,
            _entry_id: &str,
            _attempt_id: &str,
            _phase: PublishPhase,
            _root_map: &[(PathBuf, PathBuf)],
            _now_ms: i64,
        ) -> Result<(), PublishLogError> {
            unreachable!("恢复只更新撤回检查点")
        }

        async fn record_partial(
            &self,
            _entry_id: &str,
            _attempt_id: &str,
            visible_roots: u32,
            _now_ms: i64,
        ) -> Result<(), PublishLogError> {
            if self.fail_partial_once.swap(false, Ordering::SeqCst) {
                return Err(PublishLogError::Backend(Box::new(std::io::Error::from(
                    std::io::ErrorKind::PermissionDenied,
                ))));
            }
            self.directory
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .partial_visible_roots = Some(visible_roots);
            Ok(())
        }
    }

    #[async_trait]
    impl BeginReceiveFailurePort for Store {
        async fn begin_receive_failure(
            &self,
            _entry_id: &str,
            _attempt_id: &str,
            _now_ms: i64,
        ) -> Result<BeginReceiveFailureOutcome, AttemptError> {
            Ok(BeginReceiveFailureOutcome::Begun)
        }
    }

    #[async_trait]
    impl CommitInboundReceivePort for Store {
        async fn commit_inbound_receive(
            &self,
            settlement: &InboundReceiveSettlement,
        ) -> Result<(), InboundReceiveCommitError> {
            assert!(matches!(
                settlement,
                InboundReceiveSettlement::NoEntry {
                    artifacts: NoEntryReceiveArtifacts::RolledBack,
                    ..
                }
            ));
            self.commit_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_commit_once.swap(false, Ordering::SeqCst) {
                return Err(InboundReceiveCommitError::Backend(Box::new(
                    std::io::Error::from(std::io::ErrorKind::PermissionDenied),
                )));
            }
            Ok(())
        }
    }

    struct Clock;

    struct TestArtifactCleaner;

    #[async_trait]
    impl CleanupReceiveArtifactsPort for TestArtifactCleaner {
        async fn cleanup_receive_artifacts(
            &self,
            artifacts: &[ReceiveArtifact],
        ) -> Result<(), uc_core::ports::ReceiveArtifactLogError> {
            for artifact in artifacts {
                let paths = match artifact.ownership {
                    ReceiveArtifactOwnership::ManagedStaging
                        if artifact.staged_path != artifact.final_path =>
                    {
                        vec![&artifact.staged_path, &artifact.final_path]
                    }
                    ReceiveArtifactOwnership::ManagedStaging => vec![&artifact.staged_path],
                    ReceiveArtifactOwnership::UserDestination
                        if artifact.staged_path != artifact.final_path =>
                    {
                        vec![&artifact.staged_path]
                    }
                    ReceiveArtifactOwnership::UserDestination => Vec::new(),
                };
                for path in paths {
                    match tokio::fs::symlink_metadata(path).await {
                        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                            if let Err(error) = tokio::fs::remove_dir_all(path).await {
                                if error.kind() != std::io::ErrorKind::NotFound {
                                    return Err(uc_core::ports::ReceiveArtifactLogError::Backend(
                                        error.into(),
                                    ));
                                }
                            }
                        }
                        Ok(_) => {
                            if let Err(error) = tokio::fs::remove_file(path).await {
                                if error.kind() != std::io::ErrorKind::NotFound {
                                    return Err(uc_core::ports::ReceiveArtifactLogError::Backend(
                                        error.into(),
                                    ));
                                }
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            return Err(uc_core::ports::ReceiveArtifactLogError::Backend(
                                error.into(),
                            ));
                        }
                    }
                }
            }
            Ok(())
        }
    }

    impl ClockPort for Clock {
        fn now_ms(&self) -> i64 {
            10
        }
    }

    fn store(state: AttemptState, path: PathBuf) -> Arc<Store> {
        Arc::new(Store {
            attempt: EntryReceiveAttempt {
                entry_id: "entry-1".to_owned(),
                current_attempt_id: "attempt-1".to_owned(),
                state,
                updated_at_ms: 1,
            },
            artifact: ReceiveArtifactRecord {
                entry_id: "entry-1".to_owned(),
                attempt_id: "attempt-1".to_owned(),
                phase: ReceiveArtifactPhase::Preparing,
                resolution: ReceiveArtifactResolution::Pending,
                artifacts: vec![ReceiveArtifact {
                    item_id: "item-1".to_owned(),
                    staged_path: path.clone(),
                    final_path: path,
                    ownership: ReceiveArtifactOwnership::ManagedStaging,
                }],
                updated_at_ms: 1,
            },
            include_artifact: true,
            directory: Mutex::new(None),
            fail_commit_once: std::sync::atomic::AtomicBool::new(false),
            fail_partial_once: std::sync::atomic::AtomicBool::new(false),
            provisional: Mutex::new(Vec::new()),
            finalized_provisional: AtomicUsize::new(0),
            commit_calls: AtomicUsize::new(0),
        })
    }

    fn use_case(store: Arc<Store>) -> ReconcileReceiveAttemptsUseCase {
        ReconcileReceiveAttemptsUseCase::new(
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            store.clone(),
            Arc::new(TestArtifactCleaner),
            store.clone(),
            store.clone(),
            store,
            Arc::new(Clock),
        )
    }

    #[tokio::test]
    async fn interrupted_receive_is_cleaned_before_it_is_failed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.bin");
        tokio::fs::write(&path, b"partial").await.unwrap();
        let store = store(AttemptState::Receiving, path.clone());

        assert_eq!(use_case(store.clone()).execute().await.unwrap(), 1);
        assert!(!path.exists());
        assert_eq!(store.commit_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn restart_recovery_records_a_numeric_summary_only_when_it_changed_state() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.bin");
        tokio::fs::write(&path, b"partial").await.unwrap();
        let store = store(AttemptState::Receiving, path);

        use_case(store).execute().await.unwrap();

        assert_eq!(logs.count("receive attempts reconciled after restart"), 1);
        let output = logs.output();
        assert!(output.contains("reconciled=1"));
        assert!(output.contains("failed=1"));
        assert!(output.contains("rolled_back=1"));
        assert!(output.contains("cancelled=0"));
        assert!(!output.contains("partial.bin"));
    }

    #[tokio::test]
    async fn provisional_orphan_file_is_deleted_before_its_row_is_discarded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("provisional.bin");
        tokio::fs::write(&path, b"orphan").await.unwrap();
        let staged_uri = url::Url::from_file_path(&path).unwrap().to_string();
        let placeholder = path.with_file_name("unused.bin");
        let store = Arc::new(Store {
            attempt: EntryReceiveAttempt {
                entry_id: "completed-entry".to_owned(),
                current_attempt_id: "completed-attempt".to_owned(),
                state: AttemptState::Completed,
                updated_at_ms: 1,
            },
            artifact: ReceiveArtifactRecord {
                entry_id: "completed-entry".to_owned(),
                attempt_id: "completed-attempt".to_owned(),
                phase: ReceiveArtifactPhase::Settled,
                resolution: ReceiveArtifactResolution::Landed,
                artifacts: vec![ReceiveArtifact {
                    item_id: "unused".to_owned(),
                    staged_path: placeholder.clone(),
                    final_path: placeholder,
                    ownership: ReceiveArtifactOwnership::ManagedStaging,
                }],
                updated_at_ms: 1,
            },
            include_artifact: false,
            directory: Mutex::new(None),
            fail_commit_once: std::sync::atomic::AtomicBool::new(false),
            fail_partial_once: std::sync::atomic::AtomicBool::new(false),
            provisional: Mutex::new(vec![ProvisionalReceiveRecovery {
                transfer_id: "mobile-orphan".to_owned(),
                cached_path: Some(staged_uri),
            }]),
            finalized_provisional: AtomicUsize::new(0),
            commit_calls: AtomicUsize::new(0),
        });

        assert_eq!(use_case(store.clone()).execute().await.unwrap(), 1);
        assert!(!path.exists());
        assert_eq!(store.finalized_provisional.load(Ordering::SeqCst), 1);
        assert_eq!(store.commit_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn terminal_attempt_with_pending_artifacts_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("must-remain.bin");
        tokio::fs::write(&path, b"keep").await.unwrap();
        let store = store(AttemptState::Completed, path.clone());

        assert!(use_case(store.clone()).execute().await.is_err());
        assert!(path.exists());
        assert_eq!(store.commit_calls.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn directory_restart_cleanup_preserves_reused_names_and_removes_only_owned_outputs() {
        for zero_visible in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let area = root
                .path()
                .join(format!("{DIRECTORY_RECEIVE_STAGING_PREFIX}fixture"));
            let staged_alpha = area.join("alpha");
            let staged_beta = area.join("beta");
            let alpha = root.path().join("alpha");
            let beta = root.path().join("beta");
            tokio::fs::create_dir_all(&alpha).await.unwrap();
            tokio::fs::create_dir_all(&beta).await.unwrap();
            tokio::fs::write(alpha.join("new.txt"), b"new alpha")
                .await
                .unwrap();
            if zero_visible {
                tokio::fs::write(beta.join("new.txt"), b"new beta")
                    .await
                    .unwrap();
            } else {
                tokio::fs::create_dir_all(&staged_alpha).await.unwrap();
                tokio::fs::write(staged_alpha.join("old.txt"), b"withdrawn original")
                    .await
                    .unwrap();
                tokio::fs::write(beta.join("old.txt"), b"owned original")
                    .await
                    .unwrap();
            }
            let store = store(AttemptState::Failing, root.path().join("absent-artifact"));
            *store.directory.lock().unwrap() = Some(DirectoryPublishRecord {
                entry_id: "entry-1".into(),
                attempt_id: "attempt-1".into(),
                phase: PublishPhase::Publishing,
                root_map: vec![(staged_alpha, alpha.clone()), (staged_beta, beta.clone())],
                partial_visible_roots: Some(if zero_visible { 0 } else { 1 }),
                landed: false,
            });
            assert_eq!(use_case(store).execute().await.unwrap(), 1);
            assert_eq!(
                tokio::fs::read(alpha.join("new.txt")).await.unwrap(),
                b"new alpha"
            );
            if zero_visible {
                assert_eq!(
                    tokio::fs::read(beta.join("new.txt")).await.unwrap(),
                    b"new beta"
                );
            } else {
                assert!(!beta.exists(), "仍由旧尝试拥有的未撤回根必须清理");
            }
            assert!(!area.exists(), "原目录staging区域也须在放行新接收前清理");
        }
    }
    #[tokio::test]
    async fn directory_recovery_commit_failure_never_recleans_names_reused_before_the_next_recovery(
    ) {
        for fail_checkpoint in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let area = root
                .path()
                .join(format!("{DIRECTORY_RECEIVE_STAGING_PREFIX}retry"));
            let staged = area.join("root");
            let target = root.path().join("root");
            tokio::fs::create_dir_all(&target).await.unwrap();
            tokio::fs::write(target.join("original.txt"), b"old receive")
                .await
                .unwrap();
            let store = store(AttemptState::Failing, root.path().join("absent-artifact"));
            *store.directory.lock().unwrap() = Some(DirectoryPublishRecord {
                entry_id: "entry-1".into(),
                attempt_id: "attempt-1".into(),
                phase: PublishPhase::Publishing,
                root_map: vec![(staged, target.clone())],
                partial_visible_roots: Some(1),
                landed: false,
            });
            if fail_checkpoint {
                store.fail_partial_once.store(true, Ordering::SeqCst);
            } else {
                store.fail_commit_once.store(true, Ordering::SeqCst);
            }
            let recovery = use_case(store);
            assert!(
                recovery.execute().await.is_err(),
                "真实清理后的NoEntry拒绝必须返回失败"
            );
            assert!(!target.exists());
            tokio::fs::create_dir_all(&target).await.unwrap();
            tokio::fs::write(target.join("new.txt"), b"new user data")
                .await
                .unwrap();
            assert_eq!(recovery.execute().await.unwrap(), 1);
            assert_eq!(
                tokio::fs::read(target.join("new.txt")).await.unwrap(),
                b"new user data",
                "后续恢复不能再次触碰已清理后重用的名称"
            );
        }
    }
}
