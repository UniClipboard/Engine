# Task 8: host-declared configuration files

Base: task7 final `0c57e1aba10237549c4f8584eebac5bc10b2b03f` (Engine main `1c0660d9` + task7 stack).
Branch `hp/uni/t-0366-8`. Design: t0355 `desktop-product-settings-design.md` §3.3, §7.3, §8, §9 (D4), D5, D6.

## Problem

Engine hard-codes three Desktop updater file names (`last_notified_update.json`, `skipped_version.json`,
`update_prompt_throttle.json`) in `AppPaths`, config export and factory reset. Facts found before any change:

- Export carries only two of the three (`update_prompt_throttle.json` is never exported).
- Import restores `ui-state/<name>` members into `<data_root>/ui-state/<name>`. The host reads
  `<data_root>/<name>`, so no exported updater state has ever reached the host after import.
- Upgrade backup already captures the whole data root except an explicit exclusion list, so any host
  file at the data root (declared or not, including `desktop-telemetry.json`) is already in the backup.
- Factory reset deletes the three names with `remove_dir_all`-capable removal.

## Owner and outcome

- Rules: Core `HostConfigurationFiles` (validated set of basenames), next to `AppPaths`.
- Declaration: host passes names through `EngineConfig::with_host_configuration_files`; Engine validates at
  start. No product values, DTOs or host paths enter Engine; only basenames resolved under the data root.
- Export/import: Infra `config_migration` (single owner of bundle format and staging apply).
- Reset: Infra `ProfileStateCleaner` inside the existing journaled `KeysWiped` phase.
- Backup: unchanged code; existing whole-root capture already includes declared files.
- Caller action: declare names once in `EngineConfig`. No new operation, result or event.
- Success: declared files travel in the bundle and land at `<data_root>/<name>` after the boot apply;
  reset removes them after keys are wiped; backup contains them.
- Failure: invalid declaration fails `Engine::start` with `InvalidInput` code 1109 before any I/O.
  Export of a declared path that is a symlink or a directory, or above the size limit, fails the export.
  Apply failure keeps the marker and staging; the next boot re-applies everything (existing contract).
  Reset failure keeps the persisted reset phase; the next `FactoryResetSpace` or start resumes it.
- Restart/retry: unchanged owners (boot apply, factory-reset journal).

## Contract

- Name grammar: 1–64 bytes, `[a-z0-9][a-z0-9._-]*\.json`, no `..`. One path component only, so no separator,
  no absolute path, no leading dot, no directory.
- Reserved (Engine-owned or host runtime) names are rejected: `settings.json`, `pending-import.json`,
  `upgrade-cursor.json`, `first-sync-state.json`, `daemon-run.json`, `daemon-last-exit.json`.
- Duplicates are rejected (a host bug, not a silent merge).
- Empty declaration is valid: Engine then exports, restores and resets no host file (Mobile default).
- Bundle member: `ui-state/<name>` (existing prefix kept). No manifest or staging schema change:
  `MANIFEST_SCHEMA_VER` and `PENDING_IMPORT_SCHEMA_VER` stay 1. No new persisted format version.
- Apply restores `ui-state/<name>` only when `<name>` is in the importing host's current declaration.
  Undeclared members are ignored (count logged). The imported manifest never selects a destination.
- Apply writes `<data_root>/.<name>.import-tmp` then renames over `<data_root>/<name>`: per-file atomic and
  a destination symlink is replaced, not followed.
- Declared member absent from the bundle: the live file is left unchanged (same as `settings.json`).
- Export: absent declared file is skipped; a regular file is carried; a symlink or directory fails the
  export (following a symlink would bundle an arbitrary host file); a file over 1 MiB fails the export.
- Reset: declared paths are removed as files only (a symlink is unlinked, a directory fails the reset),
  then verified absent together with Engine state.

## D5 interpretation (needs confirmation)

