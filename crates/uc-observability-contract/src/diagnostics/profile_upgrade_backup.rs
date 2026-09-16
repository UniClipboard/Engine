pub const LOCAL_DIAGNOSTIC_TARGET: &str = "uc.local_diagnostic";

/// 记录本地配置升级备份失败的稳定分类，不携带路径或原始错误正文。
pub fn record_profile_upgrade_backup_failure(
    backup_action: &'static str,
    error_kind: &'static str,
    io_error_kind: Option<&str>,
    io_error_code: Option<i32>,
) {
    tracing::event!(
        target: "uc.local_diagnostic",
        tracing::Level::ERROR,
        event.name = "profile_upgrade.backup.failed",
        backup_action,
        error_kind,
        io_error_kind,
        io_error_code,
        retryable = true,
    );
}
