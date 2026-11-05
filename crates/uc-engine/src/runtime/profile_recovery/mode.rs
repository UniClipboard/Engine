use super::*;

impl RecoverableRuntime {
    pub(crate) async fn start(
        config: EngineConfig,
        host: Arc<ReusableHost>,
        events: EventSender,
        progress: Arc<StartupProgressStore>,
        cleanup: Arc<StartupCleanupOwner>,
    ) -> Result<Self, EngineError> {
        // 声明在任何资料读写之前校验：导入恢复和恢复出厂都只按校验后的名字处理宿主文件。
        if let Err(error) = config.validated_host_configuration_files() {
            let error_kind = match error {
                HostConfigurationFileError::InvalidName => "invalid_name",
                HostConfigurationFileError::Reserved => "reserved_name",
                HostConfigurationFileError::Duplicate => "duplicate_name",
            };
            uc_warn!(
                error_kind = error_kind,
                "host configuration file declaration rejected"
            );
            // discarded-source[core-pure-validation]: the Core rule is fully classified by the variant logged above
            let error = EngineError::new(
                HOST_CONFIGURATION_FILES_INVALID_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            );
            return Err(finish_without_runtime_start(&host, &cleanup, error).await);
        }
        // 日志档位名称会进入诊断状态与导出清单，同样在任何资料读写之前校验。
        if config.validated_host_log_profile().is_err() {
            uc_warn!(
                error_kind = "invalid_label",
                "host log profile declaration rejected"
            );
            // discarded-source[core-pure-validation]: the Core rule has a single failure, fully classified above
            let error = EngineError::new(
                HOST_LOG_PROFILE_INVALID_CODE,
                EngineErrorCategory::InvalidInput,
                false,
            );
            return Err(finish_without_runtime_start(&host, &cleanup, error).await);
        }
        let paths = derive_app_paths(host.directories());
        let recovery = profile_key_recovery_store(&config, &paths, &host.capabilities());
        let mode = match start_profile_mode(
            &config,
            &host,
            &events,
            &progress,
            &recovery,
            Some(&cleanup),
        )
        .await
        {
            Ok(mode) => mode,
            Err(error) => return Err(finish_without_runtime_start(&host, &cleanup, error).await),
        };
        Ok(Self {
            mode: RwLock::new(mode),
            config,
            host,
            recovery,
            ready_summary_override: StdMutex::new(None),
            reconstruction: StdMutex::new(None),
            events,
        })
    }

    pub(crate) fn reconstruction(&self) -> Option<Arc<RecoveryBootstrap>> {
        self.reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn settle_reconstruction(&self) -> bool {
        let Some(bootstrap) = self.reconstruction() else {
            return true;
        };
        bootstrap.wait_for_construction().await;
        let owner = bootstrap.lock_construction().clone();
        if let Some(owner) = owner {
            if !owner.wait_recovery_backup().await
                || bootstrap.construction_interrupted.load(Ordering::Acquire)
                || !owner.settle_recovery_construction()
            {
                return false;
            }
            *bootstrap.lock_construction() = None;
        }
        if bootstrap.construction_interrupted.load(Ordering::Acquire) {
            return false;
        }
        let mut current = self
            .reconstruction
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current
            .as_ref()
            .is_some_and(|value| Arc::ptr_eq(value, &bootstrap))
        {
            *current = None;
        }
        true
    }

    pub(crate) async fn mode(&self) -> RuntimeMode {
        let mode = self.mode.read().await.clone();
        let RuntimeMode::Ready { runtime, .. } = &mode else {
            return mode;
        };
        match runtime.admission_recovery() {
            Some(summary) => {
                self.enter_admission_recovery(summary, Some(Arc::clone(runtime)))
                    .await
            }
            None => mode,
        }
    }

    /// 转入受限模式只发布一次；之后同一进程只允许查询与关闭，重建会话不被当作修复。
    pub(crate) async fn enter_admission_recovery(
        &self,
        summary: AdmissionRecoverySummary,
        runtime: Option<Arc<ProductionRuntime>>,
    ) -> RuntimeMode {
        let mut mode = self.mode.write().await;
        if !matches!(*mode, RuntimeMode::AdmissionRecovery { .. }) {
            *mode = RuntimeMode::AdmissionRecovery { summary, runtime };
            publish_profile_recovery(&self.events, admission_summary(summary));
        }
        mode.clone()
    }

    /// 启动后重建会话失败时，若已证实准入资料无法读取，改报需要恢复而不是可重试的会话错误。
    pub(crate) async fn restrict_after_failure<T>(
        &self,
        runtime: &Arc<ProductionRuntime>,
        result: Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        let Err(error) = result else {
            return result;
        };
        match runtime.admission_recovery() {
            Some(summary) => {
                self.enter_admission_recovery(summary, Some(Arc::clone(runtime)))
                    .await;
                Err(profile_recovery_required_error())
            }
            None => Err(error),
        }
    }

    pub(crate) fn require_restart(
        &self,
        mode: &mut RuntimeMode,
        runtime: Option<Arc<ProductionRuntime>>,
    ) -> EngineError {
        *mode = RuntimeMode::RestartRequired { runtime };
        publish_profile_recovery(&self.events, restart_required_summary());
        restart_required_error_for_reset()
    }

    /// 重建与恢复装配共用：宿主能力加上资料密钥恢复存储。
    pub(crate) fn recovery_host_capabilities(&self) -> HostCapabilities {
        let mut capabilities = self.host.capabilities();
        capabilities.replace_secure_storage(Arc::new(RecoveryHostStorage {
            inner: Arc::clone(&self.recovery),
        }));
        capabilities
    }

    pub(crate) fn publish_summary(
        &self,
        bootstrap: &RecoveryBootstrap,
        update: impl FnOnce(&mut ProfileRecoverySummary),
    ) {
        let summary = {
            let mut summary = bootstrap.lock_summary();
            update(&mut summary);
            summary.clone()
        };
        publish_profile_recovery(&self.events, summary);
    }

    pub(crate) fn publish_restart_required(&self, bootstrap: &RecoveryBootstrap) {
        self.publish_summary(bootstrap, |summary| {
            summary.state = ProfileRecoveryState::Failed;
            summary.can_submit_passphrase = false;
            summary.restart_required = true;
            summary.background_ready = false;
        });
    }

    pub(crate) fn lock_ready_summary_override(
        &self,
    ) -> StdMutexGuard<'_, Option<ProfileRecoverySummary>> {
        self.ready_summary_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