D5 says the daemon deletes the product file after Engine reset succeeds. D6 puts reset under the host
declaration. To avoid two deleters: Engine deletes declared files as part of the reset state clear,
after key wipe and with Engine state, and reports success only after they are verified absent. A crash in
between is replayed by the reset journal, which a daemon-side delete after success cannot do. Desktop
task10 must therefore not delete the declared files again; it reloads its in-memory product store after
`SpaceFactoryReset`. Undeclared files (`desktop-telemetry.json`) are never touched.

## Old bundles

Bundles exported before this change carry `ui-state/last_notified_update.json` and
`ui-state/skipped_version.json` and never `update_prompt_throttle.json` or the product file. With the
mapping above they restore to `<data_root>/<name>` when the importing host declares those names. Evidence
uses a real bundle exported on `0c57e1ab` (committed fixture). Old Engines importing a new bundle keep the
pre-existing behaviour (members land in `<data_root>/ui-state/`), which is harmless. Existing dead
`<data_root>/ui-state/` directories from earlier imports are not cleaned up.

## Failure modes (written before code and before isolated checks)

H1 Declared name contains a separator, `..`, is absolute, starts with a dot, is empty or too long → path
   escape outside the data root on export, apply or reset.
H2 Declared name collides with an Engine-owned file (`settings.json`, `pending-import.json`, …) → host file
   overwrites Engine state on import or Engine state is double-handled on reset.
H3 Duplicate names → duplicate archive members or double delete.
H4 Imported bundle carries `ui-state/<x>` for an undeclared `<x>` (untrusted manifest) → arbitrary file
   written into the data root.
H5 Restore lands in `<data_root>/ui-state/` instead of `<data_root>/<name>` (existing defect) → host never
   sees imported state.
H6 Declared file missing at export → export fails, or an empty member is written.
H7 Declared path is a symlink at export → bundle carries a file outside the data root.
H8 Declared path is a symlink at apply destination → `fs::copy` writes through it outside the data root.
H9 Declared path is a directory or symlink at reset → `remove_dir_all` deletes more than the declaration.
H10 Copy failure halfway through apply → staging deleted with live files half restored; no retry.
H11 Key wipe fails during reset → declared host files already deleted though reset did not proceed.
H12 Telemetry or unlisted host files exported, restored or deleted → consent reset or ownership broadened.
H13 Old bundle (no throttle member, no product member) → import fails, or old members silently ignored
    while support is claimed.
H14 Declaration leaks names into logs or `Debug` output.
H15 Bindings (UniFFI, NAPI) break because `EngineConfig` changed shape.
H16 Upgrade backup stops containing declared files (e.g. a new exclusion) or starts depending on the
    declaration.
H17 Declared file larger than the read bound → unbounded memory in export.

## Checks

E2E (public `Engine::start` + operations, real files, isolated temp roots, synthetic data):

1. Source with declared + telemetry + unlisted files exports; target stages, restarts; declared files are
   byte-identical at `<data_root>/<name>`; telemetry/unlisted target files unchanged; no `ui-state/` dir.
   Staging inventory and hashes recorded (H4, H5, H6, H12).
2. Bundle with an undeclared member: not restored (H4) — covered by target declaring a subset.
3. Destination symlink at apply: replaced, target outside untouched (H8).
4. Export with a declared symlink fails; with a missing declared file succeeds (H6, H7).
5. Factory reset with injected key-delete failure keeps declared files; retry removes them; telemetry
   kept (H11, H12, D5 ordering).
6. Invalid declarations fail start with 1109 and create no data (H1–H3).
7. Upgrade backup (1.2.3 → 1.2.4) inventory contains declared files byte-identical (H16).
8. Real `0c57e1ab` bundle restores its two members at the root; throttle absent as expected (H13).

Repository checks: AGENTS delivery list; uc-infra-profile tests; uc-engine config migration and host
contract tests; bindings compile through workspace check.

## Progress

- [x] Orientation, design, failure modes
- [x] Real pre-change bundle exported on clean `0c57e1ab` (library/legacy-bundle-0c57e1ab)
- [ ] Implementation
- [ ] E2E + artifacts
- [ ] Docs (interface, security, tech debt)
- [ ] Validation with exit codes
