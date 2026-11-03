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
        ensure!(
            unsafe { libc::kill(-(group as i32), 0) } != 0
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH),
            "cleanup process group still active"
        );
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
    report::write(
        &pending,
        &json!({"schema_version":1,"directory":name,"group":group}),
    )?;
    fs::File::open(&pending)?.sync_all()?;
    fs::rename(pending, directory.join("lease.json"))?;
    Ok(())
}
pub fn complete() -> Result<()> {
    fs::remove_file(registry()?.join("lease.json"))?;
    Ok(())
}
