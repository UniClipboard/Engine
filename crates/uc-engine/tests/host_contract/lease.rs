use std::fs::{read_dir, File, OpenOptions};
use std::path::{Path, PathBuf};

pub(super) fn find_lease(root: &Path) -> Option<PathBuf> {
    for entry in read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if let Some(path) = find_lease(&path) {
                return Some(path);
            }
        } else if path.file_name().unwrap() == "profile-content-key-vault.lease" {
            return Some(path);
        }
    }
    None
}

pub(super) fn open_lease(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}
