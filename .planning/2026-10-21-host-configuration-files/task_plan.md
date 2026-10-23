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
  reset removes only ByEngine files after keys are wiped; ByHost files survive; backup contains all files.
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
- Reset: ByEngine declared paths are removed as files only (a symlink is unlinked, a directory fails the reset),
  then verified absent together with Engine state.

## D5/D6 ownership (approved)

The daemon deletes the product file only AFTER Engine reset succeeds. Declare that file as `ByHost`.
Engine preserves every `ByHost` file after both failed and successful reset. Declare updater-state
files as `ByEngine`; Engine deletes them after key wipe in the existing journaled reset. Task10 owns
post-success product deletion, retry, error visibility and in-memory reload. Telemetry is undeclared
and survives reset. Whole-root upgrade backups still include telemetry and undeclared files.

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
5. Factory reset with injected key-delete failure keeps all host files; retry removes ByEngine files;
   ByHost product, telemetry and unlisted files survive failure and success (H11, H12, D5 ordering).
6. Invalid declarations fail start with 1109 and create no data (H1–H3).
7. Upgrade backup (1.2.3 → 1.2.4) inventory contains declared files byte-identical (H16).
8. Real `0c57e1ab` bundle restores its two members at the root; throttle absent as expected (H13).

Repository checks: AGENTS delivery list; uc-infra-profile tests; uc-engine config migration and host
contract tests; bindings compile through workspace check.

## Progress

- [x] Orientation, design, failure modes
- [x] Real pre-change bundle exported on clean `0c57e1ab` (library/legacy-bundle-0c57e1ab)
- [x] Implementation
- [x] Nonvisual E2E + artifacts; cross-install runtime acceptance remains blocked
- [x] Docs (interface and existing import debt; no security exception)
- [x] Local validation with exit codes; final immutable-source receipt stays in the thread library

## Resume constraints and additional failure modes (before continuation)

WIP `3c42d7b719fd0713f44ea3c310383bc3472c2fd5` and all original evidence are preserved.
H18 ByHost file removed on reset success or failure violates D5; verify both outcomes.
H19 Cross-install import copies data but Engine cannot start: investigate missing admission-key versus
    host adapter inconsistency; preserve original bundle and baseline trace. Do not export additional
    credentials or replace this acceptance gate with same-install evidence.
H20 Import copy fails after some host files: marker and staging must survive; repeat boot applies the
    same staged bytes and starts successfully after the destination obstruction is removed.
H21 Export regular-file check races with symlink replacement: Unix no-follow open must reject it.
H22 Import missing optional member must preserve existing destination bytes.
H23 Reset-owned directory must fail without recursively deleting it; ByHost and telemetry survive.
H24 Secure-storage set failure during apply must remain distinguishable from successful recovery.

## Verification scope

The host-file E2E suite uses Unix symlink primitives and is compiled only on Unix.
Windows and native platform runs are skipped. The public config migration E2E remains portable.
Cross-install reproduction tests can pass while recording an expected 1101; this is a reproduced
blocker, not accepted cross-install behaviour. Only the same-install scenario proves successful
public export/import/restart. The legacy bundle test proves mapping bytes, not a usable session.

## Cross-install adoption repair (authorized continuation)

Checkpoint `7412241308b6fb044d28f724351c269b63b82822` is immutable. Preserve its report,
acceptance receipt, bundles and failures. New work uses separate commits.

### Failure modes, before implementation and isolated checks

A1 Startup caches the target lifecycle across import: source ciphertext opens with the wrong generation.
A2 Recovery caches the target decrypted vault across replacement: imported KEK and old secrets become mixed.
A3 Target active manifests, content-key catalogs, upgrade journals or generation directories survive adoption:
   startup follows target state under source keys, or exposes target data as imported data.
A4 A crash between retiring two target components leaves no usable profile and loses original files.
A5 A crash after writing the new KEK but before replacing the vault prevents recovery preparation on retry.
A6 Backup fails or fills the disk after retirement: importing must not begin; every original remains retained.
A7 A retry repeats retirement after imported files exist: imported files overwrite the original backup.
A8 The imported archive selects retirement names, backup locations or a transaction identifier: unrelated files can move.
A9 An existing target backup record uses its old vault key after import: later backup preparation cannot decrypt it.
A10 Cleanup removes staging before removing its marker: the next boot sees an incomplete import.
A11 A secure-storage write fails but apply returns success: startup opens a mixture or discards the retry marker.
A12 Existing target contains unrelated root files, telemetry or data inside retired Engine directories:
    adoption must preserve their exact bytes, including retained originals.
