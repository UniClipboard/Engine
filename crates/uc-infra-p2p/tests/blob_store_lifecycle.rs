#![cfg(target_os = "linux")]

use std::{fs, time::Duration};

use anyhow::Result;
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use iroh_blobs::{
    api::Store,
    store::{
        fs::{options::Options, FsStore},
        GcConfig,
    },
};
use tokio::{
    sync::oneshot,
    time::{sleep, timeout},
};

fn store_threads() -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir("/proc/self/task")? {
        match fs::read_to_string(entry?.path().join("comm")) {
            Ok(name) if name.starts_with("iroh-blob-") => count += 1,
            Ok(_) => {}
            // Linux 在任务目录枚举后退出时可返回 ENOENT 或 ESRCH。
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(3) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(count)
}

// OS 线程目录可能短暂保留刚退出线程的条目；原回归仍要求两秒内全部消失。
async fn wait_for_store_threads(initial: usize) -> Result<()> {
    timeout(Duration::from_secs(2), async {
        loop {
            if store_threads()? <= initial {
                return Ok::<_, anyhow::Error>(());
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

// 即便通用 Store 引用仍然存在，关闭也必须释放整个 SDK 私有 runtime。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_joins_private_store_runtime_with_and_without_gc() -> Result<()> {
    let initial = store_threads()?;
    for enable_gc in [false, true] {
        let root = tempfile::tempdir()?;
        let mut options = Options::new(root.path());
        if enable_gc {
            options.gc = Some(GcConfig {
                interval: Duration::from_secs(3600),
                add_protected: None,
            });
        }
        let fs = FsStore::load_with_opts(root.path().join("blobs.db"), options.clone()).await?;
        let store: Store = fs.clone().into();
        // 观察订阅是长期流；保留消费者不能阻止整个 Store 关闭。
        let observed = store.add_slice(b"observed live content").temp_tag().await?;
        let mut observer = Box::pin(store.observe(observed.hash()).stream().await?);
        assert!(timeout(Duration::from_secs(5), observer.next())
            .await?
            .is_some());
        assert!(timeout(Duration::from_millis(50), observer.next())
            .await
            .is_err());
        timeout(Duration::from_secs(5), store.shutdown()).await??;
        assert!(timeout(Duration::from_secs(5), observer.next())
            .await?
            .is_none());
        drop(observer);
        drop(fs);
        wait_for_store_threads(initial).await?;
        drop(store);

        // 已被 SDK 接受的导入需要通过原 FinishImport 路径结算，关闭不能提前关库。
        let fs = FsStore::load_with_opts(root.path().join("blobs.db"), options.clone()).await?;
        let importer = fs.clone();
        let closer = fs.clone();
        let (started, entered) = oneshot::channel();
        let (release, resume) = oneshot::channel();
        let importing = tokio::spawn(async move {
            let input = stream::unfold(Some((started, resume)), |state| async move {
                let (started, resume) = state?;
                let _ = started.send(());
                let item = resume
                    .await
                    .map_err(std::io::Error::other)
                    .map(|()| Bytes::from_static(b"accepted import must settle before shutdown"));
                Some((item, None))
            });
            importer.add_stream(input).await.temp_tag().await
        });
        timeout(Duration::from_secs(5), entered).await??;
        let mut closing = tokio::spawn(async move { closer.shutdown().await });
        assert!(timeout(Duration::from_millis(50), &mut closing)
            .await
            .is_err());
        release
            .send(())
            .map_err(|_| anyhow::anyhow!("import receiver closed early"))?;
        let tag = timeout(Duration::from_secs(5), importing).await???;
        let hash = tag.hash();
        timeout(Duration::from_secs(5), closing).await???;
        wait_for_store_threads(initial).await?;
        drop(tag);
        drop(fs);
        let reopened = FsStore::load(root.path()).await?;
        assert_eq!(
            reopened.get_bytes(hash).await?.as_ref(),
            b"accepted import must settle before shutdown"
        );
        timeout(Duration::from_secs(5), reopened.shutdown()).await??;
        wait_for_store_threads(initial).await?;

        // 自然释放不能被小时级 GC 睡眠反向保活。
        let store = FsStore::load_with_opts(root.path().join("blobs.db"), options).await?;
        drop(store);
        wait_for_store_threads(initial).await?;

        let invalid = root.path().join("not-a-directory");
        fs::write(&invalid, b"fixture")?;
        assert!(FsStore::load(&invalid).await.is_err());
        wait_for_store_threads(initial).await?;
    }
    // Store 拥有独立 runtime；加载它的调用方 runtime 可以先退出。
    tokio::task::spawn_blocking(|| -> Result<()> {
        let root = tempfile::tempdir()?;
        let loader = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let store = loader.block_on(FsStore::load(root.path()))?;
        drop(loader);
        let closer = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        closer.block_on(async {
            timeout(Duration::from_secs(5), store.shutdown()).await??;
            wait_for_store_threads(0).await
        })
    })
    .await??;
    Ok(())
}
