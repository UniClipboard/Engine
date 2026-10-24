//! 认证后的引用仍须核对当前文件系统归属；验证在数据库删除前完整完成。
use anyhow::{Context, Result};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::str;
use thiserror::Error;
use uc_core::clipboard::PersistedClipboardRepresentation;
use uc_core::error_class::ErrorClass;
use url::Url;

pub(super) struct CacheFiles {
    pub(super) root: PathBuf,
    pub(super) paths: Vec<PathBuf>,
}

#[derive(Debug, Error)]
pub(super) enum CacheOwnershipRefusal {
    #[error("history cache reference contains parent traversal")]
    ParentTraversal,
    #[error("history cache reference contains a symbolic link")]
    SymbolicLink,
    #[error("history cache reference escapes its managed root")]
    OutsideRoot,
}

impl ErrorClass for CacheOwnershipRefusal {
    fn class(&self) -> &'static str {
        match self {
            Self::ParentTraversal => "parent_traversal",
            Self::SymbolicLink => "symbolic_link",
            Self::OutsideRoot => "outside_root",
        }
    }
}

impl CacheFiles {
    pub(super) async fn verify(
        representations: &[PersistedClipboardRepresentation],
        cache_root: &Path,
    ) -> Result<Option<Self>> {
        let mut candidates = Vec::new();
        for representation in representations {
            if !representation
                .mime_type
                .as_ref()
                .is_some_and(|mime| mime.as_str().contains("uri-list"))
            {
                continue;
            }
            let Some(inline) = &representation.inline_data else {
                continue;
            };
            for line in str::from_utf8(inline)
                .context("decode history file references")?
                .lines()
            {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let path = if line.starts_with("file://") {
                    let url = Url::parse(line).context("parse history file reference URI")?;
                    // discarded-source[no-information]: 非本机 file URI 不能指向本机缓存；unit 不含诊断信息。
                    let Ok(local_path) = url.to_file_path() else {
                        continue;
                    };
                    local_path
                } else {
                    PathBuf::from(line)
                };
                if let Ok(relative) = path.strip_prefix(cache_root) {
                    // 兼容历史原始路径，但不能让 .. 绕过受管根。
                    if relative
                        .components()
                        .any(|part| part == Component::ParentDir)
                    {
                        return Err(CacheOwnershipRefusal::ParentTraversal.into());
                    }
                    candidates.push(relative.to_path_buf());
                }
            }
        }
        if candidates.is_empty() {
            return Ok(None);
        }
        let root = match tokio::fs::canonicalize(cache_root).await {
            Ok(root) => root,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(source).context("resolve managed history cache root"),
        };
        let mut paths = Vec::new();
        for relative in candidates {
            let mut current = root.clone();
            let mut missing = false;
            for component in relative.components() {
                current.push(component);
                match tokio::fs::symlink_metadata(&current).await {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(CacheOwnershipRefusal::SymbolicLink.into());
                    }
                    Ok(_) => {}
                    Err(source) if source.kind() == ErrorKind::NotFound => {
                        missing = true;
                        break;
                    }
                    Err(source) => {
                        return Err(source).context("inspect managed history cache reference");
                    }
                }
            }
            if missing {
                continue;
            }
            let actual = tokio::fs::canonicalize(&current)
                .await
                .context("resolve managed history cache reference")?;
            if actual == root || !actual.starts_with(&root) {
                return Err(CacheOwnershipRefusal::OutsideRoot.into());
            }
            if !paths.contains(&actual) {
                paths.push(actual);
            }
        }
        Ok(Some(Self { root, paths }))
    }
}

#[cfg(test)]
mod tests {
    use super::CacheFiles;
    use std::path::Path;
    use std::str::Utf8Error;
    use uc_core::clipboard::{MimeType, PersistedClipboardRepresentation};
    use uc_core::ids::{FormatId, RepresentationId};
    use url::ParseError;

    #[tokio::test]
    async fn file_reference_decode_and_url_parse_failures_retain_original_sources() {
        for bytes in [vec![0xff], b"file://[".to_vec()] {
            let malformed_utf8 = bytes.len() == 1;
            let representation = PersistedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("files"),
                Some(MimeType("text/uri-list".into())),
                bytes.len() as i64,
                Some(bytes),
                None,
            );
            let Err(failure) =
                CacheFiles::verify(&[representation], Path::new("unused-cache")).await
            else {
                panic!("invalid persisted reference must retain its failure");
            };
            if malformed_utf8 {
                assert!(failure.downcast_ref::<Utf8Error>().is_some());
            } else {
                assert!(failure.downcast_ref::<ParseError>().is_some());
            }
            assert!(failure.source().is_some());
        }
    }
}
