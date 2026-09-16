# Sensitive Input Type Audit

## Goal

Replace application-facing plain-string sensitive inputs with existing protected domain types, without changing the stable Engine contract or unrelated serialization models.

## Phases

- [complete] Inventory sensitive string inputs and their conversion paths.
- [complete] Establish failing compile checks for the selected conversions.
- [complete] Convert inputs and callers with the smallest coherent changes.
- [complete] Update architecture documentation and remove obsolete wrappers.
- [complete] Run focused and repository verification.

## Completion Criteria

- No application-facing passphrase/password/token field uses `String` when an established protected type is immediately reconstructed downstream.
- Engine remains the stable external boundary and preserves its public data format.
- All affected production crates compile.
- Focused tests run, or pre-existing blockers are recorded precisely.
- Architecture maintenance record is updated.

## Errors

- Red compile check failed at the three expected redundant `Passphrase::new` calls in `SpaceFacade`; resolved by forwarding typed values directly.
- Full all-target workspace check is blocked by an unrelated unclosed delimiter in `space/query_space_membership_status/active_space_status.rs`; the affected Engine library check passed before that concurrent edit appeared.
