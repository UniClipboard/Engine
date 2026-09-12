use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
pub use uc_observability_contract::diagnostics::connectivity::LocalDiagnosticSource;

#[derive(Debug, Clone, Serialize)]
pub struct FileSourceCounts {
    pub source: LocalDiagnosticSource,
    pub accepted_count: u64,
    pub written_count: u64,
    pub queue_dropped_count: u64,
    pub quota_dropped_count: u64,
    pub write_failed_count: u64,
    pub last_written_at_ms: Option<u64>,
}

#[derive(Default)]
pub(crate) struct SourceCounters {
    pub(crate) accepted: AtomicU64,
    pub(crate) written: AtomicU64,
    pub(crate) queue_dropped: AtomicU64,
    pub(crate) quota_dropped: AtomicU64,
    pub(crate) write_failed: AtomicU64,
    pub(crate) last_written_at_ms: AtomicU64,
}

pub(crate) struct FileStatistics([SourceCounters; LocalDiagnosticSource::ALL.len()]);
impl Default for FileStatistics {
    fn default() -> Self {
        Self(std::array::from_fn(|_| SourceCounters::default()))
    }
}

pub(crate) fn increment(counter: &AtomicU64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
        Some(count.saturating_add(1))
    });
}

impl FileStatistics {
    pub(crate) fn source(&self, source: LocalDiagnosticSource) -> &SourceCounters {
        &self.0[source as usize]
    }
    pub(crate) fn snapshot(&self) -> Vec<FileSourceCounts> {
        LocalDiagnosticSource::ALL
            .into_iter()
            .map(|source| {
                let counts = self.source(source);
                FileSourceCounts {
                    source,
                    accepted_count: counts.accepted.load(Ordering::Relaxed),
                    written_count: counts.written.load(Ordering::Relaxed),
                    queue_dropped_count: counts.queue_dropped.load(Ordering::Relaxed),
                    quota_dropped_count: counts.quota_dropped.load(Ordering::Relaxed),
                    write_failed_count: counts.write_failed.load(Ordering::Relaxed),
                    last_written_at_ms: match counts.last_written_at_ms.load(Ordering::Relaxed) {
                        0 => None,
                        timestamp => Some(timestamp),
                    },
                }
            })
            .collect()
    }
}
