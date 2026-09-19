use std::sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard};

use async_trait::async_trait;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::crypto::domain::Passphrase;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::security::{
    ProfileKeyRecoveryError, ProfileKeyRecoveryStore, ProfileRecoveryOutcome,
    ProfileRecoveryPreparation,
};

use super::ProductionRuntime;
use crate::assembly::host::{adapt_shared_secure_storage, derive_app_paths};
use crate::engine::event_stream::EventSender;
use crate::engine::startup::StartupProgressStore;
use crate::engine::EngineRuntime;
use crate::error_codes::{
    PROFILE_RECOVERY_PARTIAL_CODE, PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
    PROFILE_RECOVERY_REQUIRED_CODE, PROFILE_RECOVERY_UNSUPPORTED_CODE, UNLOCK_SPACE_CORRUPTED_CODE,
    UNLOCK_SPACE_UNAUTHORIZED_CODE,
};
#[cfg(feature = "dev-tools")]
use crate::{DevOperation, DevOperationResult};
use crate::{
    EncryptionStateSummary, EngineConfig, EngineError, EngineErrorCategory, EngineEvent,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostSecureStorage,
    Operation, OperationKind, OperationResult, ProfileRecoveryLoss, ProfileRecoveryState,
    ProfileRecoverySummary,
};

#[derive(Clone)]
enum RuntimeMode {
    Ready {
        runtime: Arc<ProductionRuntime>,
        recovered: bool,
    },
    Recovery(Arc<RecoveryBootstrap>),
}

struct RecoveryBootstrap {
    input: Mutex<Option<(EngineConfig, HostCapabilities)>>,
    gate: Mutex<()>,
    summary: StdMutex<ProfileRecoverySummary>,
    progress: Arc<StartupProgressStore>,
}

pub(crate) struct RecoverableRuntime {
    mode: RwLock<RuntimeMode>,
    recovery: Arc<ProfileKeyRecoveryStore>,
    events: EventSender,
}

impl RecoverableRuntime {
    pub(crate) async fn start(
        config: EngineConfig,
        mut host: HostCapabilities,
        events: EventSender,
        progress: Arc<StartupProgressStore>,
    ) -> Result<Self, EngineError> {
        let paths = derive_app_paths(host.directories());
        let backing = adapt_shared_secure_storage(Arc::clone(&host.secure_storage));
        let recovery = Arc::new(ProfileKeyRecoveryStore::new(
            paths.vault_dir,
            config.profile_id().to_owned(),
            backing,
        ));
        let mode = match recovery.prepare_startup().await? {
            ProfileRecoveryPreparation::Ready => {
                host.replace_secure_storage(Arc::new(RecoveryHostStorage {
                    inner: Arc::clone(&recovery),
                }));
                RuntimeMode::Ready {
                    runtime: Arc::new(
                        ProductionRuntime::start(
                            config,
                            host,
                            events.clone(),
                            Arc::clone(&progress),
                        )
                        .await?,
                    ),
                    recovered: false,
                }
            }
            ProfileRecoveryPreparation::AwaitingPassphrase {
                independent_material_missing,
            } => {
                progress.recovery_available();
                let summary = ProfileRecoverySummary {
                    state: if independent_material_missing {
                        ProfileRecoveryState::PartiallyRecoverable
                    } else {
                        ProfileRecoveryState::AwaitingPassphrase
                    },
                    can_submit_passphrase: true,
                    restart_required: false,
                    background_ready: false,
                    cleanup_pending: false,
                    losses: losses(independent_material_missing),
                };
                events.send(EngineEvent::ProfileRecoveryChanged(summary.clone()));
                RuntimeMode::Recovery(Arc::new(RecoveryBootstrap {
                    input: Mutex::new(Some((config, host))),
                    gate: Mutex::new(()),
                    progress,
                    summary: StdMutex::new(summary),
                }))
            }
        };
        Ok(Self {
            mode: RwLock::new(mode),
            recovery,
            events,
        })
    }

    async fn mode(&self) -> RuntimeMode {
        self.mode.read().await.clone()
    }

