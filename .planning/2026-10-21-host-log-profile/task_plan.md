# Task 9: host-provided log profile; remove Engine `debug_mode`

Base: task8 final `976b9ffe1c16a5d281e3158c879a08fd20108d93` (tasks 7 and 8 are ancestors).
Branch `hp/uni/t-0367-9-debug-mode`. Design: t0355 `desktop-product-settings-design.md` §2, §3.2, §5.4, §7.2, §8 (P2), D7, task 9.

## Problem

Facts found before any change:

- `GeneralSettings.debug_mode` is persisted in Engine `settings.json` and exposed through
  `GeneralSettings{Summary,Patch}`, `Operation::UpdateDebugMode` and `DiagnosticsStatusSummary.debug_mode`.
- Engine never applies it. The host installs its own log layers before `Engine::start`
  (Desktop `select_log_profile` reads the raw file in bootstrap). Engine module logs follow only the
  process runtime standard/detailed capture mode.
- `DiagnosticsFacade::current_effective_log_profile` guesses the host profile from `RUST_LOG`,
  `UC_LOG_PROFILE`, the stored `debug_mode` and Engine's own `cfg!(debug_assertions)`. Desktop already
  overrides this guess with the profile it actually installed (`debug_status_dto`).
- The diagnostics export manifest copies the stored `debug_mode` and the guessed profile.
- UniFFI and HarmonyOS N-API expose no diagnostics status, debug-mode operation or settings type.

## Owner and outcome

- Rule: Core `HostLogProfile` (validated label), next to the other host declarations in `app_dirs`.
- Declaration: host passes the label of the log profile it actually installed through
  `EngineConfig::with_host_log_profile`. Engine validates at start. Not persisted, not exported in config.
- Report: Application `DiagnosticsFacade` is the single reader. `QueryDiagnostics` and the export
  manifest read the same injected value. No settings read, no environment read, no build-type fallback.
- Caller action: declare the label once in `EngineConfig`. No new operation, result or event.
- Success: status and manifest report exactly the declared label, or an explicit absence.
- Failure: an invalid label fails `Engine::start` with `InvalidInput` code 1110 before any profile I/O.
- Restart/retry: none. Each start reports its own declaration. Restart semantics belong to the host.

## Contract

- Label grammar: 1–32 bytes of `[a-z0-9_-]`. Desktop labels (`dev`, `prod`, `debug`, `debug_clipboard`,
  `cli`) are valid. Engine does not interpret the label.
- No declaration: `DiagnosticsStatusSummary.effective_log_profile = None`; manifest `effectiveLogProfile: null`.
- Removed: `Operation::UpdateDebugMode`, `OperationKind::UpdateDebugMode`, `UpdateDebugModeInput`,
  `DebugModeUpdateSummary`, `OperationResult::DebugModeUpdated`, `UPDATE_DEBUG_MODE_FAILED_CODE` (1412 in
  the diagnostics group), `DiagnosticsStatusSummary.{debug_mode,restart_required}`,
  `GeneralSettings{Summary,Patch}.debug_mode`, Core `GeneralSettings.debug_mode`, manifest `debugMode`.
- Legacy `general.debug_mode` (any JSON type) is ignored on load and dropped on the next save.
  No settings schema version change (task7 precedent).
- Unchanged: process runtime standard/detailed capture, capture windows, module-log budgets,
  network-library logging rules and its lease, settings schema version, bindings.

## Failure modes (written before code)

1. Engine still reads `RUST_LOG` / `UC_LOG_PROFILE` or uses its own build type, so it reports a profile
   the host never installed.
2. Status and export manifest come from different sources and disagree.
3. The label leaks into persistence (settings, config bundle) and survives a restart with another label.
4. A missing declaration is reported as a concrete profile (`prod`/`dev`) instead of absence.
5. Legacy `debug_mode: true` or an invalid type (`"yes"`, `42`, object) fails settings load and therefore
   session start (#581 class).
6. Legacy `debug_mode` is written back on the next save, so it reappears for old binaries or the host importer
   cannot rely on the key disappearing.
7. A legacy `debug_mode: true` still changes the reported profile.
8. The label `debug` raises Engine module logs, starts a detailed capture or changes network-library rules.
9. An invalid label (empty, too long, uppercase, spaces, path characters, free text) reaches status,
   manifest or logs, or is silently replaced.
10. An invalid label is rejected after profile I/O, leaving partial state.
11. Removed operation stays dispatchable or a stale error code stays in the uniqueness list.
12. Diagnostic export still loads `settings.json`, so a broken settings file fails export.
13. Bindings' generated interfaces change without notice to Mobile.
14. Upgrade matrix still requires `general.debug_mode` after save, or no longer checks legacy profiles.
15. Debug output of `EngineConfig` prints the label without bound (it is bounded by grammar; still check).

## Verification plan

- Public Engine E2E `crates/uc-engine/tests/host_log_profile_e2e.rs` with a synthetic host:
  explicit labels across restarts, no declaration, invalid labels, legacy `debug_mode` values,
  status/manifest agreement, standard capture mode, `UC_TEST_ARTIFACTS_DIR` evidence.
  Run with `RUST_LOG=trace UC_LOG_PROFILE=debug` set to prove the environment is ignored.
- Upgrade matrix: `ENGINE_GENERAL_KEYS = ["device_name"]`, explicit absence check, invalid legacy
  `debug_mode` injected.
- Existing host/public contracts and mobile probe adapted minimally. Removed-behavior unit tests deleted.
- Repository checks from AGENTS.md.

## Phases

- [ ] 1. Failure modes and plan (this file)
- [ ] 2. Contract, Core, Application, Engine wiring
- [ ] 3. Adapt existing checks, upgrade matrix, mobile probe
- [ ] 4. Public E2E and artifacts
- [ ] 5. Docs, downstream handoff
- [ ] 6. Task7 old-anchor smoke investigation
- [ ] 7. Full checks on final immutable SHA
