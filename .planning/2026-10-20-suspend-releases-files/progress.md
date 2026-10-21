# Progress

- 2026-10-20: read code, design reviewed by advisor.
- E2E `bindings/uc-engine-uniffi/tests/suspension_releases_files.rs` red on base: after suspend the
  network lease + segment and profile/control SQLite stay open and locked (copy in thread library).
- E2: DbPool suspend/resume (min_idle 0, drain wait, typed `DbPoolError::Suspended`) composed with
  security into `ProfileLocalResources` (uc-infra-profile). Application participants unchanged.
- E1: file queue park/unpark messages; RunStore parked state, reopen under `.cleanup.lock`;
  process count `EngineLogActivity`; Engine ends it before publishing Suspended/Stopped.
- E4: `open_lockable_files` census (uc-infra-local); record before Suspended with counts only.
- E3: startup drain identified (iroh net_report QAD probe connections); see findings.
