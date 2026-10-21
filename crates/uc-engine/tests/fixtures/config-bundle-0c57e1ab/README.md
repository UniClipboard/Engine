# Config bundle exported before host declarations

A real `.ucbundle` exported by Engine `0c57e1aba10237549c4f8584eebac5bc10b2b03f` (task7 final, before
host-declared configuration files) through `Engine::start` → `CreateSpace` → `ExportConfig`.
All data is synthetic; the passphrase is `synthetic-legacy-passphrase`.

- `source-*.json`: the files placed at the source data root before export. That Engine carried only
  `last_notified_update.json` and `skipped_version.json` (as `ui-state/<name>` members); the other four
  files were never exported.
- `SHA256SUMS`: hashes of every file here; the bundle bytes must never change.

Used by `tests/host_configuration_files_e2e.rs` to prove old bundles restore to `<data_root>/<name>`.
