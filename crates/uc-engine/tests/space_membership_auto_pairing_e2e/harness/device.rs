//! 单个设备的 Engine 启动与资料目录。

use super::*;

pub(crate) struct DeviceHarness {
    pub(crate) root: uc_testkit::TempDirLease,
    pub(crate) secure_storage: MemorySecureStorage,
    pub(crate) rendezvous_base_url: String,
}

impl DeviceHarness {
    pub(crate) fn new(rendezvous_base_url: String) -> Self {
        Self {
            root: scenario::temp_dir("device-profile"),
            secure_storage: MemorySecureStorage::default(),
            rendezvous_base_url,
        }
    }

    pub(crate) async fn start(&self) -> Engine {
        self.start_with_clipboard(Box::new(EmptyClipboard)).await
    }

    pub(crate) async fn start_with_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Engine {
        self.start_with_events(clipboard).await.0
    }

    pub(crate) async fn start_with_files(&self, files: Box<dyn HostFileAccess>) -> Engine {
        self.start_with_host(Box::new(EmptyClipboard), files)
            .await
            .0
    }

    pub(crate) async fn start_with_events(
        &self,
        clipboard: Box<dyn HostClipboard>,
    ) -> (Engine, uc_engine::EventStream) {
        self.start_with_host(clipboard, Box::new(EmptyFiles)).await
    }

    /// 场景只在本机直连，禁用公网 relay 回退，结果不受外部 relay 可达性影响。
    pub(crate) async fn start_with_host(
        &self,
        clipboard: Box<dyn HostClipboard>,
        files: Box<dyn HostFileAccess>,
    ) -> (Engine, uc_engine::EventStream) {
        let root = self.root.path();
        let host = HostCapabilities::new(
            HostDirectories::new(
                root.join("private"),
                root.join("cache"),
                root.join("temporary"),
                root.join("logs"),
            ),
            Box::new(self.secure_storage.clone()),
            clipboard,
            files,
        );
        let config = EngineConfig::new("1.1.0")
            .with_rendezvous_base_url(self.rendezvous_base_url.clone())
            .with_test_relay_fallback(false);
        Engine::start(config, host)
            .await
            .expect("start complete engine")
    }
}
