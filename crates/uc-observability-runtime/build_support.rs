use std::fs;
use std::path::Path;
use std::process::Command;
use std::str;

pub(super) const BUILD_INPUTS: [&str; 9] = [
    ".cargo/config.toml",
    "crates",
    "bindings",
    "compatibility",
    "tests",
    "third_party",
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
];

fn git_output(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new("git");
    // Git hook 的仓库选择变量不能覆盖本次要检查的 checkout。
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
    ] {
        command.env_remove(name);
    }
    let result = command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    result.status.success().then_some(result.stdout)
}

pub(super) fn git(root: &Path, args: &[&str]) -> Option<String> {
    git_output(root, args).map(|output| String::from_utf8_lossy(&output).trim().to_owned())
}

pub(super) fn source_state(root: &Path) -> String {
    match source_modified(root, &BUILD_INPUTS) {
        Some(false) => "clean",
        Some(true) => "modified",
        None => "unknown",
    }
    .into()
}

fn source_modified(root: &Path, inputs: &[&str]) -> Option<bool> {
    // 未初始化的子模块目录会让 Git 向上找到父仓库，不能据此认定子模块干净。
    if !git(root, &["rev-parse", "--show-prefix"])?.is_empty() {
        return None;
    }
    let mut index_args = vec!["ls-files", "--stage", "-z", "--"];
    index_args.extend_from_slice(inputs);
    let index = git_output(root, &index_args)?;
    let mut submodules = Vec::new();
    for entry in index
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if !entry.starts_with(b"160000 ") {
            continue;
        }
        let separator = entry.iter().position(|byte| *byte == b'\t')?;
        let header = str::from_utf8(&entry[..separator]).ok()?;
        let mut fields = header.split(' ');
        fields.next()?;
        let revision = fields.next()?.to_owned();
        if fields.next()? != "0" {
            return Some(true);
        }
        let path = str::from_utf8(&entry[separator + 1..]).ok()?.to_owned();
        submodules.push((path, revision));
    }
    // -z 保留路径原始分隔；强制检查子模块，不能受本机 ignore 配置影响。
    let mut status_args = vec![
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignore-submodules=none",
        "--",
    ];
    status_args.extend_from_slice(inputs);
    let status = git_output(root, &status_args)?;
    for entry in status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let state = entry.get(..3)?;
        let path = entry.get(3..)?;
        if state == b"?? " && path == b".cargo-ok" {
            // Cargo 的 checkout-ready 标记是根目录中的未跟踪空普通文件。
            let marker = fs::symlink_metadata(root.join(".cargo-ok")).ok()?;
            if marker.is_file() && marker.len() == 0 {
                continue;
            }
        }
        if state == b" M " && submodules.iter().any(|(name, _)| name.as_bytes() == path) {
            // 父仓库只报告聚合状态；在下面核对子模块修订与真实文件变化。
            continue;
        }
        return Some(true);
    }
    for (path, revision) in submodules {
        let child = root.join(path);
        if !git(&child, &["rev-parse", "--show-prefix"])?.is_empty() {
            return None;
        }
        if git(&child, &["rev-parse", "HEAD"])? != revision {
            return Some(true);
        }
        if source_modified(&child, &[])? {
            return Some(true);
        }
    }
    Some(false)
}