    async fn execute_ready(
        &self,
        runtime: Arc<ProductionRuntime>,
        recovered: bool,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        if matches!(operation, Operation::QueryProfileRecovery) {
            return Ok(OperationResult::ProfileRecovery(ProfileRecoverySummary {
                state: if recovered {
                    ProfileRecoveryState::Recovered
                } else {
                    ProfileRecoveryState::NotRequired
                },
                can_submit_passphrase: false,
                restart_required: false,
                background_ready: true,
                cleanup_pending: self.recovery.cleanup_pending(),
                losses: Vec::new(),
            }));
        }
        let kind = operation.kind();
        let result = runtime.execute(operation, cancellation).await?;
        if matches!(kind, OperationKind::FactoryResetSpace) {
            self.recovery.forget_after_factory_reset();
            return Ok(result);
        }
        if matches!(
            kind,
            OperationKind::CreateSpace
                | OperationKind::JoinSpace
                | OperationKind::UnlockSpace
                | OperationKind::ChangeEncryptionPassphrase
                | OperationKind::ResetSpace
        ) {
            self.recovery.refresh_after_authentication().await?;
        }
        Ok(result)
    }

    async fn execute_recovery(
        &self,
        bootstrap: Arc<RecoveryBootstrap>,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        match operation {
            Operation::QueryProfileRecovery => Ok(OperationResult::ProfileRecovery(
                bootstrap.lock_summary().clone(),
            )),
            Operation::QueryEncryptionState => {
                Ok(OperationResult::EncryptionState(EncryptionStateSummary {
                    initialized: true,
                    session_ready: false,
                }))
            }
            Operation::UnlockSpace(input) => {
                let _gate = bootstrap.gate.lock().await;
                if let RuntimeMode::Ready { runtime, recovered } = self.mode().await {
                    return self
                        .execute_ready(
                            runtime,
                            recovered,
                            Operation::UnlockSpace(input),
                            cancellation,
                        )
                        .await;
                }
                if !bootstrap.lock_summary().can_submit_passphrase {
                    return Err(recovery_unavailable());
                }
                self.publish_summary(&bootstrap, |summary| {
                    summary.state = ProfileRecoveryState::Recovering
                });
                let passphrase = Passphrase::new(input.passphrase.expose());
                match self.recovery.recover(&passphrase).await {
                    Ok(ProfileRecoveryOutcome::PartiallyRecoverable) => {
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::PartiallyRecoverable;
                            summary.restart_required = false;
                            summary.losses = losses(true);
                        });
                        Err(EngineError::new(
                            PROFILE_RECOVERY_PARTIAL_CODE,
                            EngineErrorCategory::Unavailable,
                            false,
                        ))
                    }
                    Ok(ProfileRecoveryOutcome::Ready) => {
                        let bootstrap_input = bootstrap.input.lock().await.take();
                        let Some((config, mut host)) = bootstrap_input else {
                            self.publish_restart_required(&bootstrap);
                            return Err(recovery_unavailable());
                        };
                        host.replace_secure_storage(Arc::new(RecoveryHostStorage {
                            inner: Arc::clone(&self.recovery),
                        }));
                        let runtime = match ProductionRuntime::start(
                            config,
                            host,
                            self.events.clone(),
                            Arc::clone(&bootstrap.progress),
                        )
                        .await
                        {
                            Ok(runtime) => Arc::new(runtime),
                            Err(error) => {
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        let result = match self
                            .execute_ready(
                                Arc::clone(&runtime),
                                true,
                                Operation::UnlockSpace(input),
                                cancellation,
                            )
                            .await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                let _ = runtime.shutdown(None).await;
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        *self.mode.write().await = RuntimeMode::Ready {
                            runtime,
                            recovered: true,
                        };
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::Recovered;
                            summary.can_submit_passphrase = false;
                            summary.restart_required = false;
                            summary.background_ready = true;
                            summary.cleanup_pending = self.recovery.cleanup_pending();
                            summary.losses.clear();
                        });
                        Ok(result)
                    }
                    Err(ProfileKeyRecoveryError::WrongPassphrase) => {
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::AwaitingPassphrase;
                            summary.restart_required = false;
                        });
                        Err(EngineError::new(
                            UNLOCK_SPACE_UNAUTHORIZED_CODE,
                            EngineErrorCategory::Unauthorized,
                            false,
                        ))
                    }
                    Err(error) => {
                        let error = EngineError::from(error);
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::Failed;
                            summary.can_submit_passphrase = error.is_retryable();
                            summary.restart_required = false;
                        });
                        Err(error)
                    }
                }
            }
            _ => Err(recovery_unavailable()),
        }
    }

    fn publish_summary(
        &self,
        bootstrap: &RecoveryBootstrap,
        update: impl FnOnce(&mut ProfileRecoverySummary),
    ) {
        let summary = {
            let mut summary = bootstrap.lock_summary();
            update(&mut summary);
            summary.clone()
        };
        self.events
            .send(EngineEvent::ProfileRecoveryChanged(summary));
    }

    fn publish_restart_required(&self, bootstrap: &RecoveryBootstrap) {
        self.publish_summary(bootstrap, |summary| {
            summary.state = ProfileRecoveryState::Failed;
            summary.can_submit_passphrase = false;
            summary.restart_required = true;
            summary.background_ready = false;
        });
    }
}

