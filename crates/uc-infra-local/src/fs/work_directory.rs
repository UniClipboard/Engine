//! 失败路径上的临时工作目录清理。

use std::io::ErrorKind;
use std::path::Path;

use uc_observability_contract::{error_source::io_error_kind, uc_warn};

/// 尽力删除失败流程遗留的工作目录；不存在视为已清理，其他失败只记录分类，不改变调用方的失败结果。
///
/// 残留目录可能含数据库副本或重绑定数据，因此不记录路径。
pub fn remove_work_directory_best_effort(directory: &Path) {
    match std::fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => {
            uc_warn!(
                error_kind = "work_dir_cleanup",
                io_error_kind = io_error_kind(&error),
                "failed to remove work directory after failed preparation"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_an_existing_directory_and_tolerates_a_missing_one() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(work.join("nested")).unwrap();

        remove_work_directory_best_effort(&work);
        remove_work_directory_best_effort(&work);

        assert!(!work.exists());
    }

    #[test]
    fn a_cleanup_failure_is_recorded_without_the_path() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not-a-directory-secret-name");
        std::fs::write(&file, b"x").unwrap();

        remove_work_directory_best_effort(&file);

        assert_eq!(logs.count("failed to remove work directory"), 1);
        assert!(logs.output().contains("error_kind=\"work_dir_cleanup\""));
        assert!(!logs.output().contains("not-a-directory-secret-name"));
    }
}
