use super::*;

impl ProductionSessionFactory {
    pub(super) async fn build_network(&self) -> Result<IrohNode, EngineError> {
        #[cfg(feature = "dev-tools")]
        self.test_control
            .network_build_count
            .fetch_add(1, Ordering::SeqCst);
        build_network_runtime(
            &self.wired.application,
            &self.wired.sync_engine,
            self.rendezvous_base_url.clone(),
            self.relay_fallback_override,
            self.iroh_bind_port_override,
            #[cfg(feature = "dev-tools")]
            Some(self.network_partition_gate.clone()),
            #[cfg(not(feature = "dev-tools"))]
            None,
        )
        .await
        .map_err(|error| session_runtime_error("p2p network", error))
    }

    pub(super) async fn prepare(
        &self,
        session_builder: IrohSessionBuilder,
    ) -> Result<PreparedProductionSession, EngineError> {
        #[cfg(feature = "dev-tools")]
        if self
            .test_control
            .failure(SessionHandoverFailurePoint::SessionPreparation)
            .consume()
        {
            return Err(session_runtime_error(
                "p2p session",
                "injected session preparation failure",
            ));
        }
        let wired = &self.wired;
        #[cfg(feature = "lan-compat")]
        let paths = &self.paths;
        let events = self.events.clone();
        let application = wired.application.clone();
        let prepared = prepare_daemon_session(
            &application,
            &wired.sync_engine,
            &self.app_version,
            #[cfg(feature = "lan-compat")]
            wired.mobile_sync_ports.clone(),
            #[cfg(feature = "dev-tools")]
            Arc::clone(&self.joiner_final_confirmation_gate),
            session_builder,
        )
        .await
        .map_err(|error| session_runtime_error("p2p session", error))?;
        let network_adapters = prepared.application;
        let prepared_session = prepared.prepared_session;
        let application_runtime = match observe_local_result(
            LocalWorkStep::SessionStart,
            ApplicationRuntime::start(
                &application,
                network_adapters.binding.complete(
                    network_adapters.active_pull_client,
                    Arc::clone(&self.network_recovery),
                    FsAtomicPublisher::new(),
                    FsInboundFileTarget::new(Arc::clone(&wired.sync_engine.settings)),
                    FsHiddenPathMarker::new(),
                    Arc::new(EngineClipboardInboundEvents {
                        events: events.clone(),
                    }),
                ),
            ),
        )
        .await
        {
            Ok(runtime) => Arc::new(runtime),
            Err(error) => {
                let primary = match error.admission_failure() {
                    Some(category) => {
                        let summary = AdmissionRecoverySummary::from(category);
                        uc_error!(
                            category = log_vocab_debug(&summary.category),
                            stage = log_vocab_debug(&summary.stage),
                            "application runtime requires admission recovery"
                        );
                        profile_recovery_required_error().with_admission_recovery(summary)
                    }
                    None => session_runtime_error("application runtime", error),
                };
                prepared_session.shutdown().await;
                return Err(lifecycle_error(LifecycleError {
                    primary: primary.into(),
                    additional: Vec::new(),
                }));
            }
        };
        reconcile_session_peers(&wired.sync_engine).await;
        let facade = application_runtime.facade();
        #[cfg(feature = "lan-compat")]
        let mobile_sync = build_mobile_sync_facade(
            &wired.mobile_sync_application,
            paths,
            wired.mobile_sync_ports.clone(),
            application_runtime.inbound_clipboard(),
            Some(facade.file_transfer_for_lan_compatibility()),
            None,
            Some(facade.clipboard_outbound_for_lan_compatibility()),
            Some(facade.active_clipboard_for_lan_compatibility()),
        );
        let tasks = Arc::new(TaskRegistry::new());
        let mut active_clipboard_changes = wired.shared.active_clipboard_sse_source.subscribe();
        let active_clipboard_events = events.clone();
        let _ = tasks
            .spawn(move |cancel| async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return,
                        change = active_clipboard_changes.recv() => match change {
                            Ok(state) => active_clipboard_events
                                .send(engine_event_for_active_clipboard(&state)),
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                active_clipboard_events.send(EngineEvent::RefreshRequired {
                                    reason: RefreshReason::ConsumerLagged,
                                });
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                        }
                    }
                }
            })
            .await;
        spawn_peer_reachability_event_task(Arc::clone(&facade), &tasks, events).await;
        Ok(PreparedProductionSession {
            session: ProductionSession {
                facade,
                application: application_runtime,
                #[cfg(feature = "lan-compat")]
                mobile_sync,
                tasks,
            },
            network_session: prepared_session,
        })
    }
}

pub(super) fn engine_event_for_active_clipboard(
    state: &uc_core::clipboard::ActiveClipboardState,
) -> EngineEvent {
    EngineEvent::ActiveClipboardChanged(ActiveClipboardChanged {
        snapshot_hash: state.snapshot_hash.clone(),
        entry_id: state.entry_id.as_str().to_string(),
        activated_at_ms: state.activated_at_ms,
        activated_by: state.activated_by.as_str().to_string(),
    })
}

struct EngineClipboardInboundEvents {
    events: EventSender,
}

impl ClipboardInboundEventPort for EngineClipboardInboundEvents {
    fn emit(&self, event: ClipboardInboundEvent) {
        self.events
            .send(EngineEvent::InboundNotice(InboundNoticeEvent {
                from_device: event.from_device.as_str().to_owned(),
                snapshot_hash: event.snapshot_hash,
                text_preview: event.text_preview,
                representations: event
                    .representations
                    .into_iter()
                    .map(|representation| InboundRepresentationSummary {
                        mime_type: representation.mime_type,
                        size_bytes: representation.size_bytes,
                    })
                    .collect(),
                action: match event.action {
                    ClipboardInboundEventAction::NewEntry => InboundNoticeActionSummary::NewEntry,
                    ClipboardInboundEventAction::DuplicateIgnored => {
                        InboundNoticeActionSummary::DuplicateIgnored
                    }
                },
                at_ms: event.at_ms,
            }));
    }
}
