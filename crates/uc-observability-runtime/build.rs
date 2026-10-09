mod build_support;

use build_support::{git, source_state, BUILD_INPUTS};
use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// 与 `src/build_source.rs` 读取的数组名和长度一致。
const SYMBOL: &str = "uc_observability_build_source_v1";
const LENGTH: usize = 64;
const LIBRARY: &str = "uc_observability_build_source";

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
    let explicit = env::var("UC_ENGINE_SOURCE_COMMIT")
        .ok()
        .filter(|value| valid_commit(value));
    // 显式提供来源时，输出只取决于上面两个环境变量；不再跟踪 git 状态与源码目录，
    // 否则源码目录中任何无关文件的修改时间变化都会连锁重编所有依赖本 crate 的包。
    if explicit.is_none() {
        for path in BUILD_INPUTS {
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
    }
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
        source_state(root)
    };
    // 来源编译为独立原生静态库，而不是以 rustc-env 编入本 crate 的代码。默认把它打包进 rlib，所有产物
    // （含任何 crate 生成的静态库）与以往一样自带来源。显式提供来源的托管构建（CI）改为不打包：rlib 不再随
    // 提交变化，本 crate 与 uc-engine 跨提交可命中编译缓存，来源由 Cargo 在最终链接可执行文件、测试与动态库
    // 时加入；这类构建顺带生成的非 iOS 静态库不含来源，不作为交付物。iOS 交付的是静态库，始终打包。
    let out = PathBuf::from(env::var("OUT_DIR")?);
    let content = format!("{commit} {state}");
    if content.len() >= LENGTH {
        return Err("build source does not fit the linked array".into());
    }
    let bytes = content
        .bytes()
        .chain(std::iter::repeat_n(0, LENGTH - content.len()))
        .map(|byte| byte.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let source = out.join("build_source.c");
    fs::write(
        &source,
        format!("const unsigned char {SYMBOL}[{LENGTH}] = {{{bytes}}};\n"),
    )?;
    // 只含一个数据数组，调试信息无用，且会把本机构建目录写进发布静态库。
    cc::Build::new()
        .file(&source)
        .debug(false)
        .cargo_metadata(false)
        .try_compile(LIBRARY)?;
    let kind = if explicit.is_none() || env::var("CARGO_CFG_TARGET_OS")? == "ios" {
        "static"
    } else {
        "static:-bundle"
    };
    writeln!(output, "cargo:rustc-link-search=native={}", out.display())?;
    writeln!(output, "cargo:rustc-link-lib={kind}={LIBRARY}")?;
    Ok(())
}
