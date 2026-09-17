use tokio::time::Instant;

use super::super::ApplicationRuntimeOwners;
use super::{begin, finish, LifecycleError};

impl ApplicationRuntimeOwners {
    pub(in super::super) async fn shutdown(
        self,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let Self {
            history_maintenance,
            file_transfer_timeout,
            search,
            space,
            clipboard,
            active_clipboard,
        } = self;
        // 这里仅停止各领域的工作；共享磁盘和密钥资源仍由上级在全部结束后交接。
        let tasks = vec![
            begin("stop history maintenance", async move {
                history_maintenance.shutdown().await
            }),
            begin("stop file transfer timeout worker", async move {
                file_transfer_timeout.shutdown(deadline).await
            }),
            begin(
                "stop search runtime",
                async move { search.shutdown().await },
            ),
            begin("stop clipboard sync runtime", async move {
                clipboard.shutdown().await
            }),
            begin("stop active clipboard workers", async move {
                active_clipboard.shutdown().await
            }),
            begin(
                "stop space runtime",
                async move { space.on_shutdown().await },
            ),
        ];
        finish(tasks).await
    }
}
