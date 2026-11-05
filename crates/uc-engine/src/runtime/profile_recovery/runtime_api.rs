use super::*;

impl RecoverableRuntime {
    pub(crate) async fn execute_ready(
        &self,
        runtime: Arc<ProductionRuntime>,
        recovered: bool,
        operation: Operation,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        if matches!(operation, Operation::QueryProfileRecovery) {
            let summary = self
                .lock_ready_summary_override()
                .clone()
                .unwrap_or_else(|| ready_summary(recovered, self.recovery.cleanup_pending()));
            return Ok(OperationResult::ProfileRecovery(summary));
        }
        let kind = operation.kind();
        let result = runtime.execute(operation, cancellation).await?;
        if matches!(
            kind,
            OperationKind::CreateSpace
                | OperationKind::JoinSpace
                | OperationKind::UnlockSpace
                | OperationKind::ResetSpace
        ) {
            match self.recovery.refresh_after_authentication().await {
                Ok(()) => {
                    if self.lock_ready_summary_override().take().is_some() {
                        publish_profile_recovery(
                            &self.events,
                            ready_summary(recovered, self.recovery.cleanup_pending()),
                        );
                    }
                }
                Err(error) => {
                    uc_warn!(
                        error_kind = "profile_recovery_refresh",
                        io_error_kind = io_error_kind(&error),
                        "profile recovery refresh failed after committed operation"
                    );
                    let summary = ProfileRecoverySummary {
                        state: ProfileRecoveryState::Failed,
                        can_submit_passphrase: false,
                        restart_required: false,
                        background_ready: true,
                        cleanup_pending: self.recovery.cleanup_pending(),
                        losses: Vec::new(),
                        admission: None,
                    };
                    *self.lock_ready_summary_override() = Some(summary.clone());
                    publish_profile_recovery(&self.events, summary);
                }
            }
        }
        Ok(result)
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
                if matches!(operation, Operation::FactoryResetSpace) {
                    return self.factory_reset_ready(cancellation).await;
                }
                let result = self
                    .execute_ready(Arc::clone(&runtime), recovered, operation, cancellation)
                    .await;
                self.restrict_after_failure(&runtime, result).await
            }
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => match operation {
                Operation::QueryProfileRecovery => {
                    Ok(OperationResult::ProfileRecovery(restart_required_summary()))
                }
                _ => Err(restart_required_error_for_reset()),
            },
            RuntimeMode::Recovery(bootstrap) => {
                self.execute_recovery(bootstrap, operation, cancellation)
                    .await
            }
            RuntimeMode::AdmissionRecovery { summary, .. } => match operation {
                Operation::QueryProfileRecovery => {
                    Ok(OperationResult::ProfileRecovery(admission_summary(summary)))
                }
                Operation::QueryEncryptionState => {
                    Ok(OperationResult::EncryptionState(EncryptionStateSummary {
                        initialized: true,
                        session_ready: false,
                    }))
                }
                _ => Err(profile_recovery_required_error()),
            },
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
            RuntimeMode::Recovery(_) | RuntimeMode::AdmissionRecovery { .. } => {
                Err(profile_recovery_required_error())
            }
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => {
                Err(restart_required_error_for_reset())
            }
        }
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. }
            | RuntimeMode::AdmissionRecovery {
                runtime: Some(runtime),
                ..
            } => {
                // The lifecycle queue keeps an accepted suspend alive after its caller times out.
                // Once the original deadline has elapsed, cleanup must run without reusing that
                // stale deadline or it can leave the production runtime only partly suspended.
                let cleanup_deadline = deadline.filter(|deadline| *deadline > Instant::now());
                runtime.suspend(cleanup_deadline).await?;
                self.recovery.suspend();
                Ok(())
            }
            // 重置后的旧运行期已经停止，没有可挂起的内容。
            RuntimeMode::Recovery(bootstrap) => {
                if bootstrap.lock_construction().is_some()
                    || bootstrap.lock_recovered_runtime().is_some()
                {
                    return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                }
                Ok(())
            }
            RuntimeMode::AdmissionRecovery { runtime: None, .. }
            | RuntimeMode::Stopped
            | RuntimeMode::RestartRequired { .. } => Ok(()),
        }
    }

    async fn resume(&self, cancellation: CancellationToken) -> Result<(), EngineError> {
        match self.mode().await {
            RuntimeMode::Ready { runtime, .. } => match runtime.resume(cancellation).await {
                Ok(()) => Ok(()),
                // 与启动期一致：恢复为可查询的受限实例，业务操作统一返回需要恢复。
                Err(error) => match runtime.admission_recovery() {
                    Some(summary) => {
                        self.enter_admission_recovery(summary, Some(Arc::clone(&runtime)))
                            .await;
                        Ok(())
                    }
                    None => Err(error),
                },
            },
            // 受限模式不重建业务会话；已启动的运行期保持挂起直到关闭。
            RuntimeMode::Recovery(bootstrap) => {
                if bootstrap.lock_construction().is_some()
                    || bootstrap.lock_recovered_runtime().is_some()
                {
                    return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                }
                Ok(())
            }
            RuntimeMode::AdmissionRecovery { .. }
            | RuntimeMode::Stopped
            | RuntimeMode::RestartRequired { .. } => Ok(()),
        }
    }

    async fn shutdown(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        if !self.settle_reconstruction().await {
            return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
        }
        let stopped_in_recovery = loop {
            let current = self.mode().await;
            // 读取模式时可能等待正在登记的重建；拿到模式后再核对其责任。
            if !self.settle_reconstruction().await {
                return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
            }
            match current {
                RuntimeMode::Ready { runtime, .. }
                | RuntimeMode::AdmissionRecovery {
                    runtime: Some(runtime),
                    ..
                }
                | RuntimeMode::RestartRequired {
                    runtime: Some(runtime),
                } => {
                    runtime.shutdown(deadline).await?;
                    self.recovery.suspend();
                }
                RuntimeMode::Recovery(bootstrap) => {
                    // 恢复动作持有同一门直到模式交接完成。
                    let _recovery = bootstrap.gate.lock().await;
                    bootstrap.wait_for_construction().await;
                    if !matches!(self.mode().await, RuntimeMode::Recovery(current) if Arc::ptr_eq(&current, &bootstrap))
                    {
                        continue;
                    }
                    let construction = bootstrap.lock_construction().clone();
                    if let Some(owner) = construction {
                        if !owner.wait_recovery_backup().await {
                            return Err(EngineError::new(
                                1108,
                                EngineErrorCategory::Internal,
                                false,
                            ));
                        }
                        if bootstrap.construction_interrupted.load(Ordering::Acquire)
                            || !owner.settle_recovery_construction()
                        {
                            return Err(EngineError::new(
                                1108,
                                EngineErrorCategory::Internal,
                                false,
                            ));
                        }
                        *bootstrap.lock_construction() = None;
                    }
                    if bootstrap.lock_recovered_runtime().is_some() {
                        return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
                    }
                    let mut mode = self.mode.write().await;
                    if !matches!(&*mode, RuntimeMode::Recovery(current) if Arc::ptr_eq(current, &bootstrap))
                    {
                        continue;
                    }
                    *mode = RuntimeMode::Stopped;
                    break true;
                }
                RuntimeMode::AdmissionRecovery { runtime: None, .. }
                | RuntimeMode::Stopped
                | RuntimeMode::RestartRequired { runtime: None } => {}
            }
            break false;
        };
        // 最终关闭成功后释放本实例保留的生产运行期。
        // 不修改停止参与者为重置流程保留数据库的契约。
        if !stopped_in_recovery {
            let previous = std::mem::replace(&mut *self.mode.write().await, RuntimeMode::Stopped);
            drop(previous);
        }
        self.host.close_change_stream().await.map_err(|source| {
            uc_warn!(
                error_kind = "change_stream_shutdown",
                io_error_kind = io_error_kind(&source),
                "host clipboard change stream shutdown failed"
            );
            lifecycle_error(LifecycleError {
                primary: anyhow::Error::new(source).context("close host change stream"),
                additional: Vec::new(),
            })
        })?;
        Ok(())
    }

    async fn wait_shutdown_obligations(&self) {
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work().await;
        }
        let mode = self.mode().await;
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work().await;
        }
        if let RuntimeMode::Recovery(bootstrap) = mode {
            bootstrap.wait_accepted_work().await;
        }
    }

    fn wait_shutdown_obligations_blocking(&self) {
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work_blocking();
        }
        let mode = self.mode.blocking_read().clone();
        if let Some(bootstrap) = self.reconstruction() {
            bootstrap.wait_accepted_work_blocking();
        }
        if let RuntimeMode::Recovery(bootstrap) = mode {
            bootstrap.wait_accepted_work_blocking();
        }
    }

    async fn supports_shutdown_retry(&self) -> bool {
        matches!(self.mode().await, RuntimeMode::Stopped) && self.host.has_retryable_change_stream()
    }
}

/// 恢复模式没有启动任何资料运行时，恢复出厂前无需停止。
pub(crate) struct NoProfileRuntime;

#[async_trait]
impl StopProfileRuntimePort for NoProfileRuntime {
    async fn stop_profile_runtime(&self) -> Result<(), LifecycleError> {
        Ok(())
    }
}

pub(crate) struct RecoveryHostStorage {
    pub(crate) inner: Arc<ProfileKeyRecoveryStore>,
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
