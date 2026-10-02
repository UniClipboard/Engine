//! 宿主交给 Engine 的能力在一次进程内可能服务多个资料运行期（离开空间后会重建运行期）。
//!
//! `HostCapabilities` 的剪贴板与变化流只能被一次装配消费，所以这里持有宿主能力本体，
//! 每次装配只借出一份视图；变化流是宿主级资源，运行期结束时归还而不是关闭，
//! 只有 Engine 最终关闭时才真正关闭它。

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use async_trait::async_trait;

use crate::contract::HostAnalyticsCapabilities;
use crate::{
    HostCapabilities, HostCapabilityError, HostClipboard, HostClipboardChange,
    HostClipboardChangeStream, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostSecureStorage,
};

pub(crate) struct ReusableHost {
    directories: HostDirectories,
    secure_storage: Arc<dyn HostSecureStorage>,
    clipboard: Arc<SharedClipboard>,
    files: Arc<dyn HostFileAccess>,
    analytics: HostAnalyticsCapabilities,
}

impl ReusableHost {
    pub(crate) fn new(host: HostCapabilities) -> Self {
        let (directories, secure_storage, clipboard, files, analytics) = host.into_parts();
        Self {
            directories,
            secure_storage,
            clipboard: Arc::new(SharedClipboard {
                inner: RwLock::new(clipboard),
                stream: Mutex::new(StreamSlot::NotTaken),
            }),
            files,
            analytics,
        }
    }

    pub(crate) fn directories(&self) -> &HostDirectories {
        &self.directories
    }

    /// 为一次运行期装配生成宿主能力；同一时刻只应有一个运行期使用它。
    pub(crate) fn capabilities(&self) -> HostCapabilities {
        HostCapabilities::from_shared_parts(
            self.directories.clone(),
            Arc::clone(&self.secure_storage),
            Box::new(ClipboardView(Arc::clone(&self.clipboard))),
            Arc::clone(&self.files),
            self.analytics.clone(),
        )
    }

    /// Engine 最终关闭：关闭未被运行期占用的变化流。
    pub(crate) async fn close_change_stream(&self) -> Result<(), HostCapabilityError> {
        let idle = {
            let mut slot = self.clipboard.lock_stream();
            match std::mem::replace(&mut *slot, StreamSlot::Absent) {
                StreamSlot::Idle(stream) => Some(stream),
                other => {
                    *slot = other;
                    None
                }
            }
        };
        match idle {
            Some(mut stream) => stream.shutdown().await,
            None => Ok(()),
        }
    }
}

enum StreamSlot {
    NotTaken,
    Idle(Box<dyn HostClipboardChangeStream>),
    Lent,
    Absent,
}

struct SharedClipboard {
    inner: RwLock<Box<dyn HostClipboard>>,
    stream: Mutex<StreamSlot>,
}

impl SharedClipboard {
    fn lock_stream(&self) -> std::sync::MutexGuard<'_, StreamSlot> {
        self.stream.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn give_back(&self, stream: Box<dyn HostClipboardChangeStream>) {
        *self.lock_stream() = StreamSlot::Idle(stream);
    }
}

struct ClipboardView(Arc<SharedClipboard>);

impl HostClipboard for ClipboardView {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.0
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .read()
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.0
            .inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .write(snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        let mut slot = self.0.lock_stream();
        match std::mem::replace(&mut *slot, StreamSlot::Lent) {
            StreamSlot::NotTaken => {
                let taken = self
                    .0
                    .inner
                    .write()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take_change_stream();
                match taken {
                    Ok(Some(stream)) => Ok(Some(self.lend(stream))),
                    Ok(None) => {
                        *slot = StreamSlot::Absent;
                        Ok(None)
                    }
                    Err(error) => {
                        *slot = StreamSlot::NotTaken;
                        Err(error)
                    }
                }
            }
            StreamSlot::Idle(stream) => Ok(Some(self.lend(stream))),
            previous @ (StreamSlot::Lent | StreamSlot::Absent) => {
                *slot = previous;
                Ok(None)
            }
        }
    }
}

impl ClipboardView {
    fn lend(
        &self,
        stream: Box<dyn HostClipboardChangeStream>,
    ) -> Box<dyn HostClipboardChangeStream> {
        Box::new(LentChangeStream {
            stream: Some(stream),
            home: Arc::clone(&self.0),
        })
    }
}

/// 运行期持有的变化流：关闭或丢弃都只是归还给宿主级槽位。
struct LentChangeStream {
    stream: Option<Box<dyn HostClipboardChangeStream>>,
    home: Arc<SharedClipboard>,
}

