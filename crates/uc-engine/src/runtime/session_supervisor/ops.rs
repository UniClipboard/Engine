use super::*;

impl SessionSupervisor {
    pub(in super::super) fn new(
        application: ApplicationAssembly,
        local_resources: Arc<ProfileLocalResources>,
    ) -> Arc<Self> {
        use uc_observability_contract::diagnostics::connectivity::{
            LocalDiagnosticSource, NetworkRecorder, SourceCapability, SourceCollection,
        };
        NetworkRecorder::current().register_source(
            LocalDiagnosticSource::Sessions,
            SourceCapability::Partial,
            SourceCollection::Enabled,
        );
        Arc::new_cyclic(|owner| Self {
            runtime: Mutex::new(SessionRuntimeState {
                network: None,
                session: None,
            }),
            factory: StdMutex::new(None),
            application: application.clone(),
            lifecycle: Mutex::new(()),
            operations: SessionOperationGate::new_open(),
            #[cfg(feature = "dev-tools")]
            test_control: Arc::new(SessionHandoverTestControl::default()),
            session_recovery_enabled: AtomicBool::new(false),
            admission_recovery: StdMutex::new(None),
            refresh_hold: StdMutex::new(None),
            coordinator: application
                .runtime_lifecycle(Arc::new(SessionWork(owner.clone())), local_resources),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(in super::super) fn configure_factory(
        &self,
        wired: WiredDependencies,
        #[cfg(feature = "lan-compat")] paths: uc_application::facade::AppPaths,
        app_version: String,
        events: EventSender,
        rendezvous_base_url: Option<String>,
        relay_fallback_override: Option<bool>,
        iroh_bind_port_override: Option<u16>,
        #[cfg(feature = "dev-tools")]
        network_partition_gate: uc_infra_p2p::network::iroh::IrohNetworkPartitionGate,
        #[cfg(feature = "dev-tools")] joiner_final_confirmation_gate: Arc<
            JoinerFinalConfirmationGate,
        >,
        network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
    ) {
        let factory = Arc::new(ProductionSessionFactory {
            wired,
            #[cfg(feature = "lan-compat")]
            paths,
            app_version,
            events,
            rendezvous_base_url,
            relay_fallback_override,
            iroh_bind_port_override,
            #[cfg(feature = "dev-tools")]
            network_partition_gate,
            #[cfg(feature = "dev-tools")]
            joiner_final_confirmation_gate,
            #[cfg(feature = "dev-tools")]
            test_control: Arc::clone(&self.test_control),
            network_recovery,
        });
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(factory);
    }

    pub(in super::super) async fn current_facade(&self) -> Result<Arc<AppFacade>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.facade))
            .ok_or_else(operation_unavailable_error)
    }

