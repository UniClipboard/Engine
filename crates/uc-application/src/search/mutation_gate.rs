use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};
use uc_core::ids::EntryId;
use uc_core::ports::SearchIndexPort;
use uc_core::search::tag::SearchTagCount;
use uc_core::search::{
    RebuildProgress, SearchDocument, SearchError, SearchIndexMeta, SearchPosting, SearchQuery,
    SearchResultsPage,
};

pub(crate) struct SearchMutationGate {
    inner: Arc<RwLock<()>>,
}

impl SearchMutationGate {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(())),
        }
    }

    pub(crate) async fn begin_update(&self) -> OwnedRwLockReadGuard<()> {
        Arc::clone(&self.inner).read_owned().await
    }

    pub(crate) async fn begin_rebuild(&self) -> OwnedRwLockWriteGuard<()> {
        Arc::clone(&self.inner).write_owned().await
    }
}

pub(crate) struct CoordinatedSearchIndex {
    inner: Arc<dyn SearchIndexPort>,
    gate: Arc<SearchMutationGate>,
}

impl CoordinatedSearchIndex {
    pub(crate) fn new(inner: Arc<dyn SearchIndexPort>, gate: Arc<SearchMutationGate>) -> Self {
        Self { inner, gate }
    }
}

#[async_trait]
impl SearchIndexPort for CoordinatedSearchIndex {
    async fn index_entry(
        &self,
        document: SearchDocument,
        postings: Vec<SearchPosting>,
    ) -> Result<(), SearchError> {
        let _guard = self.gate.begin_update().await;
        self.inner.index_entry(document, postings).await
    }

    async fn remove_entry(&self, entry_id: &EntryId) -> Result<(), SearchError> {
        let _guard = self.gate.begin_update().await;
        self.inner.remove_entry(entry_id).await
    }

    async fn search(&self, query: SearchQuery) -> Result<SearchResultsPage, SearchError> {
        self.inner.search(query).await
    }

    async fn rebuild(
        &self,
        entries: Vec<(SearchDocument, Vec<SearchPosting>)>,
        progress_tx: tokio::sync::mpsc::Sender<RebuildProgress>,
    ) -> Result<(), SearchError> {
        let _guard = self.gate.begin_rebuild().await;
        self.inner.rebuild(entries, progress_tx).await
    }

    async fn get_index_meta(&self) -> Result<SearchIndexMeta, SearchError> {
        self.inner.get_index_meta().await
    }

    async fn set_entry_favorite_tag(
        &self,
        entry_id: &EntryId,
        favorited: bool,
    ) -> Result<(), SearchError> {
        let _guard = self.gate.begin_update().await;
        self.inner.set_entry_favorite_tag(entry_id, favorited).await
    }

    async fn list_tags(&self) -> Result<Vec<SearchTagCount>, SearchError> {
        self.inner.list_tags().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::SearchMutationGate;

    #[tokio::test]
    async fn live_update_waits_until_the_rebuild_snapshot_and_replacement_complete() {
        let gate = Arc::new(SearchMutationGate::new());
        let rebuild = gate.begin_rebuild().await;
        let (completed_tx, mut completed_rx) = tokio::sync::mpsc::channel(1);
        let update_gate = Arc::clone(&gate);
        let update = tokio::spawn(async move {
            let _guard = update_gate.begin_update().await;
            completed_tx.send(()).await.unwrap();
        });

        tokio::task::yield_now().await;
        assert!(completed_rx.try_recv().is_err());

        drop(rebuild);
        tokio::time::timeout(Duration::from_secs(1), completed_rx.recv())
            .await
            .unwrap()
            .unwrap();
        update.await.unwrap();
    }
}
