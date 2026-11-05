use super::*;

impl SessionSupervisor {
    pub(super) fn configured_factory(&self) -> Option<Arc<ProductionSessionFactory>> {
        self.factory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) async fn stop_current_session(
        &self,
        reason: FileTransferCancellationReason,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let mut errors = Vec::new();
        if let Err(error) = self.quiesce_network_session().await {
            errors.push(error.into());
        }
        let session = self.runtime.lock().await.session.take();
        if let Some(session) = session {
            if let Err(error) = session.shutdown(deadline).await {
                errors.push(error.into());
            }
        }
        if let Err(error) = self.application.cancel_active_file_transfers(reason).await {
            errors.push(anyhow::Error::new(error).context("cancel active file transfers"));
        }
        LifecycleError::from_errors(errors)
    }

    pub(super) async fn quiesce_network_session(&self) -> Result<(), EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionQuiesce)
            .consume()
        {
            return Err(session_runtime_error(
                "quiesce p2p session",
                "injected session quiesce failure",
            ));
        }
        let mut runtime = self.runtime.lock().await;
        let Some(network) = runtime.network.as_mut() else {
            return Ok(());
        };
        network
            .quiesce_session()
            .await
            .map_err(|error| session_runtime_error("quiesce p2p session", error))
    }

    pub(super) async fn shutdown_network(
        &self,
        deadline: Option<Instant>,
    ) -> Result<(), EngineError> {
        let network = {
            let mut runtime = self.runtime.lock().await;
            if runtime.session.is_some() {
                return Err(operation_unavailable_error());
            }
            runtime.network.take()
        };
        if let Some(network) = network {
            network
                .shutdown_at(deadline)
                .await
                .map_err(|error| session_runtime_error("p2p network shutdown", error))?;
        }
        Ok(())
    }

    pub(super) async fn complete_pending_space_transition(
        &self,
        facade: &AppFacade,
        error_context: &'static str,
    ) -> Result<(), EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::TransitionCompletion)
            .consume()
        {
            return Err(retryable_space_transition_runtime_error(
                error_context,
                "injected transition completion failure",
            ));
        }
        observe_local_result(
            LocalWorkStep::SessionCompleteTransition,
            facade.complete_pending_space_transition(),
        )
        .await
        .map(|_| ())
        .map_err(|error| space_transition_error(error_context, error))
    }

    pub(super) async fn install_active_session(
        &self,
        factory: &ProductionSessionFactory,
    ) -> Result<(), EngineError> {
        let session_builder = {
            let mut runtime = self.runtime.lock().await;
            if runtime.session.is_some() {
                return Err(operation_unavailable_error());
            }
            if runtime.network.is_none() {
                runtime.network = Some(factory.build_network().await?);
            }
            runtime
                .network
                .as_ref()
                .ok_or_else(operation_unavailable_error)?
                .prepare_session()
        };
        let prepared = match factory.prepare(session_builder).await {
            Ok(prepared) => prepared,
            Err(error) => {
                if let Some(summary) = error.admission_recovery() {
                    *self
                        .admission_recovery
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(summary);
                }
                return Err(error);
            }
        };
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionActivation)
            .consume()
        {
            prepared.network_session.shutdown().await;
            cleanup_prepared_session(prepared.session, "activation_failed").await;
            return Err(session_runtime_error(
                "activate p2p session",
                "injected session activation failure",
            ));
        }
        let mut runtime = self.runtime.lock().await;
        let Some(network) = runtime.network.as_mut() else {
            drop(runtime);
            prepared.network_session.shutdown().await;
            cleanup_prepared_session(prepared.session, "network_unavailable").await;
            return Err(operation_unavailable_error());
        };
        let activation = network
            .activate_session(prepared.network_session)
            .await
            .map_err(|error| session_runtime_error("activate p2p session", error));
        if let Err(error) = activation {
            drop(runtime);
            cleanup_prepared_session(prepared.session, "activation_failed").await;
            return Err(error);
        }
        runtime.session = Some(prepared.session);
        Ok(())
    }

    /// 拆除会话前调用：替换会话完整可用之前，宿主收到的重新查询通知只能读到不可用错误。
    pub(super) fn hold_refresh_until_session_installed(&self) {
        let mut hold = self
            .refresh_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if hold.is_none() {
            *hold = self
                .configured_factory()
                .map(|factory| factory.events.hold_refresh());
        }
    }

    pub(super) fn release_refresh_hold(&self) {
        let hold = self
            .refresh_hold
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(hold);
    }

    pub(super) async fn install_new_session(
        &self,
        resume_space_activities: bool,
    ) -> Result<(), EngineError> {
        let factory = self
            .configured_factory()
            .ok_or_else(super::operation_unavailable_error)?;
        observe_local_result(
            LocalWorkStep::SessionPrepare,
            self.install_active_session(&factory),
        )
        .await?;
        let mut resume_space_activities = resume_space_activities;
        let facade = self.current_facade().await?;
        let pending_transition = match facade.has_pending_space_transition().await {
            Ok(pending) => pending,
            Err(error) => {
                let primary =
                    operation_error_with_code(1103, "inspect pending space transition", error);
                return Err(self.shutdown_after_failure(primary).await);
            }
        };
        if pending_transition {
            self.hold_refresh_until_session_installed();
            self.quiesce_network_session().await?;
            let session = self
                .runtime
                .lock()
                .await
                .session
                .take()
                .ok_or_else(operation_unavailable_error)?;
            session.shutdown(None).await.map_err(lifecycle_error)?;
            self.complete_pending_space_transition(&facade, "recover pending space transition")
                .await?;
            observe_local_result(
                LocalWorkStep::SessionPrepare,
                self.install_active_session(&factory),
            )
            .await?;
            resume_space_activities = true;
        }
        let facade = self.current_facade().await?;
        let unlocked = match observe_local_result(
            LocalWorkStep::SessionRecover,
            facade.recover_space_session(),
        )
        .await
        {
            Ok(recovered) => recovered.unlocked,
            // 缺少缓存口令是正常锁定；存储故障和损坏不能降级为恢复成功。
            Err(RecoverSpaceSessionError::KeyringMiss) if !resume_space_activities => false,
            Err(error @ RecoverSpaceSessionError::Unavailable(_)) => {
                let primary =
                    retryable_operation_error_with_code(1103, "recover local session", error);
                return Err(self.shutdown_after_failure(primary).await);
            }
            Err(error) => {
                let primary = operation_error_with_code(1103, "recover local session", error);
                return Err(self.shutdown_after_failure(primary).await);
            }
        };
        if resume_space_activities && !unlocked {
            let primary = operation_error_with_code(
                1103,
                "activate transitioned space session",
                "the transitioned space could not be unlocked",
            );
            return Err(self.shutdown_after_failure(primary).await);
        }
        self.operations.reopen();
        // 未完成切换、会话恢复都已结束，替换会话此时才可读。
        self.release_refresh_hold();
        Ok(())
    }
}
