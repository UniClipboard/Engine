# Findings

- Core now exposes `AdmissionPendingRecovery::Initial` with the saved route, saved request, and encrypted password-equivalent as one read-only view.
- Application owns `recover_pending`; Infra must only establish/authenticate/exchange through a transport-agnostic port.
- The production Infra and Engine wiring are intentionally out of scope for this slice.
