use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use blake3::Hasher;
use tar::{Archive, Builder, EntryType, Header};
use zeroize::Zeroizing;

use super::error::invalid_archive;
use super::{private_new_file, sync_directory, ProfileBackupSource};

const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;
const MAX_SOURCE_BYTES: u64 = 4096;
const MAX_PATH_BYTES: usize = 4096;
const MAX_INVENTORY_BYTES: usize = 16 * 1024 * 1024;

pub(in super::super) fn write_selected_tree<W: Write>(
    output: W,
    root: &Path,
    source: &ProfileBackupSource,
    excluded: &[PathBuf],
) -> io::Result<(W, [u8; 32])> {
    let mut builder = Builder::new(HashingIo::new(output));
    let metadata = Zeroizing::new(serde_json::to_vec(source).map_err(io::Error::other)?);
    if metadata.len() as u64 > MAX_SOURCE_BYTES {
        return Err(invalid_archive());
    }
    append(
        &mut builder,
        "source.json".as_ref(),
        metadata.len() as u64,
        EntryType::Regular,
        metadata.as_slice(),
    )?;
    let mut budget = InventoryBudget::default();
    budget.admit(Path::new("data"))?;
    match fs::symlink_metadata(root) {
        Ok(_) => append_tree(&mut builder, root, Path::new("data"), &mut budget, excluded)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // 仅缓存待处理内容存在时，主资料目录可能尚未创建；归档空根而不改动来源。
            append(
                &mut builder,
                Path::new("data"),
                0,
                EntryType::Directory,
                io::empty(),
            )?;
        }
        Err(error) => return Err(error),
    }
    let hashed = builder.into_inner()?;
    Ok((hashed.inner, *hashed.hasher.finalize().as_bytes()))
}

fn append_tree<W: Write>(
    builder: &mut Builder<W>,
    source: &Path,
    relative: &Path,
    budget: &mut InventoryBudget,
    excluded: &[PathBuf],
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        append(builder, relative, 0, EntryType::Directory, io::empty())?;
        let mut children = Vec::new();
        for child in fs::read_dir(source)? {
            let child = child?;
            if excluded.contains(&child.path()) {
                continue;
            }
            // 在保留待遍历项时扣除预算，避免深层目录累计持有多个满额列表。
            budget.admit(&relative.join(child.file_name()))?;
            children.push(child);
        }
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            append_tree(
                builder,
                &child.path(),
                &relative.join(child.file_name()),
                budget,
                excluded,
            )?;
        }
    } else if metadata.is_file() {
        let mut file = open_regular_file(source)?;
        let before = file.metadata()?;
        append(
            builder,
            relative,
            before.len(),
            EntryType::Regular,
            &mut file,
        )?;
        let after = file.metadata()?;
        if before.len() != after.len() || before.modified()? != after.modified()? {
            return Err(io::Error::other(
                "profile backup source changed during read",
            ));
        }
    } else {
        return Err(invalid_archive());
    }
    Ok(())
}

fn append<W: Write>(
    builder: &mut Builder<W>,
    path: &Path,
    size: u64,
    kind: EntryType,
    input: impl Read,
) -> io::Result<()> {
    let mut header = Header::new_gnu();
    header.set_size(size);
    header.set_entry_type(kind);
    header.set_mode(if kind.is_dir() { 0o700 } else { 0o600 });
    header.set_mtime(0);
    header.set_cksum();
    builder.append_data(&mut header, path, input)
}

