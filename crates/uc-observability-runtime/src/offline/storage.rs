//! Encrypted history reload and bounded ciphertext evidence. No file key or plaintext fallback.

use super::{unix_ms, Entry, OfflineHistory, MAX_BYTES, MAX_RECORD_BYTES, RETENTION};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::PoisonError;
use std::time::{Duration, Instant};

const MAX_ENCRYPTED_BYTES: usize = 8 * 1024 * 1024;

/// Protection callbacks use the active profile cipher. WouldBlock means unavailable keys;
/// InvalidData from open means confirmed unreadable ciphertext, never missing key material.
pub struct OfflineHistoryStorage {
    pub directory: PathBuf,
    pub seal: Box<dyn Fn(&[u8]) -> io::Result<Vec<u8>> + Send + Sync>,
    pub open: Box<dyn Fn(&[u8]) -> io::Result<Vec<u8>> + Send + Sync>,
    loaded: bool,
}
impl OfflineHistoryStorage {
    pub fn new(
        directory: PathBuf,
        seal: Box<dyn Fn(&[u8]) -> io::Result<Vec<u8>> + Send + Sync>,
        open: Box<dyn Fn(&[u8]) -> io::Result<Vec<u8>> + Send + Sync>,
    ) -> Self {
        Self {
            directory,
            seal,
            open,
            loaded: false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RecoveryRecord {
    reason: String,
    original_bytes: u64,
    retained_bytes: usize,
    truncated: bool,
    incidents: u64,
}

#[derive(Serialize, Deserialize)]
struct StoredHistory {
    version: u32,
    entries: Vec<(u64, usize, String)>,
    evicted: u64,
    rejected: u64,
    persistence_trimmed: u64,
    #[serde(default)]
    recovery: Option<RecoveryRecord>,
}

struct Damage {
    ciphertext: Vec<u8>,
    original_bytes: u64,
    reason: &'static str,
}

enum Reload {
    Missing,
    Saved(StoredHistory),
    Damaged(Damage),
}

fn reload(storage: &OfflineHistoryStorage, file: &Path) -> io::Result<Reload> {
    let input = match std::fs::File::open(file) {
        Ok(input) => input,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Reload::Missing),
        Err(error) => return Err(error),
    };
    let original_bytes = input.metadata()?.len();
    let mut encrypted = Vec::new();
    input
        .take(MAX_ENCRYPTED_BYTES as u64 + 1)
        .read_to_end(&mut encrypted)?;
    let damaged = |reason| {
        Reload::Damaged(Damage {
            ciphertext: encrypted[..encrypted.len().min(MAX_ENCRYPTED_BYTES)].to_vec(),
            original_bytes,
            reason,
        })
    };
    if encrypted.len() > MAX_ENCRYPTED_BYTES {
        return Ok(damaged("history_oversized"));
    }
    let clear = match (storage.open)(&encrypted) {
        Ok(clear) => clear,
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            return Ok(damaged("invalid_ciphertext"));
        }
        Err(error) => return Err(error),
    };
    let saved = match serde_json::from_slice::<StoredHistory>(&clear) {
        Ok(saved) => saved,
        // discarded-source[input-validation]: parse detail cannot change the bounded recovery decision
        Err(_) => return Ok(damaged("invalid_history_json")),
    };
    if saved.version != 1 {
        return Ok(damaged("unsupported_history_format"));
    }
    Ok(Reload::Saved(saved))
}

fn atomic_write(directory: &Path, file: &Path, bytes: &[u8]) -> io::Result<()> {
    std::fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(file).map_err(|error| error.error)?;
    Ok(())
}

impl OfflineHistory {
    pub(super) fn persist(&self) {
        let mut slot = self.storage.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(storage) = slot.as_mut() else { return };
        let result = self.persist_with(storage);
        *self
            .persistence
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = match result {
            Ok(()) => "encrypted",
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => "key_unavailable",
            Err(_) => "io_failure",
        };
    }

    fn persist_with(&self, storage: &mut OfflineHistoryStorage) -> io::Result<()> {
        let file = storage.directory.join("history.enc");
        let mut damage = None;
        if !storage.loaded {
            match reload(storage, &file)? {
                Reload::Missing => storage.loaded = true,
                Reload::Damaged(value) => damage = Some(value),
                Reload::Saved(saved) => {
                    let mut history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
                    history.evicted = history
                        .evicted
                        .saturating_add(saved.evicted)
                        .saturating_add(saved.persistence_trimmed);
                    history.rejected = history.rejected.saturating_add(saved.rejected);
                    *self.recovery.lock().unwrap_or_else(PoisonError::into_inner) = saved.recovery;
                    let now = unix_ms();
                    for (wall_ms, signal, line) in saved.entries.into_iter().rev() {
                        let age = Duration::from_millis(now.saturating_sub(wall_ms));
                        if age > RETENTION
                            || signal > 2
                            || line.len() > MAX_RECORD_BYTES
                            || history.bytes + line.len() > MAX_BYTES
                        {
                            history.evicted += 1;
                            continue;
                        }
                        history.bytes += line.len();
                        history.entries.push_front(Entry {
                            at: Instant::now().checked_sub(age).unwrap_or_else(Instant::now),
                            wall_ms,
                            signal,
                            line,
                        });
                    }
                    storage.loaded = true;
                }
            }
        }
        let previous = self
            .recovery
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let recovery = damage
            .as_ref()
            .map(|damage| RecoveryRecord {
                reason: damage.reason.into(),
                original_bytes: damage.original_bytes,
                retained_bytes: damage.ciphertext.len(),
                truncated: damage.original_bytes > damage.ciphertext.len() as u64,
                incidents: previous
                    .as_ref()
                    .map_or(1, |previous| previous.incidents.saturating_add(1)),
            })
            .or(previous);
        let mut entries = Vec::new();
        let mut bytes = 0;
        let mut trimmed = 0;
        let (evicted, rejected);
        {
            let history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
            evicted = history.evicted;
            rejected = history.rejected;
            for entry in history.entries.iter().rev() {
                if entry.at.elapsed() > RETENTION || bytes + entry.line.len() > 512 * 1024 {
                    trimmed += 1;
                    continue;
                }
                bytes += entry.line.len();
                entries.push((entry.wall_ms, entry.signal, entry.line.clone()));
            }
        }
        self.persistence_trimmed.store(trimmed, Ordering::Release);
        entries.reverse();
        let clear = serde_json::to_vec(&StoredHistory {
            version: 1,
            entries,
            evicted,
            rejected,
            persistence_trimmed: trimmed,
            recovery: recovery.clone(),
        })?;
        // Prove that the active profile can protect new history before touching either evidence slot.
        // Even invalid framing is not permission to overwrite a file while the profile is locked.
        let encrypted = (storage.seal)(&clear)?;
        if encrypted.len() > MAX_ENCRYPTED_BYTES {
            return Err(io::Error::other(
                "encrypted diagnostic history exceeds quota",
            ));
        }
        if let Some(damage) = damage {
            // One atomic, bounded evidence slot. Never remove the original before evidence is durable.
            atomic_write(
                &storage.directory,
                &storage.directory.join("history.quarantine.enc"),
                &damage.ciphertext,
            )?;
        }
        atomic_write(&storage.directory, &file, &encrypted)?;
        storage.loaded = true;
        *self.recovery.lock().unwrap_or_else(PoisonError::into_inner) = recovery;
        Ok(())
    }
}
