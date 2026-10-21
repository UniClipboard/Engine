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