A13 Legacy bundle lacks complete runtime state: reject before staging and require re-export (approved).
A14 Successful first boot still fails on the second boot: verify both, setup completion and session readiness.
A15 Target is mid factory reset: import must not bypass the reset journal.
A16 Source/target backup roots are on different filesystems: never use copy-and-delete without verification.

### Transaction and ownership invariants

- The existing Application profile-startup owner invokes one complete offline import capability before
  version upgrade and before loading the lifecycle used for runtime wiring.
- The Infra import owner alone selects a fixed list of Engine-owned components to retire. Imported
  paths never select retirement scope. Unknown root files and telemetry stay in place.
- A local journal outside the untrusted staging directory binds one staged marker to one transaction.
  Publish and flush it before any move. Persist the original component inventory once.
- Retire original Engine components with no-replace renames into a private, retained local recovery
  directory. Never delete originals. On retry, an existing retired destination is authoritative;
  do not move newly installed files into it. Fail on ambiguous states.
- Capture and verify retired Engine state with the maintained ProfileBackupArchive before installing
  staged bytes. Retain the archive receipt and originals after success. No extra credential export.
- Retain existing target upgrade-backup records with their original encrypted vault so imported
  credentials never reinterpret those records. Keep prior backups, rather than pruning or resetting them.
- Reset the in-memory recovery cache before and after replacement. Adopt only the bundle's existing
  encrypted profile vault and KEK. Reread lifecycle after adoption; never generate a replacement identity.
- Preserve marker/staging/journal on any failure. Replay installation only after retirement and backup
  are durable. A failed attempt never publishes an Engine handle or starts background runtime work.
- Finish installation before clearing the pending marker. Remove the marker before cleaning staging;
  an orphan staging directory is inert and the next explicit staging operation can replace it.
- Keep D5/D6 ownership and historical upgrade-backup scope unchanged. Keep the task7 old-anchor gate.

### Acceptance

Use real public Engine starts and operations with separate synthetic installations and no credential
copying. Verify new and existing targets, first and later starts, unchanged source Space identity,
setup completion, usable encryption, product bytes, unrelated target bytes, retained target backups,
and interrupted retirement/install retry. Run the original exported legacy bundle unchanged.
Historical negative 1101 evidence remains at the checkpoint; new acceptance requires successful starts.

### Approved incomplete-bundle decision

The user explicitly chose: reject incomplete legacy bundles before target mutation and require a new
export from the retained source installation. This replaces successful-import acceptance for the
original `0c57e1ab` bundle. Preserve its original bytes and negative evidence.

A17 V3 export includes only the profile database: control security material and membership are lost.
    Preserve both database snapshots and the authenticated V3 manifest. Resolve installed paths with
    the maintained runtime-layout owner. Never merge V3 credentials into a legacy upgrade source.
A18 Source writes during the two database snapshots or vault capture: reject an export when persistent
    database revisions or protected state bytes change. Do not seal a mixed snapshot.
A19 Imported current-version state lacks the source Engine upgrade cursor or encrypted content-key
    catalog: startup can run a false legacy isolation or lose access to protected payloads.
    Carry these existing protected files; do not export additional independent keys.
A20 A new bundle requires state older readers ignore: use manifest schema 2 so old readers reject it.
    Keep the outer encryption and KEK contract unchanged. Reject schema-1 incomplete bundles before staging.

A21 V3 rows and admission credentials bind the source runtime generations. A merged monolith cannot
    be treated as a pre-V3 source. Preserve the authenticated source manifest and both active stores.
A22 Missing source blobs leave a valid manifest with unavailable content. Carry active encrypted blobs
    and reject unsupported files; keep the existing archive size and AEAD limits.
A23 Imported protected files reach disk after the completion journal. Flush files and containing
    directories before declaring installation complete.

A24 An old Engine opens new staging without reading the bundle manifest. Bump pending marker schema
    to 2 as well as archive schema 2, so an old boot reader cannot apply a partial single-store import.

## Continuation verification status

- Public content E2E: 8 passed, including new/existing targets, blob reads, new encrypted writes,
  partial-adoption replay, original-target recovery, explicit legacy rejection, reset and backup.
- Existing Infra config-migration tests: 39 passed. No new unit tests were added.
- Final delivery uses an immutable repair commit and a fresh source receipt. Historical progress and
  verification sections above describe checkpoint 74122413, not current acceptance.

## PR226 Windows follow-up (before repair)

The x64 Windows job on 976b9ffe failed to compile: adoption imports a synchronous no-replace
helper that was exported only on Linux, Android and macOS. The async publisher already has a
maintained Windows MoveFileExW implementation. Keep 976b9ffe immutable and add a separate fix.

