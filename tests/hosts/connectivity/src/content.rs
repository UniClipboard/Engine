//! 测试合成剪贴板内容：宿主按指令放入一份快照，再经公开 `CaptureCurrentClipboard` 写入历史。

use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use url::Url;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uc_engine::{
    Engine, HostCapabilityError, HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot,
    HostFileHandle, Operation, OperationResult,
};

use crate::{string, unavailable, Files, ManagedFile};

#[derive(Clone)]
pub(crate) struct Clipboard(Arc<Mutex<HostClipboardSnapshot>>, PathBuf);

impl Clipboard {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self(
            Arc::new(Mutex::new(HostClipboardSnapshot {
                observed_at_ms: 0,
                representations: vec![],
            })),
            root,
        )
    }
}

impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(self.0.lock().map_err(|_| unavailable())?.clone())
    }
    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        *self.0.lock().map_err(|_| unavailable())? = snapshot;
        Ok(())
    }
}

impl Clipboard {
    /// 只读回合成内容的系统写入结果；不通过历史推断应用成功。
    pub(crate) fn observed(&self) -> Result<Value> {
        let snapshot = self.0.lock().map_err(|_| unavailable())?;
        let mut representations = Vec::new();
        for representation in &snapshot.representations {
            let value = match representation {
                HostClipboardRepresentation::Inline {
                    format,
                    mime_type,
                    bytes,
                } if mime_type
                    .as_deref()
                    .is_some_and(|mime| matches!(mime, "text/uri-list" | "file/uri-list"))
                    || format == "files" =>
                {
                    let mut files = Vec::new();
                    let mut directories = Vec::new();
                    for line in std::str::from_utf8(bytes)?
                        .lines()
                        .filter(|line| !line.is_empty())
                    {
                        let path = Url::parse(line)?
                            .to_file_path()
                            .map_err(|_| anyhow::anyhow!("invalid synthetic file URI"))?;
                        let path = path.canonicalize()?;
                        if !path.starts_with(self.1.canonicalize()?) {
                            bail!("synthetic clipboard path must remain in host profile");
                        }
                        if path.is_dir() {
                            let mut members = Vec::new();
                            read_directory(&path, &path, &mut members)?;
                            members.sort_by_key(Value::to_string);
                            directories.push(json!({ "members": members }));
                        } else {
                            files.push(read_file(&path)?);
                        }
                    }
                    files.sort_by_key(Value::to_string);
                    directories.sort_by_key(Value::to_string);
                    json!({ "kind": "files", "files": files, "directories": directories })
                }
                HostClipboardRepresentation::Inline {
                    format,
                    mime_type,
                    bytes,
                } => json!({
                    "kind": "inline", "format": format, "mime_type": mime_type,
                    "size": bytes.len(), "sha256": digest(bytes),
                }),
                HostClipboardRepresentation::File {
                    format, size_bytes, ..
                } => json!({
                    "kind": "file", "format": format, "size": size_bytes,
                }),
            };
            representations.push(value);
        }
        Ok(json!({ "representations": representations }))
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read_file(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path)?;
    Ok(json!({ "kind": "file", "size": bytes.len(), "sha256": digest(&bytes) }))
}
fn read_directory(root: &Path, directory: &Path, members: &mut Vec<Value>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            bail!("synthetic directory must not contain symlinks");
        }
        let relative = path
            .strip_prefix(root)?
            .to_str()
            .context("synthetic member UTF-8")?
            .replace('\\', "/");
        let mut value = if kind.is_dir() {
            json!({ "kind": "directory" })
        } else {
            read_file(&path)?
        };
        value["name_sha256"] = json!(digest(relative.as_bytes()));
        members.push(value);
        if kind.is_dir() {
            read_directory(root, &path, members)?;
        }
    }
    Ok(())
}

fn bytes(request: &Value) -> Result<Vec<u8>> {
    request["bytes"]
        .as_array()
        .context("missing test content bytes")?
        .iter()
        .map(|byte| {
            byte.as_u64()
                .and_then(|byte| u8::try_from(byte).ok())
                .context("invalid test content byte")
        })
        .collect()
}

/// 把一份合成内容放入测试剪贴板并捕获为历史条目。
pub(crate) async fn capture(
    engine: &Engine,
    clipboard: &Clipboard,
    files: &Files,
    request: &Value,
) -> Result<Value> {
    let observed_at_ms = request["observed_at_ms"]
        .as_i64()
        .context("missing capture time")?;
    let representation = match string(request, "kind")? {
        "text" => HostClipboardRepresentation::Inline {
            format: "text".into(),
            mime_type: Some("text/plain".into()),
            bytes: string(request, "text")?.as_bytes().to_vec(),
        },
        "image" => HostClipboardRepresentation::Inline {
            format: "image".into(),
            mime_type: Some("image/png".into()),
            bytes: bytes(request)?,
        },
        "directory" => {
            let root = clipboard.1.join("synthetic-content/directory");
            std::fs::create_dir_all(&root)?;
            for member in request["members"]
                .as_array()
                .context("synthetic directory members")?
            {
                let relative = Path::new(string(member, "name")?);
                if relative
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
                {
                    bail!("synthetic member must be relative");
                }
                let destination = root.join(relative);
                if member["directory"] == true {
                    std::fs::create_dir_all(destination)?;
                } else {
                    std::fs::create_dir_all(
                        destination.parent().context("synthetic member parent")?,
                    )?;
                    std::fs::write(destination, string(member, "content")?.as_bytes())?;
                }
            }
            HostClipboardRepresentation::Inline {
                format: "files".into(),
                mime_type: Some("text/uri-list".into()),
                bytes: Url::from_file_path(root)
                    .map_err(|_| anyhow::anyhow!("synthetic directory URI"))?
                    .as_str()
                    .as_bytes()
                    .to_vec(),
            }
        }
        "file" => {
            let handle = string(request, "handle")?;
            let content = bytes(request)?;
            let size_bytes = content.len() as u64;
            let display_name = string(request, "display_name")?.to_owned();
            files.0.lock().map_err(|_| unavailable())?.insert(
                handle.to_owned(),
                ManagedFile {
                    display_name: display_name.clone(),
                    mime_type: Some("application/octet-stream".into()),
                    bytes: content,
                },
            );
            HostClipboardRepresentation::File {
                format: "files".into(),
                handle: HostFileHandle::new(handle),
                display_name,
                mime_type: Some("application/octet-stream".into()),
                size_bytes,
            }
        }
        _ => bail!("unknown test content kind"),
    };
    *clipboard.0.lock().map_err(|_| unavailable())? = HostClipboardSnapshot {
        observed_at_ms,
        representations: vec![representation],
    };
    match engine.execute(Operation::CaptureCurrentClipboard).await? {
        OperationResult::ClipboardCaptured { entry_id, .. } => Ok(json!({ "entry": entry_id })),
        _ => bail!("clipboard capture result expected"),
    }
}
