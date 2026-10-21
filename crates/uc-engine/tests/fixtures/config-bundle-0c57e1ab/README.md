# Config bundle exported before host declarations

A real `.ucbundle` exported by Engine `0c57e1aba10237549c4f8584eebac5bc10b2b03f` (task7 final, before
host-declared configuration files) through `Engine::start` → `CreateSpace` → `ExportConfig`.
All data is synthetic; the passphrase is `synthetic-legacy-passphrase`.

- `source-*.json`: the files placed at the source data root before export. That Engine carried only
  `last_notified_update.json` and `skipped_version.json` (as `ui-state/<name>` members); the other four
  files were never exported.
- `SHA256SUMS`: hashes of every file here; the bundle bytes must never change.

Historical checkpoint tests used this bundle to prove file mapping while recording failed public startup.
The bundle omits independent V3 control state and cannot restore a complete installation.
The user approved explicit rejection before target mutation and a requirement to re-export from the
retained source installation. `tests/host_configuration_files_e2e.rs` now verifies that public result,
target preservation and subsequent target startup. The bundle and input bytes remain unchanged.
