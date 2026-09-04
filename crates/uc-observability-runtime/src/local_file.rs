use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{Days, NaiveDate, Utc};
use tracing_subscriber::fmt::MakeWriter;

use crate::{LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS};

#[derive(Clone)]
pub(crate) struct BoundedDailyMakeWriter {
    state: Arc<Mutex<WriterState>>,
    dropped_records: Arc<AtomicU64>,
}

impl BoundedDailyMakeWriter {
    pub(crate) fn new(directory: &Path) -> io::Result<(Self, Arc<AtomicU64>)> {
        std::fs::create_dir_all(directory)?;
        let today = Utc::now().date_naive();
        cleanup(
            directory,
            today,
            LOCAL_LOG_RETENTION_DAYS,
            LOCAL_LOG_MAX_BYTES,
        )?;
        let state = WriterState::open(directory.to_path_buf(), today)?;
        let dropped_records = Arc::new(AtomicU64::new(0));
        Ok((
            Self {
                state: Arc::new(Mutex::new(state)),
                dropped_records: Arc::clone(&dropped_records),
            },
            dropped_records,
        ))
    }
}

impl<'writer> MakeWriter<'writer> for BoundedDailyMakeWriter {
    type Writer = BoundedDailyWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        BoundedDailyWriter {
            owner: self.clone(),
        }
    }
}

impl Write for BoundedDailyMakeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        BoundedDailyWriter {
            owner: self.clone(),
        }
        .write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        BoundedDailyWriter {
            owner: self.clone(),
        }
        .flush()
    }
}

pub(crate) struct BoundedDailyWriter {
    owner: BoundedDailyMakeWriter,
}

impl Write for BoundedDailyWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut state = lock(&self.owner.state);
        state.rotate_if_needed()?;
        let additional = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if state.total_bytes.saturating_add(additional) > LOCAL_LOG_MAX_BYTES {
            self.owner.dropped_records.fetch_add(1, Ordering::Relaxed);
            return Ok(buffer.len());
        }
        state.file.write_all(buffer)?;
        state.total_bytes = state.total_bytes.saturating_add(additional);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        lock(&self.owner.state).file.flush()
    }
}

struct WriterState {
    directory: PathBuf,
    date: NaiveDate,
    file: File,
    total_bytes: u64,
}

impl WriterState {
    fn open(directory: PathBuf, date: NaiveDate) -> io::Result<Self> {
        let path = directory.join(file_name(date));
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let total_bytes =
            managed_files(&directory)?
                .into_iter()
                .try_fold(0_u64, |total, entry| {
                    entry
                        .path
                        .metadata()
                        .map(|metadata| total.saturating_add(metadata.len()))
                })?;
        Ok(Self {
            directory,
            date,
            file,
            total_bytes,
        })
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let today = Utc::now().date_naive();
        if today == self.date {
            return Ok(());
        }
        cleanup(
            &self.directory,
            today,
            LOCAL_LOG_RETENTION_DAYS,
            LOCAL_LOG_MAX_BYTES,
        )?;
        *self = Self::open(self.directory.clone(), today)?;
        Ok(())
    }
}

#[derive(Debug)]
struct ManagedFile {
    date: NaiveDate,
    path: PathBuf,
}

fn managed_files(directory: &Path) -> io::Result<Vec<ManagedFile>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(date) = parse_file_name(&name) else {
            continue;
        };
        files.push(ManagedFile {
            date,
            path: entry.path(),
        });
    }
    files.sort_by_key(|entry| entry.date);
    Ok(files)
}

fn cleanup(
    directory: &Path,
    today: NaiveDate,
    retention_days: u64,
    max_bytes: u64,
) -> io::Result<()> {
    let oldest = today
        .checked_sub_days(Days::new(retention_days.saturating_sub(1)))
        .unwrap_or(NaiveDate::MIN);
    for entry in managed_files(directory)? {
        if entry.date < oldest {
            std::fs::remove_file(entry.path)?;
        }
    }

    let files = managed_files(directory)?;
    let mut total = files.iter().try_fold(0_u64, |total, entry| {
        entry
            .path
            .metadata()
            .map(|metadata| total.saturating_add(metadata.len()))
    })?;
    for entry in files {
        if total <= max_bytes {
            break;
        }
        let size = entry.path.metadata()?.len();
        std::fs::remove_file(entry.path)?;
        total = total.saturating_sub(size);
    }
    Ok(())
}

pub fn managed_log_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    managed_files(directory).map(|files| files.into_iter().map(|entry| entry.path).collect())
}

fn file_name(date: NaiveDate) -> String {
    format!("engine.{}.jsonl", date.format("%Y-%m-%d"))
}

fn parse_file_name(name: &str) -> Option<NaiveDate> {
    let date = name.strip_prefix("engine.")?.strip_suffix(".jsonl")?;
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cleanup_removes_expired_and_oldest_managed_files_only() {
        let directory = tempdir().expect("temp dir");
        let today = NaiveDate::from_ymd_opt(2026, 9, 4).expect("date");
        std::fs::write(directory.path().join("engine.2026-08-28.jsonl"), b"old").expect("old");
        std::fs::write(directory.path().join("engine.2026-09-01.jsonl"), b"12345").expect("first");
        std::fs::write(directory.path().join("engine.2026-09-02.jsonl"), b"67890").expect("second");
        std::fs::write(directory.path().join("keep.txt"), b"private host file").expect("unmanaged");

        cleanup(directory.path(), today, 7, 5).expect("cleanup");

        assert!(!directory.path().join("engine.2026-08-28.jsonl").exists());
        assert!(!directory.path().join("engine.2026-09-01.jsonl").exists());
        assert!(directory.path().join("engine.2026-09-02.jsonl").exists());
        assert!(directory.path().join("keep.txt").exists());
    }

    #[test]
    fn managed_file_enumeration_ignores_similar_and_nested_names() {
        let directory = tempdir().expect("temp dir");
        std::fs::write(directory.path().join("engine.2026-09-04.jsonl"), b"{}").expect("managed");
        std::fs::write(directory.path().join("engine.latest.jsonl"), b"{}").expect("similar");
        std::fs::create_dir(directory.path().join("engine.2026-09-03.jsonl")).expect("directory");

        let files = managed_log_files(directory.path()).expect("enumeration");
        assert_eq!(
            files,
            vec![directory.path().join("engine.2026-09-04.jsonl")]
        );
    }
}
