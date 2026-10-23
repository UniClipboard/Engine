# Findings

- Original baseline trace: StageConfigImport succeeds, startup fails 1101 in dependency wiring after
  apply; old members land in ui-state instead of root. Changed trace remains retained separately.
- Current migratable secret list contains only profile KEK. AdmissionKeyManager uses a separate
  secure-storage key. This is a source observation, not yet a verified runtime diagnosis.
- Whole-root upgrade backup is independent of declarations and includes telemetry.

## Verified diagnosis

- `diagnosis-2.log`: first boot fails at prepare_source/current Space identity; second boot fails at
  load_journal/AEAD decryption. Both code 1101, with no Engine handle for public recovery operations.
- Synthetic vault preparation returns Ready; admission key is present. Opening legacy current-Space
  ciphertext with the imported key and imported lifecycle generation returns 38 plaintext bytes.
- Therefore the earlier missing-key attribution was incorrect. No extra credential export is needed
  to explain the failure. PrepareProfileStartupUseCase caches the target lifecycle across import;
  target storage-upgrade journals also outlive the profile replacement.
- Same-install partial-copy retry starts the real Engine successfully. This is additional evidence,
  not cross-install acceptance. Cross-install mapping tests explicitly retain the failed runtime gate.
- Reproduced task7 a13 mbx metadata-probing failure in this worktree. Root-invoked manifest metadata
  succeeds. No supported task-local build configuration was established; downgrade remains unexecuted.

## Adoption repair continuation

- Preserved the checkpoint report, handoff, acceptance receipt and source receipt under
  library/adoption-repair/checkpoint-74122413 before replacing any current summary.
- Existing ProfileBackupArchive verifies retained local originals without exporting platform keys.
- The bundle exports the profile database but no generation selector. The adoption owner must retire
  target selectors and upgrade state before existing legacy-to-V3 bootstrap consumes the portable snapshot.
- Retained target backup records must stay with their old encrypted vault; importing a different vault
  must not cause future backup preparation to reinterpret those records under different keys.
- The local adoption journal lives outside imported staging. Its component inventory uses fixed Engine
  names and a locally generated UUID. Imported members cannot select retirement targets.
- A source-edit script failed before changing the E2E file (ValueError substring not found). The first
  compile is still useful for constructor/type validation; its requested new test name is not yet present.

- First compile succeeded but ran zero tests because the E2E rename script had failed. It is compile
  evidence only, not a passing acceptance scenario.
- First real new-install scenario failed before installing source state: ProfileBackupArchive rejected
  an archive destination that contained the retired source directory. Changed the archive destination
  to a disjoint sibling directory. This retains the existing archive safety rule.

- Complete-export attempts 3 and V3-runtime attempts 1–3 failed and remain retained. The merged
  monolith restored security material but violated V3 control-generation ownership. The repair now
  carries the authenticated V3 manifest and two consistent snapshots, preserving existing bindings.
- V3-runtime-4 public new-install E2E passed. First and second boot are ready, without key copying.
- Active encrypted blob files and the existing content-key catalog travel with the same generation.
  The export retries only a typed concurrent-source-change failure, at most three attempts.