#[async_trait]
impl HostClipboardChangeStream for LentChangeStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        match self.stream.as_mut() {
            Some(stream) => stream.next().await,
            None => Ok(HostClipboardChange::Closed),
        }
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        if let Some(stream) = self.stream.take() {
            self.home.give_back(stream);
        }
        Ok(())
    }
}

impl Drop for LentChangeStream {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.home.give_back(stream);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::HostFileHandle;

    #[derive(Default)]
    struct Counters {
        taken: AtomicUsize,
        shut_down: AtomicUsize,
    }

    struct OneShotClipboard {
        counters: Arc<Counters>,
        taken: bool,
    }

    impl HostClipboard for OneShotClipboard {
        fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
            Ok(HostClipboardSnapshot {
                observed_at_ms: 1,
                representations: Vec::new(),
            })
        }

        fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
            Ok(())
        }

        fn take_change_stream(
            &mut self,
        ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
            if std::mem::replace(&mut self.taken, true) {
                return Ok(None);
            }
            self.counters.taken.fetch_add(1, Ordering::SeqCst);
            Ok(Some(Box::new(CountingStream(Arc::clone(&self.counters)))))
        }
    }

    struct CountingStream(Arc<Counters>);

    #[async_trait]
    impl HostClipboardChangeStream for CountingStream {
        async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
            Ok(HostClipboardChange::Changed)
        }

        async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
            self.0.shut_down.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct NoFiles;

    impl HostFileAccess for NoFiles {
        fn metadata(
            &self,
            _handle: &HostFileHandle,
        ) -> Result<crate::HostFileMetadata, HostCapabilityError> {
            Err(HostCapabilityError::new(
                crate::HostCapabilityErrorCategory::Unavailable,
                "unused",
            ))
        }

        fn read_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _max_bytes: u32,
        ) -> Result<Vec<u8>, HostCapabilityError> {
            Ok(Vec::new())
        }

        fn write_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _bytes: &[u8],
        ) -> Result<(), HostCapabilityError> {
            Ok(())
        }

        fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
            Ok(())
        }
    }

    struct NoSecrets;

    impl HostSecureStorage for NoSecrets {
        fn get(&self, _key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
            Ok(None)
        }

        fn set(&self, _key: &str, _value: &[u8]) -> Result<(), HostCapabilityError> {
            Ok(())
        }

        fn delete(&self, _key: &str) -> Result<(), HostCapabilityError> {
            Ok(())
        }
    }

    fn reusable(counters: &Arc<Counters>) -> ReusableHost {
        ReusableHost::new(HostCapabilities::new(
            HostDirectories::new("/p".into(), "/c".into(), "/t".into(), "/l".into()),
            Box::new(NoSecrets),
            Box::new(OneShotClipboard {
                counters: Arc::clone(counters),
                taken: false,
            }),
            Box::new(NoFiles),
        ))
    }

    fn take_stream(host: &ReusableHost) -> Option<Box<dyn HostClipboardChangeStream>> {
        let (_, _, mut clipboard, _, _) = host.capabilities().into_parts();
        clipboard.take_change_stream().unwrap()
    }

    #[tokio::test]
    async fn a_one_shot_change_stream_serves_every_runtime_and_closes_once_at_the_end() {
        let counters = Arc::new(Counters::default());
        let host = reusable(&counters);

        for _ in 0..3 {
            let mut stream =
                take_stream(&host).expect("every runtime must receive the host change stream");
            assert_eq!(stream.next().await.unwrap(), HostClipboardChange::Changed);
            stream.shutdown().await.unwrap();
        }
        assert_eq!(counters.taken.load(Ordering::SeqCst), 1);
        assert_eq!(counters.shut_down.load(Ordering::SeqCst), 0);

        host.close_change_stream().await.unwrap();
        assert_eq!(counters.shut_down.load(Ordering::SeqCst), 1);
        host.close_change_stream().await.unwrap();
        assert_eq!(counters.shut_down.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_dropped_runtime_stream_is_returned_to_the_host_slot() {
        let counters = Arc::new(Counters::default());
        let host = reusable(&counters);

        let stream = take_stream(&host);
        assert!(stream.is_some());
        assert!(
            take_stream(&host).is_none(),
            "a stream already lent to a runtime must not be handed out twice"
        );
        drop(stream);

        assert!(take_stream(&host).is_some());
        assert_eq!(counters.taken.load(Ordering::SeqCst), 1);
    }
}
