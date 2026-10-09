use std::io::{self, Read, Write};
use std::path::Path;

use uc_observability_contract::{error_source::io_error_kind, uc_warn};

use crate::{HostCapabilityError, HostFileAccess, HostFileHandle};

const COPY_CHUNK_SIZE: usize = 64 * 1024;

/// 复制失败；各变体只在 Engine 内部使用，对外错误码由调用方的映射决定。
pub(crate) enum HostFileCopyError {
    /// 读取本地源文件失败。
    LocalRead(io::Error),
    /// 创建、写入或刷新本地目标文件失败。
    LocalWrite(io::Error),
    /// 宿主返回了空块或超出请求大小的块。
    HostChunkInvalid,
    Host(HostCapabilityError),
}

impl HostFileCopyError {
    /// 复制流程的负责人在映射为公开错误码前调用一次；只写固定分类与 io 种类，不含路径。
    pub(crate) fn record(&self) {
        match self {
            Self::LocalRead(error) => uc_warn!(
                error_kind = "local_file_read",
                io_error_kind = io_error_kind(error),
                "host file copy failed to read the local file"
            ),
            Self::LocalWrite(error) => uc_warn!(
                error_kind = "local_file_write",
                io_error_kind = io_error_kind(error),
                "host file copy failed to write the local file"
            ),
            Self::HostChunkInvalid => uc_warn!(
                error_kind = "host_chunk_invalid",
                "host file copy received an invalid chunk from the host"
            ),
            Self::Host(_) => {}
        }
    }
}

pub(crate) async fn copy_path_to_host(
    files: &dyn HostFileAccess,
    destination: &HostFileHandle,
    source: &Path,
) -> Result<(), HostFileCopyError> {
    let mut input = std::fs::File::open(source).map_err(HostFileCopyError::LocalRead)?;
    let mut buffer = vec![0_u8; COPY_CHUNK_SIZE];
    let mut offset = 0_u64;
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(HostFileCopyError::LocalRead)?;
        if read == 0 {
            break;
        }
        files
            .write_chunk(destination, offset, &buffer[..read])
            .map_err(HostFileCopyError::Host)?;
        offset += read as u64;
        tokio::task::yield_now().await;
    }
    files
        .finish_write(destination)
        .map_err(HostFileCopyError::Host)
}

