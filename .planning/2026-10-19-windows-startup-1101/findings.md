# Findings

## Evidence from Desktop t-0328 (read-only)
- A3/A7: `profile storage upgrade step failed` error_kind=corrupt, upgrade_phase=verified,
  upgrade_action=recover_active_target, target_activated=true. 4 ms after new daemon claimed profile.
- Init daemon released the Desktop profile claim (~12.005) while its process lived ~5 s longer.
- Exact SQLite result not captured.

## Code
- validation.rs `open_connection`: bare `SqliteConnection::establish` (no busy_timeout); every query error -> Corrupt.
- target.rs `ensure_tables_empty`: same pattern, also on the verify_promoted path.
- Repo primitives: `uc_infra_storage::db::connection::{establish_waiting, is_lock_contention}` (5 s busy wait).
- Engine has no cross-process profile exclusion before storage upgrade (vault lease is taken later at unlock).
- Storage -> StorageUnavailable retryable=true; Corrupt -> CorruptData retryable=false.

## Failure modes to cover (written before tests)
1. SQLITE_BUSY on first read (integrity_check) while another process closes its last WAL connection
   (checkpoint + WAL reset under exclusive lock).
2. SQLITE_BUSY during WAL/shm recovery by another opener.
3. Busy/locked on foreign_key_check, ensure_tables_empty COUNT, sqlite_master reads, schema_digest.
4. Lock held longer than the busy wait -> must fail as retryable storage, journal unchanged.
5. Real corruption: NOTADB (garbage file), malformed page, integrity_check != ok, FK violation -> Corrupt.
6. Rows in the wrong store, missing owned table, schema fingerprint mismatch -> Corrupt.
7. Failed verification must not advance the journal (no mark_promoted/save_journal); restart succeeds.
8. Open failure for reasons other than lock (missing file / permission) -> must not become lock class.
9. Logging must not include SQLite message text, paths, or content.
