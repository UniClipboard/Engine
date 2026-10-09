//! 捕获当前线程 tracing 输出的测试辅助：断言某条记录是否出现、出现几次以及携带哪些固定字段。
//!
//! 订阅者只对安装它的线程生效，异步测试需使用单线程运行时（`#[tokio::test]` 默认即是）。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing::subscriber::DefaultGuard;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
pub struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

pub struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Ok(mut captured) = self.0.lock() {
            captured.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for CapturedLogs {
    type Writer = CapturedLogWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedLogWriter(Arc::clone(&self.0))
    }
}

impl CapturedLogs {
    /// 在当前线程安装订阅者，返回的守卫释放后恢复原订阅者。异步测试需使用单线程运行时。
    pub fn install(&self) -> DefaultGuard {
        let subscriber = tracing_subscriber::fmt()
            .with_writer(self.clone())
            .with_ansi(false)
            .with_max_level(Level::TRACE)
            .finish();
        tracing::subscriber::set_default(subscriber)
    }

    pub fn output(&self) -> String {
        self.0
            .lock()
            .map(|captured| String::from_utf8_lossy(&captured).into_owned())
            .unwrap_or_default()
    }

    /// 输出里包含 `needle` 的行数。
    pub fn count(&self, needle: &str) -> usize {
        self.output()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    }
}
