//! 保留原接收owner已经取得的实际快照，只控制完整能力返回的时机，不伪造查询结果。
use super::{fixture_for, ReceiveExports};
use async_trait::async_trait;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use uc_application::deps::{
    ProgressPageReply, ProgressReceiveError, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
};
use uc_core::ids::DeviceId;

pub(crate) fn decorate_progress_queries(
    path: &Path,
    inner: Arc<dyn ProgressSnapshotProviderPort>,
) -> Arc<dyn ProgressSnapshotProviderPort> {
    match fixture_for(path) {
        Some(fixture) => Arc::new(ObservedQueries { inner, fixture }),
        None => inner,
    }
}
struct ObservedQueries {
    inner: Arc<dyn ProgressSnapshotProviderPort>,
    fixture: Arc<ReceiveExports>,
}
#[async_trait]
impl ProgressSnapshotProviderPort for ObservedQueries {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        let reply = self.inner.snapshot_page(peer, query).await?;
        if self
            .fixture
            .pause_snapshot_reply
            .swap(false, Ordering::SeqCst)
        {
            self.fixture.snapshot_held.notify_one();
            self.fixture
                .snapshot_release
                .acquire()
                .await
                .unwrap()
                .forget();
        }
        Ok(reply)
    }
}
