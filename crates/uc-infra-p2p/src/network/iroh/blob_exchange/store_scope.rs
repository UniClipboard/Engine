//! 固定SDK的事务actor运行在独立runtime；请求parent不能跨subscriber解释。
use std::future::IntoFuture;
use tracing::{instrument::WithSubscriber, Dispatch};

/// SDK future的创建和驱动一起隔离；外层领域结果、业务关联及错误来源保持原责任。
pub(in super::super) async fn without_request_span<Make, Work>(make: Make) -> Work::Output
where
    Make: FnOnce() -> Work,
    Work: IntoFuture,
{
    async move { make().await }
        .with_subscriber(Dispatch::none())
        .await
}
