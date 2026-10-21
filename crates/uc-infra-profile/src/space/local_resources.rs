//! Profile 的本地资源交接：后台安全资料与运行期数据库连接作为同一项本地资源暂停和恢复。
//!
//! 暂停先撤销安全会话与内容密钥租约，再关闭 profile 与 control 数据库的全部连接；恢复顺序相反，
//! 因为安全会话恢复时需要读取 control 数据库。暂停成功后本进程不再持有这些数据库文件及其 SQLite 锁。
//! 最终停止不关闭数据库：恢复出厂等负责人在停止运行期后继续清理同一数据库，连接随其所有者一起释放。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use uc_application::deps::{RuntimeLifecyclePort, TransitionContext};
use uc_infra_storage::db::pool::DbPool;

use super::RuntimeSpaceAccessAdapter;

pub struct ProfileLocalResources {
    security: Arc<RuntimeSpaceAccessAdapter>,
    databases: Vec<DbPool>,
}

impl ProfileLocalResources {
    /// `databases` 可以包含同一个连接池的多个克隆（旧布局中 profile 与 control 共用一个库）。
    pub fn new(security: Arc<RuntimeSpaceAccessAdapter>, databases: Vec<DbPool>) -> Self {
        Self {
            security,
            databases,
        }
    }
}

/// 等待方离开（共同期限到达）时通知阻塞线程停止等待；旧连接池保留给后续暂停继续等待。
struct StopWaitingOnDrop(Arc<AtomicBool>);

impl Drop for StopWaitingOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait]
impl RuntimeLifecyclePort for ProfileLocalResources {
    async fn suspend(&self, context: &TransitionContext) -> anyhow::Result<()> {
        self.security.suspend(context).await?;
        if context.is_final_stop() {
            return Ok(());
        }
        for database in &self.databases {
            let database = database.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            let _stop_waiting = StopWaitingOnDrop(Arc::clone(&cancelled));
            // 等待借出连接归还会阻塞线程；移动绑定使用单线程运行期，不能在运行线程上等待。
            let closed =
                tokio::task::spawn_blocking(move || database.suspend_connections(&cancelled))
                    .await
                    .context("close database connections")?;
            anyhow::ensure!(closed, "database connections are still in use");
        }
        Ok(())
    }

    async fn resume(&self, context: &TransitionContext) -> anyhow::Result<()> {
        for database in &self.databases {
            let database = database.clone();
            tokio::task::spawn_blocking(move || database.resume_connections())
                .await
                .context("reopen database connections")??;
        }
        self.security.resume(context).await
    }
}