impl RecoveryBootstrap {
    fn lock_summary(&self) -> StdMutexGuard<'_, ProfileRecoverySummary> {
        self.summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl EngineRuntime for RecoverableRuntime {
    async fn execute(
        &self,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, recovered } => {
                self.execute_ready(runtime, recovered, operation, cancellation)
                    .await
            }
            RuntimeMode::Recovery(bootstrap) => {
                self.execute_recovery(bootstrap, operation, cancellation)
                    .await
            }
        }
    }

    #[cfg(feature = "dev-tools")]
    async fn execute_dev(
        &self,
        operation: DevOperation,
        cancellation: CancellationToken,
    ) -> Result<DevOperationResult, EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => {
                runtime.execute_dev(operation, cancellation).await
            }
            RuntimeMode::Recovery(_) => Err(recovery_unavailable()),
        }
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => {
                // The lifecycle queue keeps an accepted suspend alive after its caller times out.
                // Once the original deadline has elapsed, cleanup must run without reusing that
                // stale deadline or it can leave the production runtime only partly suspended.
                let cleanup_deadline = deadline.filter(|deadline| *deadline > Instant::now());
                runtime.suspend(cleanup_deadline).await
            }
            RuntimeMode::Recovery(_) => Ok(()),
        }
    }

    async fn resume(&self, cancellation: CancellationToken) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => runtime.resume(cancellation).await,
            RuntimeMode::Recovery(_) => Ok(()),
        }
    }

    async fn shutdown(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => runtime.shutdown(deadline).await,
            RuntimeMode::Recovery(_) => Ok(()),
        }
    }
}

struct RecoveryHostStorage {
    inner: Arc<ProfileKeyRecoveryStore>,
}

impl HostSecureStorage for RecoveryHostStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        match self.inner.get(key) {
            Ok(value) => Ok(value),
            Err(error) => Err(error.into()),
        }
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        match self.inner.set(key, value) {
            Ok(()) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        match self.inner.delete(key) {
            Ok(()) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl From<SecureStorageError> for HostCapabilityError {
    fn from(error: SecureStorageError) -> Self {
        let category = match error {
            SecureStorageError::Unavailable(_) => HostCapabilityErrorCategory::Unavailable,
            SecureStorageError::PermissionDenied(_) => {
                HostCapabilityErrorCategory::PermissionDenied
            }
            _ => HostCapabilityErrorCategory::Io,
        };
        Self::new(category, "profile recovery storage failed")
    }
}

impl From<ProfileKeyRecoveryError> for EngineError {
    fn from(error: ProfileKeyRecoveryError) -> Self {
        match error {
            ProfileKeyRecoveryError::WrongPassphrase => Self::new(
                UNLOCK_SPACE_UNAUTHORIZED_CODE,
                EngineErrorCategory::Unauthorized,
                false,
            ),
            ProfileKeyRecoveryError::Corrupt => Self::new(
                UNLOCK_SPACE_CORRUPTED_CODE,
                EngineErrorCategory::Internal,
                false,
            ),
            ProfileKeyRecoveryError::Unsupported => Self::new(
                PROFILE_RECOVERY_UNSUPPORTED_CODE,
                EngineErrorCategory::Unavailable,
                false,
            ),
            ProfileKeyRecoveryError::Storage(_) => Self::new(
                PROFILE_RECOVERY_PERSISTENCE_FAILED_CODE,
                EngineErrorCategory::Unavailable,
                true,
            ),
        }
    }
}

fn recovery_unavailable() -> EngineError {
    EngineError::new(
        PROFILE_RECOVERY_REQUIRED_CODE,
        EngineErrorCategory::Unavailable,
        false,
    )
}

fn restart_required_error(error: EngineError) -> EngineError {
    EngineError::new(error.code(), error.category(), false)
}

fn losses(missing: bool) -> Vec<ProfileRecoveryLoss> {
    if missing {
        vec![
            ProfileRecoveryLoss::LocalHistory,
            ProfileRecoveryLoss::LocalControlState,
            ProfileRecoveryLoss::DeviceIdentity,
        ]
    } else {
        Vec::new()
    }
}