pub(crate) async fn copy_host_to_path(
    files: &dyn HostFileAccess,
    source: &HostFileHandle,
    destination: &Path,
) -> Result<(), HostFileCopyError> {
    let metadata = files.metadata(source).map_err(HostFileCopyError::Host)?;
    let mut output = std::fs::File::create(destination).map_err(HostFileCopyError::LocalWrite)?;
    let mut offset = 0_u64;
    while offset < metadata.size_bytes {
        let remaining = metadata.size_bytes - offset;
        let requested = remaining.min(COPY_CHUNK_SIZE as u64) as u32;
        let chunk = files
            .read_chunk(source, offset, requested)
            .map_err(HostFileCopyError::Host)?;
        if chunk.is_empty() || chunk.len() > requested as usize {
            return Err(HostFileCopyError::HostChunkInvalid);
        }
        output
            .write_all(&chunk)
            .map_err(HostFileCopyError::LocalWrite)?;
        offset += chunk.len() as u64;
        tokio::task::yield_now().await;
    }
    output.flush().map_err(HostFileCopyError::LocalWrite)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    use uc_core::ids::RepresentationId;

    use super::*;
    use crate::{HostCapabilityError, HostFileMetadata};

    struct ScriptedHostSource {
        size_bytes: u64,
        chunk: Vec<u8>,
    }

    impl HostFileAccess for ScriptedHostSource {
        fn metadata(
            &self,
            _handle: &HostFileHandle,
        ) -> Result<HostFileMetadata, HostCapabilityError> {
            Ok(HostFileMetadata {
                display_name: "PRIVATE_HOST_FILE_NAME".into(),
                size_bytes: self.size_bytes,
                mime_type: None,
            })
        }

        fn read_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _max_bytes: u32,
        ) -> Result<Vec<u8>, HostCapabilityError> {
            Ok(self.chunk.clone())
        }

        fn write_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _bytes: &[u8],
        ) -> Result<(), HostCapabilityError> {
            unreachable!("copy to a path never writes to the host")
        }

        fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
            unreachable!("copy to a path never writes to the host")
        }
    }

    fn private_missing_path() -> std::path::PathBuf {
        std::env::temp_dir()
            .join(format!("PRIVATE_MISSING_DIR_{}", RepresentationId::new()))
            .join("PRIVATE_FILE_NAME")
    }

    #[tokio::test]
    async fn a_missing_local_source_is_recorded_as_a_read_failure_without_its_path() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let files = RecordingFiles::default();

        let error = copy_path_to_host(
            &files,
            &HostFileHandle::new("destination"),
            &private_missing_path(),
        )
        .await
        .unwrap_err();
        error.record();

        assert!(matches!(error, HostFileCopyError::LocalRead(_)));
        assert_eq!(
            logs.count("error_kind=\"local_file_read\""),
            1,
            "{}",
            logs.output()
        );
        assert!(logs.output().contains("NotFound"), "{}", logs.output());
        assert!(!logs.output().contains("PRIVATE"), "{}", logs.output());
    }

    #[tokio::test]
    async fn an_unwritable_local_destination_is_recorded_as_a_write_failure_not_a_source_failure() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let files = ScriptedHostSource {
            size_bytes: 1,
            chunk: vec![1],
        };

        let error = copy_host_to_path(
            &files,
            &HostFileHandle::new("source"),
            &private_missing_path(),
        )
        .await
        .unwrap_err();
        error.record();

        assert!(matches!(error, HostFileCopyError::LocalWrite(_)));
        assert_eq!(
            logs.count("error_kind=\"local_file_write\""),
            1,
            "{}",
            logs.output()
        );
        assert_eq!(logs.count("local_file_read"), 0, "{}", logs.output());
        assert!(!logs.output().contains("PRIVATE"), "{}", logs.output());
    }

    #[tokio::test]
    async fn an_invalid_host_chunk_is_recorded_with_a_fixed_kind() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let destination =
            std::env::temp_dir().join(format!("uc-engine-host-chunk-{}", RepresentationId::new()));
        let files = ScriptedHostSource {
            size_bytes: 1,
            chunk: Vec::new(),
        };

        let result = copy_host_to_path(&files, &HostFileHandle::new("source"), &destination).await;
        let _ = std::fs::remove_file(&destination);
        let error = result.unwrap_err();
        error.record();

        assert!(matches!(error, HostFileCopyError::HostChunkInvalid));
        assert_eq!(
            logs.count("error_kind=\"host_chunk_invalid\""),
            1,
            "{}",
            logs.output()
        );
    }

    #[derive(Default)]
    struct RecordingFiles {
        bytes: Mutex<Vec<u8>>,
        finished: AtomicBool,
    }

    impl HostFileAccess for RecordingFiles {
        fn metadata(
            &self,
            _handle: &HostFileHandle,
        ) -> Result<HostFileMetadata, HostCapabilityError> {
            unreachable!("copy does not query destination metadata")
        }

        fn read_chunk(
            &self,
            _handle: &HostFileHandle,
            _offset: u64,
            _max_bytes: u32,
        ) -> Result<Vec<u8>, HostCapabilityError> {
            unreachable!("copy does not read from destination")
        }

        fn write_chunk(
            &self,
            _handle: &HostFileHandle,
            offset: u64,
            bytes: &[u8],
        ) -> Result<(), HostCapabilityError> {
            let mut output = self.bytes.lock().expect("output lock");
            assert_eq!(offset, output.len() as u64);
            output.extend_from_slice(bytes);
            Ok(())
        }

        fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[tokio::test]
    async fn copy_writes_every_chunk_and_finishes_the_host_handle() {
        let source =
            std::env::temp_dir().join(format!("uc-engine-host-copy-{}", RepresentationId::new()));
        let expected = vec![0x5a; COPY_CHUNK_SIZE * 2 + 17];
        std::fs::write(&source, &expected).expect("write source");
        let files = RecordingFiles::default();

        let result = copy_path_to_host(&files, &HostFileHandle::new("destination"), &source).await;
        let _ = std::fs::remove_file(source);

        assert!(result.is_ok());
        assert_eq!(*files.bytes.lock().expect("output lock"), expected);
        assert!(files.finished.load(Ordering::SeqCst));
    }
}
