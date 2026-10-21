# Progress

## 2026-10-21 resume

- Preserved WIP 3c42d7b7 and original synthetic bundles/logs.
- Verified task7 final is an ancestor. No peer edits or newer-main integration.
- Report was absent on resume. Progress command returned "not inside a Herdr pane"; continue without retry.
- Corrected stale D5 plan text. Existing code already has ByHost/ByEngine ownership.
- Cross-install import 1101 remains a required unresolved acceptance gate.

## Verification milestones

- Initial WIP suite: 4 passed, 2 startup failures (1101). Evidence retained.
- Retry diagnostic compile failed once due to borrowing `self.text`; fixed the test report writer.
- Final host-file scenarios before the added ready-state assertion: 7 passed. Two scenarios record
  reproduced cross-install blockers; they do not represent accepted cross-install behaviour.
- Existing config-migration pipeline E2E: 2 passed (Infra path, not public Engine startup acceptance).
- metadata, workspace all-targets, lan-compat/dev-tools all-targets, fmt, baseline Rust style,
  direct-log enforcement, repository preflight and diff check: exit 0.
- A concurrent comment edit caused mbx to discard one test compile; retained ready-check.log (101).
  Freeze source and rerun before finalization.
- No new unit tests were written. Existing tests received only required constructor updates.

- Source-frozen ready-check rerun: exit 0. After partial copy failure and retry, public setup reports
  completed, with a Space and no re-pairing requirement; encryption reports initialized and session-ready.
- All delivery command exit codes are 0 in resume-validation/delivery/results.json. Repository preflight
  is separately recorded as 0 in resume-validation/exit-codes.txt.
- Local implementation is ready for immutable-source verification. Cross-install runtime acceptance
  and old-anchor downgrade execution remain unresolved gates; native runs remain skipped.

## Authorized cross-install repair continuation

- Preserved immutable checkpoint 74122413 and its summary receipts under adoption-repair/checkpoint-74122413.
- Committed failure modes and adoption invariants separately at aeb6410b before the repair.
- Traced missing V3 control state, rejected a failed monolithic snapshot approach, and retained every failure log.
- Implemented complete V3 export and offline adoption with the existing vault/KEK, runtime layout, and backup archive.
- The user approved rejecting incomplete schema-1 bundles before staging and requiring source re-export.
- v3-runtime-4: public fresh-install first/subsequent startup passed.
- expanded-e2e: 7 passed; the remaining failure was a missing parent in the isolated backup-restore fixture.
- existing-target-2: public existing-install, two partial-adoption failures, replay, original-target backup
  restoration and public original-passphrase recovery passed.
- full-public-e2e: both low-level migration scenarios passed; two public scenarios rejected an oversized
  synthetic SendText input. The fixture was corrected to the public 64 KiB input limit.
- content-e2e: all 8 public host-file scenarios passed, including large encrypted blob reads and new writes.
- No new unit tests were added. Existing constructor fixtures and startup-order assertions were maintained.
- Final immutable-source repository checks and matched artifact receipts remain in progress.
- Task7 old-anchor mbx gap remains explicit. Native GUI, devices, Windows, downstream, and release are skipped.
