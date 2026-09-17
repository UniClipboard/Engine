use std::path::Path;
use std::process::Command;

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

pub(super) fn git(root: &Path, args: &[&str]) -> Option<String> {
    let result = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    result
        .status
        .success()
        .then(|| String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

pub(super) fn source_state(root: &Path) -> String {
    let mut args = vec!["status", "--porcelain", "--"];
    args.extend(BUILD_INPUTS);
    git(root, &args)
        .map(|status| {
            if status.is_empty() {
                "clean"
            } else {
                "modified"
            }
            .to_owned()
        })
        .unwrap_or_else(|| "unknown".into())
}
