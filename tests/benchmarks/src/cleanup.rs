//! 私有清理租约只保存本次生成的目录名和进程组，不保存密钥。
use crate::report;
use anyhow::{ensure, Context, Result};
use serde_json::json;
use std::{
    env,
    fs::{self, DirBuilder},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Path, PathBuf},
};

fn registry() -> Result<PathBuf> {
    let path = env::temp_dir().join("uc-benchmark-state");
    if !path.exists() {
        DirBuilder::new().mode(0o700).create(&path)?;
    }
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == unsafe { libc::geteuid() },
        "invalid cleanup registry"
    );
    Ok(path)
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // 复用已有 libc 绑定，避免仅为进程身份增加依赖。
fn process_start_abstime(pid: i32) -> Result<Option<u64>> {
    use libc::{proc_pid_rusage, rusage_info_v2, RUSAGE_INFO_V2};
    use std::mem::MaybeUninit;
    let mut usage = MaybeUninit::<rusage_info_v2>::zeroed();
    let status = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, usage.as_mut_ptr().cast()) };
    if status != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error.into());
    }
    Ok(Some(unsafe { usage.assume_init() }.ri_proc_start_abstime))
}

#[cfg(not(target_os = "macos"))]
fn process_start_abstime(_: i32) -> Result<Option<u64>> {
    Ok(None)
}

fn group_exists(group: i32) -> bool {
    let status = unsafe { libc::kill(-group, 0) };
    status == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

pub fn recover() -> Result<bool> {
    let directory = registry()?;
    let lease = directory.join("lease.json");
    if !lease.exists() {
        return Ok(false);
    }
    let value = report::read(&lease)?;
    let name = value["directory"]
        .as_str()
        .context("cleanup directory missing")?;
    ensure!(
        name.starts_with("uc-testkit-") && !name.contains('/') && !name.contains('\\'),
        "invalid cleanup directory"
    );
    if let Some(group) = value["group"].as_i64() {
        ensure!(
            group > 1 && group <= i32::MAX as i64,
            "invalid cleanup process group"
        );
        let group = group as i32;
        if group_exists(group) {
            let recorded = value["group_start_abstime"].as_u64();
            let current = process_start_abstime(group)?;
            // 只有记录的进程身份仍指向原进程组时才拒绝回收；PID 被无关进程组复用时允许回收。
            ensure!(
                !matches!((recorded, current), (Some(recorded), Some(current)) if recorded == current),
                "cleanup process group still active"
            );
        }
    }
    let root = env::temp_dir().join(name);
    if root.exists() {
        let metadata = fs::symlink_metadata(&root)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() },
            "invalid cleanup ownership"
        );
        ensure!(
            report::read(&root.join(".uc-benchmark-owned"))?["directory"] == name,
            "cleanup marker mismatch"
        );
        fs::remove_dir_all(root)?;
    }
    fs::remove_file(lease)?;
    Ok(true)
}
pub fn register(root: &Path, group: Option<i32>) -> Result<()> {
    ensure!(
        root.parent()
            .context("profile parent missing")?
            .canonicalize()?
            == env::temp_dir().canonicalize()?,
        "unexpected profile parent"
    );
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .context("profile name missing")?;
    let marker = root.join(".uc-benchmark-owned");
    if !marker.exists() {
        report::write(&marker, &json!({"directory":name}))?;
    }
    let directory = registry()?;
    let pending = directory.join("lease.pending");
    if pending.exists() {
        fs::remove_file(&pending)?;
    }
    let identity = match group {
        Some(pid) => process_start_abstime(pid)?,
        None => None,
    };
    report::write(
        &pending,
        &json!({"schema_version":1,"directory":name,"group":group,"group_start_abstime":identity}),
    )?;
    fs::File::open(&pending)?.sync_all()?;
    fs::rename(pending, directory.join("lease.json"))?;
    fs::File::open(&directory)?.sync_all()?;
    Ok(())
}
pub fn complete() -> Result<()> {
    fs::remove_file(registry()?.join("lease.json"))?;
    Ok(())
}
