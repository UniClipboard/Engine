use super::*;

pub(crate) async fn finish_without_runtime_start(
    host: &Arc<ReusableHost>,
    cleanup: &Arc<StartupCleanupOwner>,
    primary: EngineError,
) -> EngineError {
    cleanup.record_primary(primary.clone());
    if cleanup.rollback_unconfirmed() {
        return primary;
    }
    let waiting = cleanup.start_host_close(Arc::clone(host), primary.clone());
    // 关闭任务属于启动负责人；此处仅等待其完成。
    let _ = waiting.await;
    primary
}

/// 按资料当前状态决定运行模式：正常装配运行期，或进入口令恢复/准入恢复。
/// 进程启动与离开空间后的重建使用同一条路径。
pub(crate) async fn start_profile_mode(
    config: &EngineConfig,
    host: &ReusableHost,
    events: &EventSender,
    progress: &Arc<StartupProgressStore>,
    recovery: &Arc<ProfileKeyRecoveryStore>,
    cleanup: Option<&Arc<StartupCleanupOwner>>,
) -> Result<RuntimeMode, EngineError> {
    let paths = derive_app_paths(host.directories());
    let _startup_unlock_scope = recovery.startup_unlock_scope();
    match recovery.prepare_startup().await? {
        ProfileRecoveryPreparation::Ready => {
            let mut capabilities = host.capabilities();
            capabilities.replace_secure_storage(Arc::new(RecoveryHostStorage {
                inner: Arc::clone(recovery),
            }));
            match ProductionRuntime::start(
                config.clone(),
                capabilities,
                paths,
                events.clone(),
                Arc::clone(progress),
                Arc::clone(recovery),
                cleanup,
            )
            .await
            {
                Ok(runtime) => Ok(RuntimeMode::Ready {
                    runtime: Arc::new(runtime),
                    recovered: false,
                }),
                Err(error) if cleanup.is_some_and(|owner| owner.rollback_unconfirmed()) => {
                    Err(error)
                }
                Err(error) => match error.admission_recovery() {
                    Some(summary) => {
                        progress.recovery_available();
                        publish_profile_recovery(events, admission_summary(summary));
                        Ok(RuntimeMode::AdmissionRecovery {
                            summary,
                            runtime: None,
                        })
                    }
                    None => Err(error),
                },
            }
        }
        ProfileRecoveryPreparation::AwaitingPassphrase { losses } => {
            progress.recovery_available();
            let summary = ProfileRecoverySummary {
                state: if !losses.is_empty() {
                    ProfileRecoveryState::PartiallyRecoverable
                } else {
                    ProfileRecoveryState::AwaitingPassphrase
                },
                can_submit_passphrase: losses.is_empty(),
                restart_required: false,
                background_ready: false,
                cleanup_pending: false,
                losses: public_losses(losses),
                admission: None,
            };
            publish_profile_recovery(events, summary.clone());
            Ok(RuntimeMode::Recovery(Arc::new(RecoveryBootstrap::new(
                Arc::clone(progress),
                summary,
                true,
            ))))
        }
    }
}

/// 消费一次启动输入；已被消费时返回 `false`。
pub(crate) async fn take_startup_input(bootstrap: &RecoveryBootstrap) -> bool {
    std::mem::replace(&mut *bootstrap.input_available.lock().await, false)
}

pub(crate) fn restart_required_summary() -> ProfileRecoverySummary {
    ProfileRecoverySummary {
        state: ProfileRecoveryState::Failed,
        can_submit_passphrase: false,
        restart_required: true,
        background_ready: false,
        cleanup_pending: false,
        losses: Vec::new(),
        admission: None,
    }
}

/// 离开空间的资料已清除，但同一实例无法继续服务；重试同一请求不能解决，宿主须重启 Engine。
pub(crate) fn restart_required_error_for_reset() -> EngineError {
    EngineError::new(
        FACTORY_RESET_RESTART_REQUIRED_CODE,
        EngineErrorCategory::Unavailable,
        false,
    )
}

/// 资料恢复状态机的每次转换：同一处发布事件并留下时间线，只含状态枚举与两个布尔位。
pub(crate) fn publish_profile_recovery(events: &EventSender, summary: ProfileRecoverySummary) {
    uc_info!(
        recovery_state = log_vocab_debug(&summary.state),
        restart_required = summary.restart_required,
        can_submit_passphrase = summary.can_submit_passphrase,
        "profile recovery state changed"
    );
    events.send(EngineEvent::ProfileRecoveryChanged(summary));
}
