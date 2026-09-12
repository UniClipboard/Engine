use std::{
    env,
    io::{self, Write},
    path::Path,
    process::Command,
};

fn git(root: &Path, args: &[&str]) -> Option<String> {
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

fn valid_commit(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = env::var("CARGO_MANIFEST_DIR")?;
    let root = Path::new(&manifest)
        .parent()
        .and_then(Path::parent)
        .ok_or("missing workspace root")?;
    let mut output = io::stdout().lock();
    for name in ["UC_ENGINE_SOURCE_COMMIT", "UC_ENGINE_SOURCE_STATE"] {
        writeln!(output, "cargo:rerun-if-env-changed={name}")?;
    }
    for path in [
        "crates",
        "bindings",
        "compatibility",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        writeln!(
            output,
            "cargo:rerun-if-changed={}",
            root.join(path).display()
        )?;
    }
    for name in ["HEAD", "index", "packed-refs"] {
        if let Some(path) = git(
            root,
            &["rev-parse", "--path-format=absolute", "--git-path", name],
        ) {
            writeln!(output, "cargo:rerun-if-changed={path}")?;
        }
    }
    if let Some(reference) = git(root, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(
            root,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                &reference,
            ],
        ) {
            writeln!(output, "cargo:rerun-if-changed={path}")?;
        }
    }
    let explicit = env::var("UC_ENGINE_SOURCE_COMMIT")
        .ok()
        .filter(|value| valid_commit(value));
    let commit = explicit
        .clone()
        .or_else(|| git(root, &["rev-parse", "HEAD"]).filter(|value| valid_commit(value)))
        .unwrap_or_else(|| "unknown".into());
    let state = if explicit.is_some() {
        env::var("UC_ENGINE_SOURCE_STATE")
            .ok()
            .filter(|state| matches!(state.as_str(), "clean" | "modified" | "unknown"))
            .unwrap_or_else(|| "unknown".into())
    } else {
        git(root, &["status", "--porcelain"])
            .map(|status| {
                if status.is_empty() {
                    "clean"
                } else {
                    "modified"
                }
                .to_owned()
            })
            .unwrap_or_else(|| "unknown".into())
    };
    writeln!(
        output,
        "cargo:rustc-env=UC_OBSERVABILITY_SOURCE_COMMIT={commit}"
    )?;
    writeln!(
        output,
        "cargo:rustc-env=UC_OBSERVABILITY_SOURCE_STATE={state}"
    )?;
    Ok(())
}
