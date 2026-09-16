# Space Admission Application Recovery

## Goal

Complete the Application-owned initial pending-admission recovery flow and stop before implementing the production Infra transport or persistence adapters.

## Completion Criteria

- [x] Deferred initial connection keeps the admission pending and reports one deferred item.
- [x] Successful initial authentication is committed before the saved JoinRequest is exchanged.
- [x] The exact saved request is exchanged; recovery does not regenerate it.
- [x] Candidate or stable rejection replies are handed to Core and committed through the recovery state port.
- [x] Recovery state and transport failures map to stable recovery report categories.
- [x] Application construction and public dependency surfaces expose the required ports without concrete Infra types.
- [ ] Focused Core/Application tests and Application-wide checks pass. Test execution skipped by user; test targets compile.
- [x] Work stops before production Infra/Engine adapter implementation.

## Phases

1. [completed] Finish transport dependency wiring and make the deferred path green.
2. [completed] Add successful authentication save-before-exchange behavior.
3. [completed] Add typed reply processing and stable failure behavior.
4. [completed] Complete Application exports and construction; static checks pass, tests not executed.
5. [completed] Review the scoped diff and identify the exact Infra handoff.