pub(super) fn read_tree<R: Read>(
    input: R,
    destination: Option<&Path>,
) -> io::Result<(ProfileBackupSource, [u8; 32])> {
    let mut archive = Archive::new(HashingIo::new(input));
    let mut entries = archive.entries()?;
    let mut first = entries.next().ok_or_else(invalid_archive)??;
    if first.path()?.as_ref() != Path::new("source.json")
        || !first.header().entry_type().is_file()
        || first.size() > MAX_SOURCE_BYTES
    {
        return Err(invalid_archive());
    }
    let mut bytes = Zeroizing::new(Vec::new());
    first.read_to_end(&mut bytes)?;
    let source = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    drop(first);
    let mut seen = BTreeMap::new();
    let mut budget = InventoryBudget::default();
    let mut directories = Vec::new();
    for entry in entries {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        budget.admit(&path)?;
        let kind = entry.header().entry_type();
        if seen.insert(path.clone(), kind.is_dir()).is_some() {
            return Err(invalid_archive());
        }
        if !(kind.is_file() || kind.is_dir()) || (kind.is_dir() && entry.size() != 0) {
            return Err(invalid_archive());
        }
        let relative = path.strip_prefix("data").map_err(io::Error::other)?;
        if relative.as_os_str().is_empty() && !kind.is_dir() {
            return Err(invalid_archive());
        }
        if !relative.as_os_str().is_empty()
            && seen.get(path.parent().ok_or_else(invalid_archive)?) != Some(&true)
        {
            return Err(invalid_archive());
        }
        if let Some(root) = destination {
            let target = root.join(relative);
            if kind.is_dir() {
                if !relative.as_os_str().is_empty() {
                    create_private_directory_new(&target)?;
                }
                directories.push(target);
            } else {
                let mut file = private_new_file(&target)?;
                io::copy(&mut entry, &mut file)?;
                file.sync_all()?;
            }
        } else {
            io::copy(&mut entry, &mut io::sink())?;
        }
    }
    if seen.get(Path::new("data")) != Some(&true) {
        return Err(invalid_archive());
    }
    // tar 在零块处结束，仍消费全部尾部，使截断、追加也参与摘要校验。
    let mut hashed = archive.into_inner();
    let mut trailing = [0; 8192];
    loop {
        let count = hashed.read(&mut trailing)?;
        if count == 0 {
            break;
        }
        if trailing[..count].iter().any(|byte| *byte != 0) {
            return Err(invalid_archive());
        }
    }
    for directory in directories.iter().rev() {
        sync_directory(directory)?;
    }
    Ok((source, *hashed.hasher.finalize().as_bytes()))
}

#[derive(Default)]
struct InventoryBudget {
    entries: usize,
    path_bytes: usize,
}

impl InventoryBudget {
    fn admit(&mut self, path: &Path) -> io::Result<()> {
        validate_member(path)?;
        self.entries += 1;
        self.path_bytes += path.as_os_str().len();
        if self.entries > MAX_ENTRIES || self.path_bytes > MAX_INVENTORY_BYTES {
            return Err(invalid_archive());
        }
        Ok(())
    }
}

fn validate_member(path: &Path) -> io::Result<()> {
    if path.as_os_str().len() > MAX_PATH_BYTES {
        return Err(invalid_archive());
    }
    let mut components = path.components();
    if components.next() != Some(Component::Normal("data".as_ref())) {
        return Err(invalid_archive());
    }
    let mut depth = 0;
    for component in components {
        depth += 1;
        let Component::Normal(value) = component else {
            return Err(invalid_archive());
        };
        let value = value.to_str().ok_or_else(invalid_archive)?;
        if depth > MAX_DEPTH || value.contains(['\\', ':']) || value.chars().any(char::is_control) {
            return Err(invalid_archive());
        }
    }
    Ok(())
}

pub(super) fn require_directory(path: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(invalid_archive());
    }
    Ok(())
}

pub(in super::super) fn resolve_source_root(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::canonicalize(path),
        Ok(_) => Err(invalid_archive()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(invalid_archive)?;
            require_directory(parent)?;
            Ok(fs::canonicalize(parent)?.join(path.file_name().ok_or_else(invalid_archive)?))
        }
        Err(error) => Err(error),
    }
}

pub(in super::super) fn require_disjoint_destination(
    source: &Path,
    destination: &Path,
) -> io::Result<()> {
    if !destination.is_absolute()
        || destination
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(invalid_archive());
    }
    let mut parent = destination;
    loop {
        match fs::symlink_metadata(parent) {
            Ok(metadata) => {
                if !metadata.is_dir() {
                    return Err(invalid_archive());
                }
                let resolved = fs::canonicalize(parent)?;
                let suffix = destination.strip_prefix(parent).map_err(io::Error::other)?;
                let final_path = resolved.join(suffix);
                if final_path.starts_with(source) || source.starts_with(&final_path) {
                    return Err(invalid_archive());
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or_else(invalid_archive)?;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(in super::super) fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = private_directory_builder();
    builder.recursive(true).create(path)?;
    require_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)?;
        // 已存在的共享目录不能被当作私有副本目录，也不擅自修改调用方目录权限。
        if metadata.mode() & 0o077 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
    }
    Ok(())
}

pub(super) fn create_private_directory_new(path: &Path) -> io::Result<()> {
    private_directory_builder().create(path)
}

fn private_directory_builder() -> DirBuilder {
    let mut builder = DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

pub(in super::super) fn open_regular_file(path: &Path) -> io::Result<File> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid_archive());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid_archive());
    }
    Ok(file)
}

struct HashingIo<T> {
    inner: T,
    hasher: Hasher,
}

impl<T> HashingIo<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            hasher: Hasher::new(),
        }
    }
}

impl<W: Write> Write for HashingIo<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let count = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<R: Read> Read for HashingIo<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.inner.read(bytes)?;
        self.hasher.update(&bytes[..count]);
        Ok(count)
    }
}