    pub(in super::super) async fn current_application(
        &self,
    ) -> Result<Arc<ApplicationRuntime>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.application))
            .ok_or_else(operation_unavailable_error)
    }

    pub(in super::super) async fn current_facade_and_application(
        &self,
    ) -> Result<(Arc<AppFacade>, Arc<ApplicationRuntime>), EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| {
                (
                    Arc::clone(&session.facade),
                    Arc::clone(&session.application),
                )
            })
            .ok_or_else(operation_unavailable_error)
    }

    #[cfg(feature = "lan-compat")]
    pub(in super::super) async fn current_mobile_sync(
        &self,
    ) -> Result<Arc<uc_mobile_lan::MobileSyncFacade>, EngineError> {
        self.runtime
            .lock()
            .await
            .session
            .as_ref()
            .map(|session| Arc::clone(&session.mobile_sync))
            .ok_or_else(operation_unavailable_error)
    }

    pub(in super::super) fn clear_factory(&self) {
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    pub(in super::super) async fn acquire_operation(
        &self,
    ) -> Result<SessionOperationLease, EngineError> {
        self.operations.acquire()
    }

    #[cfg(feature = "dev-tools")]
    pub(in super::super) fn fail_next_session_handover(&self, point: SessionHandoverFailurePoint) {
        self.test_control.failure(point).arm();
    }

    #[cfg(feature = "dev-tools")]
    pub(in super::super) fn session_handover_diagnostics(&self) -> SessionHandoverDiagnostics {
        SessionHandoverDiagnostics {
            network_build_count: self.test_control.network_build_count.load(Ordering::SeqCst),
            session_quiesce_failure_count: self.test_control.session_quiesce.consumed(),
            transition_completion_failure_count: self.test_control.transition_completion.consumed(),
            session_preparation_failure_count: self.test_control.session_preparation.consumed(),
            session_activation_failure_count: self.test_control.session_activation.consumed(),
        }
    }

    pub(in super::super) async fn transition_pending_session(
        &self,
    ) -> Result<Option<u64>, EngineError> {
        let waiting = Instant::now();
        let _lifecycle = self.lifecycle.lock().await;
        let waited = waiting.elapsed();
        let (facade, network_active) = {
            let runtime = self.runtime.lock().await;
            (
                runtime
                    .session
                    .as_ref()
                    .map(|session| Arc::clone(&session.facade)),
                runtime.network.is_some(),
            )
        };
        let facade = match facade {
            Some(facade) => facade,
            None if network_active => {
                record_session_lock_wait(waited);
                self.install_new_session(false).await?;
                let revision = self
                    .current_facade()
                    .await?
                    .query_device_group_choices()
                    .await
                    .map_err(|error| {
                        operation_error_with_code(
                            1103,
                            "query recovered device trust revision",
                            error,
                        )
                    })?
                    .revision;
                return Ok(Some(revision));
            }
            None => return Ok(None),
        };
        match facade.has_pending_space_transition().await {
            Ok(false) => return Ok(None),
            Ok(true) => {}
            Err(error) => {
                let error =
                    operation_error_with_code(1103, "inspect runtime space transition", error);
                return observe_session_lifecycle(async {
                    record_session_lock_wait(waited);
                    Err(error)
                })
                .await;
            }
        }
        observe_session_lifecycle(async {
            record_session_lock_wait(waited);
            self.operations.close_and_wait(None, None).await?;
            match facade.has_pending_space_transition().await {
                Ok(true) => {}
                Ok(false) => {
                    self.operations.reopen();
                    return Ok(None);
                }
                Err(error) => {
                    self.operations.reopen();
                    return Err(operation_error_with_code(
                        1103,
                        "confirm runtime space transition",
                        error,
                    ));
                }
            }

            self.hold_refresh_until_session_installed();
            self.quiesce_network_session().await?;
            let session = self
                .runtime
                .lock()
                .await
                .session
                .take()
                .ok_or_else(super::operation_unavailable_error)?;
            session.shutdown(None).await.map_err(lifecycle_error)?;
            let completed = self
                .complete_pending_space_transition(&facade, "complete runtime space transition")
                .await;
            match completed {
                Ok(_) => {
                    self.install_new_session(true).await?;
                    let revision = self
                        .current_facade()
                        .await?
                        .query_device_group_choices()
                        .await
                        .map_err(|error| {
                            operation_error_with_code(
                                1103,
                                "query transitioned device trust revision",
                                error,
                            )
                        })?
                        .revision;
                    Ok(Some(revision))
                }
                Err(error) => match self.install_new_session(false).await {
                    Ok(()) => Err(error),
                    Err(restore_error) => Err(restore_error),
                },
            }
        })
        .await
    }

    pub(in super::super) async fn reset_space(
        &self,
        current_operation: SessionOperationLease,
    ) -> Result<OperationResult, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations
            .close_and_wait(Some(current_operation), None)
            .await?;
        self.quiesce_network_session().await?;
        let session = self
            .runtime
            .lock()
            .await
            .session
            .take()
            .ok_or_else(super::operation_unavailable_error)?;
        let facade = Arc::clone(&session.facade);
        session.shutdown(None).await.map_err(lifecycle_error)?;
        let transfer_result = self
            .application
            .cancel_active_file_transfers(
                uc_core::FileTransferCancellationReason::ConnectivityRecovery,
            )
            .await
            .map_err(|error| {
                operation_error_with_code(1104, "cancel active file transfers", error)
            });
        if let Err(error) = transfer_result {
            return match self.install_new_session(false).await {
                Ok(()) => Err(error),
                Err(install_error) => Err(install_error),
            };
        }

        let result = execute_reset_space(&facade).await;
        let install_result = self.install_new_session(result.is_ok()).await;
        match (result, install_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), Ok(())) => {
                let facade = self
                    .runtime
                    .lock()
                    .await
                    .session
                    .as_ref()
                    .map(|session| Arc::clone(&session.facade))
                    .ok_or_else(super::operation_unavailable_error)?;
                match facade.has_committed_device_management_reset().await {
                    Ok(true) => execute_reset_space(&facade).await,
                    Ok(false) | Err(_) => Err(error),
                }
            }
            (_, Err(error)) => Err(error),
        }
    }

    pub(in super::super) async fn suspend(
        &self,
        deadline: Option<Instant>,
    ) -> Result<(), EngineError> {
        observe_session_request(SessionTransition::Suspend, async {
            self.coordinator
                .suspend(deadline)
                .await
                .map_err(lifecycle_error)?;
            Ok(SessionTransitionResult::Completed)
        })
        .await
    }

    pub(in super::super) async fn resume(
        &self,
        cancellation: CancellationToken,
    ) -> Result<(), EngineError> {
        observe_session_request(SessionTransition::Resume, async {
            self.coordinator
                .resume(None, cancellation)
                .await
                .map_err(lifecycle_error)?;
            Ok(SessionTransitionResult::Completed)
        })
        .await
    }

    pub(in super::super) async fn close_file_transfers(&self) -> Result<(), EngineError> {
        self.application
            .close_file_transfers()
            .await
            .map_err(|error| operation_error_with_code(1104, "close file transfers", error))
    }

    pub(in super::super) async fn stop(
        &self,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        self.coordinator.stop(deadline).await
    }

    pub(in super::super) fn admission_recovery(&self) -> Option<AdmissionRecoverySummary> {
        *self
            .admission_recovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
