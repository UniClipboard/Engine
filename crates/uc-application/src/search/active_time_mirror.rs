//! 条目活跃时间到搜索索引的镜像。
//!
//! 条目表中的活跃时间是唯一事实来源，搜索索引另存一份副本用于浏览与搜索排序。
//! 重复复制、入站重新激活、恢复等动作都通过 [`TouchClipboardEntryPort`] 推进
//! 条目表；本装饰器在同一能力内把相同的值写入索引，调用方无需各自补写，
//! 也不会出现历史已重新浮出而列表顺序不变的情况。

use std::sync::Arc;

use async_trait::async_trait;
use uc_core::clipboard::ClipboardRepositoryError;
use uc_core::ids::EntryId;
use uc_core::ports::clipboard::TouchClipboardEntryPort;
use uc_core::ports::SearchIndexPort;

pub(crate) struct SearchMirroredTouch {
    inner: Arc<dyn TouchClipboardEntryPort>,
    search_index: Arc<dyn SearchIndexPort>,
}

impl SearchMirroredTouch {
    pub(crate) fn new(
        inner: Arc<dyn TouchClipboardEntryPort>,
        search_index: Arc<dyn SearchIndexPort>,
    ) -> Self {
        Self {
            inner,
            search_index,
        }
    }
}

#[async_trait]
impl TouchClipboardEntryPort for SearchMirroredTouch {
    async fn touch_entry(
        &self,
        entry_id: &EntryId,
        active_time_ms: i64,
    ) -> Result<bool, ClipboardRepositoryError> {
        let updated = self.inner.touch_entry(entry_id, active_time_ms).await?;

        // 条目不存在时索引无事可做。镜像失败不回滚已持久化的活跃时间：
        // 条目表仍是权威值，下一次重建会据此校正索引。
        if updated {
            if let Err(error) = self
                .search_index
                .set_entry_active_time(entry_id, active_time_ms)
                .await
            {
                tracing::warn!(
                    entry_id = %entry_id,
                    error = %error,
                    "active time persisted but search index mirror failed; rebuild will reconcile"
                );
            }
        }

        Ok(updated)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::mpsc::Sender;
    use uc_core::search::{
        RebuildProgress, SearchDocument, SearchError, SearchIndexMeta, SearchPosting, SearchQuery,
        SearchResultsPage,
    };

    use super::*;

    /// 按固定结果应答，并记录收到的调用。
    struct FakeTouch {
        result: Result<bool, ()>,
        calls: Mutex<Vec<(String, i64)>>,
    }

    impl FakeTouch {
        fn replying(result: Result<bool, ()>) -> Arc<Self> {
            Arc::new(Self {
                result,
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl TouchClipboardEntryPort for FakeTouch {
        async fn touch_entry(
            &self,
            entry_id: &EntryId,
            active_time_ms: i64,
        ) -> Result<bool, ClipboardRepositoryError> {
            self.calls
                .lock()
                .unwrap()
                .push((entry_id.to_string(), active_time_ms));
            self.result
                .map_err(|()| ClipboardRepositoryError::Storage("boom".into()))
        }
    }

    /// 只记录活跃时间镜像调用；本装饰器不使用其余能力。
    struct RecordingIndex {
        fail: bool,
        calls: Mutex<Vec<(String, i64)>>,
    }

    impl RecordingIndex {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                fail,
                calls: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait]
    impl SearchIndexPort for RecordingIndex {
        async fn index_entry(
            &self,
            _document: SearchDocument,
            _postings: Vec<SearchPosting>,
        ) -> Result<(), SearchError> {
            unreachable!("touch never re-indexes the entry")
        }
        async fn remove_entry(&self, _entry_id: &EntryId) -> Result<(), SearchError> {
            unreachable!("touch never removes the entry")
        }
        async fn search(&self, _query: SearchQuery) -> Result<SearchResultsPage, SearchError> {
            unreachable!("touch never queries the index")
        }
        async fn rebuild(
            &self,
            _entries: Vec<(SearchDocument, Vec<SearchPosting>)>,
            _progress_tx: Sender<RebuildProgress>,
        ) -> Result<(), SearchError> {
            unreachable!("touch never rebuilds the index")
        }
        async fn get_index_meta(&self) -> Result<SearchIndexMeta, SearchError> {
            unreachable!("touch never reads index meta")
        }
        async fn set_entry_active_time(
            &self,
            entry_id: &EntryId,
            active_time_ms: i64,
        ) -> Result<(), SearchError> {
            self.calls
                .lock()
                .unwrap()
                .push((entry_id.to_string(), active_time_ms));
            if self.fail {
                Err(SearchError::Internal("index unavailable".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn mirrors_the_persisted_active_time_into_the_index() {
        let touch = FakeTouch::replying(Ok(true));
        let index = RecordingIndex::new(false);
        let mirrored = SearchMirroredTouch::new(touch.clone(), index.clone());

        let updated = mirrored
            .touch_entry(&EntryId::from("entry-a"), 42)
            .await
            .unwrap();

        assert!(updated);
        assert_eq!(*touch.calls.lock().unwrap(), [("entry-a".to_owned(), 42)]);
        assert_eq!(*index.calls.lock().unwrap(), [("entry-a".to_owned(), 42)]);
    }

    #[tokio::test]
    async fn skips_the_index_when_no_entry_was_touched() {
        let touch = FakeTouch::replying(Ok(false));
        let index = RecordingIndex::new(false);
        let mirrored = SearchMirroredTouch::new(touch, index.clone());

        let updated = mirrored
            .touch_entry(&EntryId::from("missing"), 42)
            .await
            .unwrap();

        assert!(!updated);
        assert!(index.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn skips_the_index_and_keeps_the_error_when_the_touch_fails() {
        let touch = FakeTouch::replying(Err(()));
        let index = RecordingIndex::new(false);
        let mirrored = SearchMirroredTouch::new(touch, index.clone());

        let result = mirrored.touch_entry(&EntryId::from("entry-a"), 42).await;

        assert!(matches!(result, Err(ClipboardRepositoryError::Storage(_))));
        assert!(index.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn index_failure_does_not_fail_the_persisted_touch() {
        let touch = FakeTouch::replying(Ok(true));
        let index = RecordingIndex::new(true);
        let mirrored = SearchMirroredTouch::new(touch, index.clone());

        let updated = mirrored
            .touch_entry(&EntryId::from("entry-a"), 42)
            .await
            .unwrap();

        assert!(updated);
        assert_eq!(index.calls.lock().unwrap().len(), 1);
    }
}
