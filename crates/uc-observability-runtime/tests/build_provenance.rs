#[path = "../build_support.rs"]
mod build_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use build_support::source_state;
use tempfile::TempDir;

struct TestRepository {
    _directory: TempDir,
    root: PathBuf,
}

impl TestRepository {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("create temporary repository");
        let root = directory.path().to_owned();
        fs::create_dir(root.join("crates")).expect("create source directory");
        fs::write(root.join("crates/lib.rs"), "pub fn example() {}\n")
            .expect("write tracked source");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(root.join("Cargo.lock"), "# lock\n").expect("write lockfile");
        run_git(&root, &["init", "-q"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test"]);
        run_git(&root, &["add", "."]);
        run_git(&root, &["commit", "-qm", "initial"]);
        Self {
            _directory: directory,
            root,
        }
    }
}

fn run_git(root: &Path, args: &[&str]) {
    let result = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git command");
    assert!(
        result.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn clean_checkout_has_clean_source_state() {
    let repository = TestRepository::new();

    assert_eq!(source_state(&repository.root), "clean");
}

#[test]
fn cargo_checkout_marker_does_not_modify_source_state() {
    let repository = TestRepository::new();
    fs::write(repository.root.join(".cargo-ok"), "").expect("write Cargo checkout marker");

    assert_eq!(source_state(&repository.root), "clean");
}

#[test]
fn tracked_source_edit_modifies_source_state() {
    let repository = TestRepository::new();
    fs::write(
        repository.root.join("crates/lib.rs"),
        "pub fn changed() {}\n",
    )
    .expect("edit tracked source");

    assert_eq!(source_state(&repository.root), "modified");
}

#[test]
fn untracked_source_file_modifies_source_state() {
    let repository = TestRepository::new();
    fs::write(
        repository.root.join("crates/new_source.rs"),
        "pub fn new_source() {}\n",
    )
    .expect("write untracked source");

    assert_eq!(source_state(&repository.root), "modified");
}
