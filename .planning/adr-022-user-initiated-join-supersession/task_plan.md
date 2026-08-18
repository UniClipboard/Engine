# ADR 022: User-Initiated Join Supersession

## Goal

Research and document the adopted decision that every explicit user JoinSpace action starts a fresh join, while automatic recovery continues the existing join and irreversible admission remains forward-only.

## Completion Criteria

- [x] Internal architecture, persistence, protocol, and current documentation conflicts are mapped.
- [x] Mature-system precedent is researched from primary public sources.
- [x] ADR defines ownership, caller contract, safe supersession boundary, stable outcomes, restart recovery, delayed-message handling, and invitation replay behavior.
- [x] CONTEXT, Spec 023, and the architecture bible agree with the decision.
- [x] No production code is changed.
- [x] Required documentation-only delivery checks pass.

## Phases

- [complete] Research repository architecture and existing ADR conventions.
- [complete] Research external precedent and extract applicable principles.
- [complete] Draft ADR and reconcile authoritative documentation.
- [complete] Review contradictions, validate links and formatting, then run delivery checks.

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| `bb-browser` daemon did not initialize while opening Stripe and AWS documentation | 2 | Status showed daemon stopped; explicit start also timed out. Switched to direct read-only public-document retrieval. |
| Plan update patch context did not match | 1 | Re-read the scoped plan and applied an exact-context update; no ADR content was affected. |
| Shell interpreted backticks in a review search expression | 1 | The search still returned useful matches; subsequent searches use single-quoted patterns. No repository content was changed by the failed fragment. |
