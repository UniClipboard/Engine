# Task 7: remove Desktop product settings from Engine

Base: Engine main 1c0660d9bcc7a1cc45be060cb64b4eaa865c90dc (branch hp/uni/t-0365-7-engine).
Design: t0355 desktop-product-settings-design.md §3.4, §7–§11. Task4 B3 failure at f0627a8.

## Owner and outcome

- Owner: Core `Settings` model (rules) + Infra `FileSettingsRepository` (load/save) + Engine contract mapping.
- Caller action: none new. Existing `QuerySettings` / `UpdateSettings` / `SettingsChanged` keep shape minus product items.
- Success: a settings.json that still carries product keys (valid or invalid) loads; the session starts;
  the next save writes only Engine-owned keys.
- Failure: Engine-owned fields with invalid values still fail load exactly as before (unchanged scope).
- Restart/retry: unchanged; no new persisted format version (removed keys are ignored, not migrated).

## Removal list (§3.4)

- Core: Theme, StartupMode, UpdateChannel, ShortcutKey, QuickPanelPosition, QuickPanelDoubleTapModifier,
  QuickPanelSettings, detect_channel (channel.rs), effective_theme_color_*, legacy silent_start/lightweight_start,
  GeneralSettings product fields (auto_start, startup_mode, restore_last_entry_on_startup, auto_check_update,
  auto_download_update, theme, theme_color*, theme_overrides_*, language, update_channel, usage_analytics_enabled),
  Settings.keyboard_shortcuts, Settings.quick_panel.
- Keep: device_name, debug_mode (task9), sync, retention, security, pairing, file_sync, network, mobile_sync.
- Application: ThemeView, UpdateChannelView, StartupModeView, ShortcutKeyView, QuickPanel*View/Patch + mapping.
- Engine contract: ThemeSummary, UpdateChannelSummary, StartupModeSummary, ShortcutKeySummary,
  QuickPanel*Summary/Patch, product fields in GeneralSettings{Summary,Patch}, SettingsSummary/Patch
  keyboard_shortcuts + quick_panel; SettingsSection{,Summary}::{KeyboardShortcuts,QuickPanel}.
- Migration: MigrationV2ToV3 = version bump only.
- Hosts: mobile probe `shortcut_count` output field.
- Out of scope: AppConfig.silent_start (not a settings field; no §3.4 entry); compatibility/ LAN line.

## Failure modes (written before checks)

F1 Invalid product enum (theme/startup_mode/update_channel/quick_panel.*) still reaches strict serde → session start fails (B3).
F2 Wrong JSON type in a product key (shortcut number, language number, analytics string) → load fails.
F3 A product key survives an Engine save (field kept somewhere, or flatten/extra map) → Desktop P3 assumption broken.
F4 Engine-owned values lost or reset by the removal (device_name, sync_on_restore, auto_sync alias, retention, network).
F5 v2 file (silent_start) migration now fails or loses Engine fields; schema_version not bumped to 3.
F6 debug_mode removed or broken early (task9 owns it).
F7 SettingsChanged still emits / lists removed sections, or a General-only product diff now emits General falsely.
F8 Historical fixture bytes modified to hide removed fields (provenance broken; SHA256SUMS mismatch).
F9 Public contract/bindings/probe still reference removed types → build break for hosts (uniffi, napi, probe).
F10 Docs (interface, glossary) still describe removed sections.
F11 Task12 rule patch applied on drifted text or widens credential exceptions.
F12 Upgrade-matrix `settings` observation changes meaning across versions (old anchors still report product fields).

## Checks (E2E first)

- E2E (upgrade-matrix, real connectivity host + real Engine runtime, real 0.19.4 fixture):
  - every legacy cell: after a real settings save, on-disk settings.json has no product keys, Engine values kept (F3,F4).
  - new cell d1-l0194-head-invalid-preferences: fixture copy with invalid product enums/types → start, unlock,
    history/device verified, restart, save drops keys (F1,F2,F3,F4). Red run on baseline source first.
- Existing: workspace check, uc-core/uc-infra-local/uc-application/uc-engine tests, fmt, arch scripts, diff check.

## Phases

1. [x] Failure modes + red E2E run on baseline (production code at 1c0660d9 + new matrix test code)
   - invalid-preferences: refused start_failed 1101, context "p2p network", upgrade_completed=true (B3 reproduced)
   - l0194-head and commit-interrupted: product-settings-written at stage settings-save (F3 reproduced)
2. [x] Code removal (core → app → engine → hosts)
3. [x] E2E green + delivery checks (dirty tree); final validation on the clean final SHA
4. [x] Docs: interface, glossary, matrix plan 051, fixture README; task12 rule patch with corrected
   key-material exception list (keyslot.json, profile-secrets-v1, iroh-identity/)
5. [x] Commits; report in the thread folder

## Findings

- F12 holds: the matrix `settings` observation only covers device_name and sync_on_restore, so old
  anchors and head observe the same fields.
- Restart in the invalid-preferences cell is also a true B3 case: invalid keys stay on disk until the
  first explicit save (fact invalid-preferences-before-restart = true).
- Residual strict fields that can still stop the session: debug_mode (task9), sync.sync_frequency,
  network.congestion_controller, retention rules, and every other Engine-owned typed field.
