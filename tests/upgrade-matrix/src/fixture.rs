//! 测试合成内容：文本、图片与文件各一份，按单元、设备与步骤区分；以及早于 Engine 的旧版资料快照。
//! 均不含任何真实用户资料。

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uc_testkit::{FailureKind, ScenarioFailure};

use crate::catalog::LegacyAnchor;

/// 1x1 像素 PNG。
const PIXEL_PNG: [u8; 67] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[derive(Clone, Debug)]
pub(crate) enum Content {
    Text(String),
    Image(Vec<u8>),
    File {
        handle: String,
        name: String,
        bytes: Vec<u8>,
    },
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn content_digest(content: &Content) -> String {
    match content {
        Content::Text(text) => digest(text.as_bytes()),
        Content::Image(bytes) | Content::File { bytes, .. } => digest(bytes),
    }
}

/// 一组代表性内容；`tag` 使不同单元、设备与步骤的内容互不相同，捕获时间按步骤递增。
pub(crate) fn representative(tag: &str) -> Vec<Content> {
    // 图片在 PNG 尾部后追加标记字节，保持可解码的同时区分内容。
    let mut image = PIXEL_PNG.to_vec();
    image.extend_from_slice(tag.as_bytes());
    vec![
        Content::Text(format!("upgrade matrix synthetic text {tag}")),
        Content::Image(image),
        Content::File {
            handle: format!("file-{tag}"),
            name: format!("synthetic-{tag}.bin"),
            bytes: format!("upgrade matrix synthetic file {tag}").into_bytes(),
        },
    ]
}

pub(crate) fn text(tag: &str) -> Content {
    Content::Text(format!("upgrade matrix synthetic text {tag}"))
}

impl Content {
    pub(crate) fn capture_request(&self) -> Value {
        let observed_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            });
        match self {
            Self::Text(text) => {
                json!({ "kind": "text", "text": text, "observed_at_ms": observed_at_ms })
            }
            Self::Image(bytes) => {
                json!({ "kind": "image", "bytes": bytes, "observed_at_ms": observed_at_ms })
            }
            Self::File {
                handle,
                name,
                bytes,
            } => json!({
                "kind": "file",
                "handle": handle,
                "display_name": name,
                "bytes": bytes,
                "observed_at_ms": observed_at_ms,
            }),
        }
    }
}

/// 把旧版资料快照装入设备资料目录，返回宿主安全存储的初始内容。
///
/// 快照在使用前按 `SHA256SUMS` 逐个校验，文件集合必须完全一致。`profile/` 原样放到 Engine
/// 私有目录（与 Desktop 传给 Engine 的应用数据根目录对应）；便携模式的 `keyring/` 文件解码为
/// 安全存储条目，与 Desktop 1.0 便携模式读取同一目录的行为一致。
pub(crate) fn install_legacy_profile(
    anchor: &LegacyAnchor,
    device_root: &Path,
) -> Result<Value, ScenarioFailure> {
    // 测试二进制复制到其他机器运行时，以 `UC_UPGRADE_MATRIX_DIR` 指向随附的矩阵目录。
    let matrix = std::env::var_os("UC_UPGRADE_MATRIX_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let fixture = matrix.join(&anchor.fixture);
    let profile = fixture.join("profile");
    let listed = fs::read_to_string(fixture.join("SHA256SUMS"))
        .map_err(|_| invalid_fixture("legacy-fixture-checksums-missing"))?;
    let mut expected = BTreeMap::new();
    for line in listed.lines().filter(|line| !line.is_empty()) {
        let (sum, relative) = line
            .split_once("  ")
            .ok_or_else(|| invalid_fixture("legacy-fixture-checksums-invalid"))?;
        expected.insert(relative.to_owned(), sum.to_owned());
    }
    let mut actual = BTreeMap::new();
    collect_files(&profile, &profile, &mut actual)?;
    if actual.keys().ne(expected.keys()) {
        return Err(invalid_fixture("legacy-fixture-files-mismatch"));
    }
    let private = device_root.join("private");
    let mut storage = serde_json::Map::new();
    for (relative, bytes) in &actual {
        if expected.get(relative) != Some(&digest(bytes)) {
            return Err(invalid_fixture("legacy-fixture-checksum-mismatch"));
        }
        let target = private.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|_| invalid_fixture("legacy-fixture-copy"))?;
        }
        fs::write(&target, bytes).map_err(|_| invalid_fixture("legacy-fixture-copy"))?;
        if let Some(name) = relative
            .strip_prefix("keyring/")
            .and_then(|name| name.strip_suffix(".bin"))
        {
            storage.insert(decode_key_name(name)?, json!(bytes));
        }
    }
    if storage.is_empty() {
        return Err(invalid_fixture("legacy-fixture-keyring-empty"));
    }
    Ok(Value::Object(storage))
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), ScenarioFailure> {
    let entries = fs::read_dir(directory).map_err(|_| invalid_fixture("legacy-fixture-read"))?;
    for entry in entries {
        let path = entry
            .map_err(|_| invalid_fixture("legacy-fixture-read"))?
            .path();
        if path.is_dir() {
            collect_files(root, &path, files)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid_fixture("legacy-fixture-read"))?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = fs::read(&path).map_err(|_| invalid_fixture("legacy-fixture-read"))?;
        files.insert(relative, bytes);
    }
    Ok(())
}

/// 0.19.x 文件钥匙串以键名 UTF-8 字节的小写十六进制命名。
fn decode_key_name(hex: &str) -> Result<String, ScenarioFailure> {
    if hex.len() % 2 != 0 {
        return Err(invalid_fixture("legacy-fixture-key-name"));
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_fixture("legacy-fixture-key-name"))?;
    String::from_utf8(bytes).map_err(|_| invalid_fixture("legacy-fixture-key-name"))
}

fn invalid_fixture(condition: &'static str) -> ScenarioFailure {
    ScenarioFailure::new(FailureKind::FixtureInvalid, condition)
}
