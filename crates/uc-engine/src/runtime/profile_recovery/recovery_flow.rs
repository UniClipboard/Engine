use super::*;

impl RecoverableRuntime {
    pub(crate) async fn execute_recovery(
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
                match self.mode().await {
                    RuntimeMode::Ready { runtime, recovered } => {
                        return self
                            .execute_ready(
                                runtime,
                                recovered,
                                Operation::UnlockSpace(input),
                                cancellation,
                            )
                            .await;
                    }
                    RuntimeMode::Recovery(current) if Arc::ptr_eq(&current, &bootstrap) => {}
                    _ => return Err(profile_recovery_required_error()),
                }
                if !bootstrap.lock_summary().can_submit_passphrase {
                    return Err(profile_recovery_required_error());
                }
                self.publish_summary(&bootstrap, |summary| {
                    summary.state = ProfileRecoveryState::Recovering
                });
                let passphrase = Passphrase::new(input.passphrase.expose());
                match self.recovery.recover(&passphrase).await {
                    Ok(ProfileRecoveryOutcome::PartiallyRecoverable(losses)) => {
                        self.publish_summary(&bootstrap, |summary| {
                            summary.state = ProfileRecoveryState::PartiallyRecoverable;
                            summary.can_submit_passphrase = false;
                            summary.restart_required = false;
                            summary.losses = public_losses(losses);
                        });
                        Err(EngineError::new(
                            PROFILE_RECOVERY_PARTIAL_CODE,
                            EngineErrorCategory::Unavailable,
                            false,
                        ))
                    }
                    Ok(ProfileRecoveryOutcome::Ready) => {
                        if !take_startup_input(&bootstrap).await {
                            self.publish_restart_required(&bootstrap);
                            return Err(profile_recovery_required_error());
                        }
                        let paths = derive_app_paths(self.host.directories());
                        let construction = RecoveryConstructionGuard::new(
                            Arc::clone(&bootstrap),
                            Arc::clone(&self.host),
                            self.events.clone(),
                        );
                        let runtime = match ProductionRuntime::start(
                            self.config.clone(),
                            self.recovery_host_capabilities(),
                            paths,
                            self.events.clone(),
                            Arc::clone(&bootstrap.progress),
                            Arc::clone(&self.recovery),
                            Some(&construction.owner),
                        )
                        .await
                        {
                            Ok(runtime) => Arc::new(runtime),
                            Err(error) => {
                                let settled = construction.finish_without_runtime();
                                if !settled {
                                    self.publish_restart_required(&bootstrap);
                                    return Err(restart_required_error(error));
                                }
                                if let Some(summary) = error.admission_recovery() {
                                    bootstrap.progress.recovery_available();
                                    self.enter_admission_recovery(summary, None).await;
                                    return Err(profile_recovery_required_error());
                                }
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        let runtime_guard = construction.handoff(Arc::clone(&runtime));
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
                                let retained = if let Err(shutdown) = runtime.shutdown(None).await {
                                    uc_warn!(
                                        error_kind = "profile_recovery_shutdown",
                                        io_error_kind = io_error_kind(&shutdown),
                                        "profile recovery unlock failed and the runtime did not shut down cleanly"
                                    );
                                    Some(Arc::clone(&runtime))
                                } else {
                                    None
                                };
                                *self.mode.write().await =
                                    RuntimeMode::RestartRequired { runtime: retained };
                                runtime_guard.complete();
                                self.publish_restart_required(&bootstrap);
                                return Err(restart_required_error(error));
                            }
                        };
                        *self.mode.write().await = RuntimeMode::Ready {
                            runtime,
                            recovered: true,
                        };
                        runtime_guard.complete();
                        if self.lock_ready_summary_override().is_none() {
                            self.publish_summary(&bootstrap, |summary| {
                                summary.state = ProfileRecoveryState::Recovered;
                                summary.can_submit_passphrase = false;
                                summary.restart_required = false;
                                summary.background_ready = true;
                                summary.cleanup_pending = self.recovery.cleanup_pending();
                                summary.losses.clear();
                            });
                        }
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
                        uc_warn!(
                            error_kind = "profile_recovery",
                            io_error_kind = io_error_kind(&error),
                            "profile recovery attempt failed"
                        );
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
            Operation::FactoryResetSpace => self.factory_reset_from_recovery(&bootstrap).await,
            _ => Err(profile_recovery_required_error()),
        }
    }

    /// 恢复模式不启动业务运行时；资料密钥完好时只装配恢复出厂依赖并执行重置，完成后须重启 Engine。
    pub(crate) async fn factory_reset_from_recovery(
        &self,
        bootstrap: &Arc<RecoveryBootstrap>,
    ) -> Result<OperationResult, EngineError> {
        let _gate = bootstrap.gate.lock().await;
        if !*bootstrap.input_available.lock().await {
            return Err(profile_recovery_required_error());
        }
        if !self.recovery.open_for_factory_reset().await? {
            return Err(profile_recovery_required_error());
        }
        if !take_startup_input(bootstrap).await {
            return Err(profile_recovery_required_error());
        }
        let paths = derive_app_paths(self.host.directories());
        let construction = RecoveryConstructionGuard::new(
            Arc::clone(bootstrap),
            Arc::clone(&self.host),
            self.events.clone(),
        );
        let wiring = wire_host_capabilities_with_emitter(
            &self.config,
            self.recovery_host_capabilities(),
            paths,
            Arc::new(EngineHostEventEmitter::new(self.events.clone())),
            Arc::clone(&bootstrap.progress),
            Arc::clone(&self.recovery),
            Some(&construction.owner),
        )
        .await;
        let wired = match wiring {
            Ok(wiring) => wiring.wired,
            Err(error) => {
                construction.finish_without_runtime();
                self.publish_restart_required(bootstrap);
                return Err(restart_required_error(startup_error(
                    "factory reset dependency wiring",
                    error,
                )));
            }
        };
        let reset = ProfileFactoryResetFacade::new(
            Arc::clone(&wired.profile_reset.lifecycle_repository),
            Arc::new(NoProfileRuntime),
            Arc::clone(&wired.profile_reset.keys),
            Arc::clone(&wired.profile_reset.backup_security),
            Arc::clone(&wired.profile_reset.state),
        );
        let result = execute_factory_reset_space(&reset).await;
        self.recovery.forget_after_factory_reset();
        drop(reset);
        drop(wired);
        if !construction.finish_without_runtime() {
            self.publish_restart_required(bootstrap);
            return Err(EngineError::new(1108, EngineErrorCategory::Internal, false));
        }
        match result {
            Ok(result) => {
                self.publish_summary(bootstrap, |summary| {
                    summary.state = ProfileRecoveryState::NotRequired;
                    summary.can_submit_passphrase = false;
                    summary.restart_required = true;
                    summary.background_ready = false;
                    summary.cleanup_pending = false;
                    summary.losses.clear();
                });
                Ok(result)
            }
            Err(error) => {
                self.publish_restart_required(bootstrap);
                Err(restart_required_error(error))
            }
        }
    }

    /// 离开空间：重置、释放旧运行期、按启动决策重建，由这里统一负责。
    ///
    /// 全程持有模式写锁，其他操作要么在旧运行期完成后排队，要么直接面对新运行期。
    /// 重置本身失败时保持原运行期（重置阶段已持久，重试同一操作会续做）；
    /// 清除完成但新运行期装配失败时转入需要重启，不报告成功。
    pub(crate) async fn factory_reset_ready(
        &self,
        cancellation: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        let mut mode = self.mode.write().await;
        let runtime = match &*mode {
            RuntimeMode::Ready { runtime, .. } => Arc::clone(runtime),
            RuntimeMode::Stopped | RuntimeMode::RestartRequired { .. } => {
                return Err(restart_required_error_for_reset())
            }
            RuntimeMode::Recovery(_) | RuntimeMode::AdmissionRecovery { .. } => {
                return Err(profile_recovery_required_error())
            }
        };
        let result = runtime
            .execute(Operation::FactoryResetSpace, cancellation)
            .await?;
        self.recovery.forget_after_factory_reset();
        *self.lock_ready_summary_override() = None;

        if let Err(error) = runtime.shutdown(None).await {
            uc_warn!(
                error_code = error.code(),
                "space left but the previous runtime did not release cleanly"
            );
            return Err(self.require_restart(&mut mode, Some(runtime)));
        }
        // 启动进度属于宿主观察过的那一次启动，这里重建使用不被任何人观察的独立进度。
        let (detached_progress, _) = StartupProgress::channel();
        let bootstrap = Arc::new(RecoveryBootstrap::new(
            Arc::clone(&detached_progress.store),
            restart_required_summary(),
            false,
        ));
        *self
            .reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&bootstrap));
        let construction = RecoveryConstructionGuard::new(
            Arc::clone(&bootstrap),
            Arc::clone(&self.host),
            self.events.clone(),
        );
        match start_profile_mode(
            &self.config,
            &self.host,
            &self.events,
            &detached_progress.store,
            &self.recovery,
            Some(&construction.owner),
        )
        .await
        {
            Ok(next) => {
                uc_info!("space left and a fresh runtime is ready");
                *mode = next;
                construction.handoff_mode();
                *self
                    .reconstruction
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                Ok(result)
            }
            Err(error) => {
                uc_warn!(
                    error_code = error.code(),
                    "space left but the fresh runtime could not start"
                );
                if construction.finish_without_runtime() {
                    *self
                        .reconstruction
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
                Err(self.require_restart(&mut mode, None))
            }
        }
    }
}
