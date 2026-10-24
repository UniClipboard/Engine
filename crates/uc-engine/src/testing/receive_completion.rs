//! 真实接收最终索引能力的受控完成边界；仅匹配本次接收输出，不替换业务流程。
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::timeout;
use uc_core::ids::EntryId;
use uc_core::ports::search::SearchPipelinePort;
use uc_core::search::document::{SearchDocument, SearchPosting};
use uc_core::search::key::SearchKeyContext;
use uc_core::search::pipeline_input::SearchPipelineInput;

static FIXTURES: OnceLock<Mutex<HashMap<PathBuf, Weak<ReceiveCompletion>>>> = OnceLock::new();

pub(crate) struct ReceiveCompletion {
    indexed_entries: Mutex<HashSet<EntryId>>,
    index_paused: AtomicBool,
    index_entered: Notify,
    released: Mutex<bool>,
    release_changed: Condvar,
    pub checkpoint: Notify,
    pub sampled: Notify,
    pub sampled_while_held: AtomicBool,
    pub gate_timed_out: AtomicBool,
}

impl ReceiveCompletion {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            indexed_entries: Mutex::new(HashSet::new()),
            index_paused: AtomicBool::new(false),
            index_entered: Notify::new(),
            released: Mutex::new(false),
            release_changed: Condvar::new(),
            checkpoint: Notify::new(),
            sampled: Notify::new(),
            sampled_while_held: AtomicBool::new(false),
            gate_timed_out: AtomicBool::new(false),
        })
    }

    pub fn install(self: &Arc<Self>, root: &Path) {
        FIXTURES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .insert(root.to_owned(), Arc::downgrade(self));
    }

    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release_changed.notify_all();
    }

    fn pause_second_index(&self, input: &SearchPipelineInput) {
        let second_entry = {
            let mut entries = self.indexed_entries.lock().unwrap();
            entries.insert(input.entry_id.clone()) && entries.len() == 2
        };
        if !second_entry {
            return;
        }
        let released = self.released.lock().unwrap();
        self.index_paused.store(true, Ordering::SeqCst);
        self.index_entered.notify_one();
        // 同步能力只持有本次工作线程；失败路径与Drop均可唤醒，不能遗留阻塞工作。
        let (released, _) = self
            .release_changed
            .wait_timeout_while(released, Duration::from_secs(30), |released| !*released)
            .unwrap();
        self.gate_timed_out.store(!*released, Ordering::SeqCst);
        self.index_paused.store(false, Ordering::SeqCst);
    }

    pub async fn before_second_sample(&self) -> Result<()> {
        timeout(Duration::from_secs(30), async {
            while !self.index_paused.load(Ordering::SeqCst) {
                self.index_entered.notified().await;
            }
        })
        .await?;
        self.checkpoint.notify_one();
        Ok(())
    }

    pub fn record_second_sample(&self) {
        self.sampled_while_held
            .store(self.index_paused.load(Ordering::SeqCst), Ordering::SeqCst);
        self.sampled.notify_one();
    }
}

pub(crate) struct ReleaseCompletionOnDrop(pub Arc<ReceiveCompletion>);
impl Drop for ReleaseCompletionOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

pub(crate) fn decorate(inner: Arc<dyn SearchPipelinePort>) -> Arc<dyn SearchPipelinePort> {
    let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
    fixtures.retain(|_, fixture| fixture.strong_count() > 0);
    if fixtures.is_empty() {
        return inner;
    }
    Arc::new(ObservedPipeline { inner })
}

struct ObservedPipeline {
    inner: Arc<dyn SearchPipelinePort>,
}
impl SearchPipelinePort for ObservedPipeline {
    fn build_document(&self, input: &SearchPipelineInput) -> SearchDocument {
        self.inner.build_document(input)
    }

    fn build_postings(
        &self,
        input: &SearchPipelineInput,
        search_key: &SearchKeyContext,
    ) -> Result<Vec<SearchPosting>> {
        self.inner.build_postings(input, search_key)
    }

    fn build(
        &self,
        input: &SearchPipelineInput,
        search_key: &SearchKeyContext,
    ) -> Result<(SearchDocument, Vec<SearchPosting>)> {
        let result = self.inner.build(input, search_key)?;
        let fixture = {
            let mut fixtures = FIXTURES.get_or_init(Mutex::default).lock().unwrap();
            fixtures.retain(|_, fixture| fixture.strong_count() > 0);
            fixtures.iter().find_map(|(root, fixture)| {
                input
                    .file_paths
                    .iter()
                    .any(|path| Path::new(path).starts_with(root))
                    .then(|| fixture.upgrade())
                    .flatten()
            })
        };
        if let Some(fixture) = fixture {
            fixture.pause_second_index(input);
        }
        Ok(result)
    }
}
