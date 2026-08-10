# Task Plan: ADR-016 workspace-wide convergence specification

## Goal

Create an implementation-ready Chinese specification for ADR-016 in `docs/specs`, keep the documentation index and architecture maintenance record consistent, and validate the finished documentation against the repository checks.

## Next Step

Correct the independently reviewed contract gaps, verify the recovery channel against the current security-session evidence, then re-run the documentation and repository checks.

## Current Phase

Phase 2 (reopened after independent review)

## Phases

### Phase 1: Requirements and discovery

- [x] Read ADR-016 and the repository maintenance rules.
- [x] Identify adjacent ADR-012 and ADR-015 specifications.
- [x] Map the ADR against current public contracts and active implementation work.
- **Status:** complete

### Phase 2: Specification design

- [x] Define one owner, a small caller-facing interface, stable states, and recovery rules.
- [x] Verify the available security-session and membership-proof contexts for recovery payload encryption.
- [x] Correct recovery authorization, join completion, state fields, and acceptance scenarios.
- [x] Define bounded multi-batch handoff continuation and final-ack cleanup semantics.
- [ ] Reconcile ADR-015, ADR-016, and the architecture overview's responsibility statements.
- **Status:** in progress

### Phase 3: Documentation changes

- [x] Add `docs/specs/016-workspace-wide-convergence.md`.
- [ ] Apply the reviewed contract corrections to the ADR, specifications, and architecture overview.
- [ ] Update the documentation index and architecture maintenance record as required.
- **Status:** in progress

### Phase 4: Verification

- [ ] Review links, naming, responsibility ownership, and security assertions after corrections.
- [ ] Run repository metadata, formatting, architecture, and diff checks.
- **Status:** pending

### Phase 5: Delivery

- [ ] Review the final diff and report verified results and any limits.
- **Status:** pending

## Key Questions

1. Which existing public contracts must be replaced or subsumed rather than wrapped?
2. What durable facts are necessary for a device to resume convergence without the original sender?
3. What constitutes workspace completion, and how can a caller recover after missed events?

## Decisions Made

| Decision | Rationale |
| --- | --- |
| Target a new numbered specification `016-workspace-wide-convergence.md` | It directly implements the newly added ADR-016 and follows the existing ADR/spec numbering convention. |
| Keep the caller-facing surface small | ADR-016 and repository rules require one application-level owner rather than product-side choreography. |
| Replace the separate convergence/removal public summaries with one workspace snapshot | Current `QueryMembershipConvergence`, `QueryMemberRemoval`, and their event/result models cannot express one receiver-confirmed workspace result without callers joining facts themselves. |
| Reopen the documentation after independent review | The first draft had real contract gaps, so it must be corrected and re-validated before delivery. |
| Recovery envelopes use a historical, purpose-separated transport key | A lagging recipient may lack the current key. The key is derived for one bounded handoff with fresh identifiers and authenticated binding data; a removed instance is authorized out before any offer. |
| A join result is local readiness, not global completion | The sponsor records the workspace change only after the joiner is ready, so the joiner must wait for that commit confirmation before normal content participation. |

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| Shell interpreted backticks inside a documentation search pattern | 1 | Use quoted literal patterns and avoid shell interpolation in subsequent searches. |
| Broad consistency patch did not match one ADR-015 paragraph | 1 | No files were changed; inspect exact context and apply focused patches. |
| Independent review found six contract gaps after the initial verification | 1 | Reopen phases 2-5; verify security evidence and correct all affected documents before rerunning checks. |
| Planning update mixed task-plan and findings contexts | 1 | No files were changed; apply separate, context-verified updates. |
| Planning update used a new progress entry as matching context | 1 | No files were changed; inspect the current progress tail before patching. |
| Unrelated input prompt was triggered during review coordination | 1 | Ignored its response; it did not alter the task scope or documentation decision. |