Failure modes: a normal rename can overwrite retained originals; COPY_ALLOWED can cross a volume
without atomicity; missing WRITE_THROUGH can publish the journal before retirement is durable;
string conversion can discard the Windows system error; unsupported mobile Unix targets must
compile and fail explicitly instead of emulating successful retirement.

Keep the fs atomic-publish module as the primitive owner. Extract its existing Windows syscall
into the synchronous capability used by both import and the async publisher. Keep replacement and
copy flags disabled, retain the original io::Error, and use WRITE_THROUGH on Windows. Provide an
explicit Unsupported synchronous capability on unsupported targets. Keep import ownership unchanged.
Extend the existing synchronous file-preservation contract to Windows before changing production
code. Run the maintained real filesystem contracts and public Engine import E2E. Add the existing
atomic-publish contracts to Windows CI runtime verification. Push only a new fast-forward commit,
then run the Windows matrix on that exact SHA. Review quota/configuration remains outside task8.


## PR226 Engine check follow-up: reuse task9 fixture repair

The failure matches task9 commit `2c3dd6388f03c3712e1f2859cd2f17e9e4983bc5`
and its retained report. This is an obsolete host assembly fixture, not an unknown production regression.
Failure modes: the invalid marker fails before the intended backup denial; the backup-reuse fixture
never reaches legacy adoption; removing archive or identity assertions would hide retry corruption.
Reuse only the exact host.rs test diff from that commit. Keep the Application startup owner and
production ordering unchanged. The backup-denied fixture must reach backup denial without an import
marker. The reuse fixture must fail lifecycle persistence after backup and legacy adoption, then keep
one archive with identical bytes across retries, adopted identity and no lifecycle marker.
Run both existing host tests and the maintained public import E2E; retain failures and exact source
receipts. Task9 already contains this repair. Downstream synchronization must retain it, not reapply
an alternative fixture. Do not import task9 log-profile implementation into task8.


## PR226 review repair: export bounds and rejected staging (before code)

Failure modes: schema-1 or incomplete staging blocks every boot; discarding a marker after a
journal exists loses adoption recovery; untrusted marker paths enlarge cleanup scope; cleanup
failure is hidden; blob buffers exceed the cumulative 2 GiB import limit; tar padding and AEAD
overhead push an otherwise accepted export past the reader limit; concurrent blob growth changes
the snapshot; synchronous capture stalls a Tokio worker; stale schema prose misleads hosts.

The existing import owner rejects incompatible staging only before a journal exists, uses fixed
staging cleanup paths, emits a fixed warning and lets ordinary startup continue. Active journals
remain fail-closed and retain all recovery data. The existing export adapter owns capture, revision
verification, bounded archive packing, sealing and writing inside a Span-propagated blocking task.
Use maintained tar streaming for on-disk blobs, preflight one cumulative budget before blob reads,
reserve tar and AEAD overhead, and keep the existing typed size rejection. No new crypto or format.
Write public Engine E2E first: upgraded schema-1 and incomplete schema-2 staging must permit first
and subsequent startup with original Space/history/config; two sparse 1 GiB blobs must reject export
without allocating their contents or replacing the destination. Keep existing import/recovery E2E.


## PR226 additional CI failure: active log-count fixture (before change)

Run 39619613559 reports 302 disk records versus 301 from the earlier counter snapshot plus legacy.
The public diagnostic report and later directory read are not atomic while the Engine writes logs.
An arbitrary tolerance or retry would hide losses. Keep the active snapshot as a lower-bound check;
then complete public Engine shutdown and public process-observability shutdown before demanding
exact equality and count growth across leave. Preserve same-instance create after leave and legacy
migration checks. Change only the existing binding E2E fixture. Retain exact numeric artifacts.
The detailed pre-change diagnosis and original failure live in the review-repair evidence directory.


## Retained rejected import repair (authorized product decision, before code)

The user approved retaining rejected staging, continuing the original Space and exposing a durable
queryable rejection with re-export guidance. The old cleanup can destroy the only remaining source.
Failure modes: deleted unique staging; overwritten rejected staging on the next Stage call; ephemeral
warning mistaken for a durable result; original recovery skipped merely because a rejected marker
exists; an active adoption journal incorrectly treated as safe rejection; I/O treated as incompatibility;
marker-selected paths or staging symlinks reach active data; invalid replacement deletes the source;
unbounded rejection archives; new diagnostic plaintext carries secrets.

Keep the original pending marker and staging bytes in place. No new format, credential copy, backup
folder or plaintext status file is needed. Their persisted contents are the durable rejection state.
The import owner performs one read-only classification reused by adoption, recovery, query and Stage.
Diagnostics queries return a safe typed reason and fixed re-export guidance. Startup continues only
for rejected inputs with no journal and successful inspection. Active journals and inspection errors
retain the existing recovery/failure boundary. Original key recovery runs normally for rejected imports.

One pending slot remains bounded. Stage defaults to refusing replacement of rejected data. A new
explicit replace_rejected_import input authorizes replacing that slot only after the new bundle has
passed the maintained complete validation. This is an irreversible host-confirmed action, not automatic
cleanup. Invalid passwords/bundles and default Stage retain all old bytes. Successful explicit restage
reuses the existing staging writer and adoption transaction; FactoryReset retains its existing explicit
whole-profile deletion contract. No extra retention copies or automatic pruning are added.

Write public E2E before production changes: actual legacy schema-1 archive staging, schema-2 missing
member after public Stage, no remaining external bundle, full byte inventories across repeated starts,
durable QueryDiagnostics rejection, original history reads and new encrypted writes, fixed guidance,
default/invalid replacement preservation, explicit valid replacement and successful later import,
active-journal and I/O failure preservation, traversal fields and staging/member/marker symlinks.
Reuse existing archive/crypto fixture tools only for old-version fixture arrangement; no secure-store
copying. Preserve all prior failures and 0a623a30 as immutable historical evidence.


## Formal Mobile rejection query (before binding code)

Task11 audited whole-directory iOS legacy migration and confirmed it can carry pending imports even
without an import button. The existing observability query is unrelated. Failure modes: no formal
binding path; a stale/native-mismatched method; converting Engine read failure into None; a one-shot
event missed across restart; private-file parsing in Mobile; losing original Space/history while
reporting rejection; leaking source materials through a binding record. Task11 agreed the typed
query and error semantics through Herdr coordination, retained in the evidence directory.

Add only MobileEngine.query_rejected_config_import -> Result<Option<RejectedConfigImport>, BindingError>.
The existing worker sends real Operation::QueryDiagnostics and projects its rejection field. Map
three reasons to a UniFFI enum, preserve the fixed message and Engine code/category/retryable. None
never means applied. Mobile owns localization and UI. No write/import API is added to Mobile.
Write a real binding integration E2E before implementation: original Space, stored history restored
through the host clipboard, new encrypted writes, repeated starts with both old and incomplete
staging, unchanged source bytes, typed reasons/guidance, and Engine query errors instead of None.
Generate actual Swift and Kotlin bindings from the matching host library, record symbols/checksums
and hashes, and hand off the new fixed stack to tasks9/10/11. Native compilation and product UI
acceptance remain downstream gates. No peer worktree changes or production data access.

## PR follow-up: completion barriers and inherited CI failures

Failure modes recorded before the follow-up changes:

- A manual rebuild is accepted before its background task starts. A query can observe the previous ready generation, pass an entry-count check, then race with the requested rebuild. The test must observe a new completed rebuild through the public status query before checking all existing tag and content invariants.
- A recovered file worker emits the recovery record into its own queue. A flush already queued by the caller can acknowledge before that recovery record is written. The test must flush after observing recovery, then require exactly one persisted recovery record with the original envelope assertions.
- The leave resource test can sample asynchronous reclamation and bounded connection-pool variation. Reuse only task9 ae75c75624e913a77697664e7045bff8649bf4b7's exact test change. Its retained negative controls reject one leaked descriptor or task per cycle. Do not merge task9 implementation.

The first two tests and their relevant production modules have no diff against the fetched task7 baseline. The first local replay passed both tests. Repeated original runs and the complete remote failure log are retained. No import implementation or assertion is removed to address these failures.

## CI follow-up: unavailable collector capture boundary

Failure modes recorded before the test change:

- A remote queue or exporter waits on slow HTTP and blocks the business producer. Keep all 10,000 operations, the strict two-second producer limit, and both dropped-record and failed-batch assertions.
- On Linux, the independent synchronous JSON system sink writes every completion to stdout. The CI capture pipe can block that sink inside the timed loop. This is not a measurement of remote queue blocking. Use a real subprocess with stdout/stderr retained in a regular artifact file, not a CI pipe. Do not change the production subscriber or its policy.
- Child recursion, child failure, or missing artifacts can falsely report success. Use one explicit child-mode flag, require a successful child exit and the summary artifact, and retain captured output on failure.
- An assertion fails before evidence is written. Record elapsed producer time, real exporter health counters, flush and shutdown results before assertions. The slow real wiremock Collector remains unchanged.

The existing E2E owns the entire scenario. The parent only isolates process output; the child installs the real public runtime, sends real HTTP export attempts, saturates the real queues, and checks every original invariant. No new unit test, product API, security contract, or dependency is introduced. CI Linux failed; the original local macOS replay passed. The platform difference remains explicit.
