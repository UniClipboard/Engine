# Mobile Analytics Binding Contract

## Goal

Expose the existing Engine analytics capture and identity capabilities through the iOS/Android UniFFI host boundary, while preserving no-op defaults for hosts that do not opt in.

## Completion Criteria

- Mobile hosts can receive every Engine analytics event through a stable, content-safe binding model.
- Mobile hosts can persist and switch analytics identity through the binding.
- `MobileEngine::start_with_analytics` installs the supplied mobile analytics capabilities into `HostCapabilities`, while `MobileEngine::start` remains compatible and no-op.
- Existing hosts can explicitly use no-op implementations without vendor dependencies in Engine.
- Generated Swift and Kotlin bindings expose the new contract.
- Contract tests fail before implementation and pass afterward.
- Architecture documentation records the ownership and boundary.
- Repository-required verification passes, or any pre-existing blocker is documented with evidence.

## Phases

1. **Contract design and RED tests** - complete
2. **Minimal binding implementation** - complete
3. **Generated binding and focused verification** - complete
4. **Architecture documentation and full verification** - complete

## Decisions

- Engine owns typed events, identity transition ordering, and conversion to mobile-safe DTOs.
- Mobile hosts own vendor transport, runtime consent gating, and durable analytics identifiers.
- No PostHog, Sentry, or OTLP dependency enters the Engine binding.
- The mobile boundary exposes one coherent analytics host capability instead of separate per-event APIs.

## Errors Encountered

| Error | Attempt | Resolution |
|---|---:|---|
| Mobile analytics flow test received no `setup_started` event | 1 | Expected RED result; implement host adapters and `HostCapabilities` injection. |
| Binding adapter could not resolve `tracing` | 1 | Add the existing lightweight tracing dependency so callback failures remain warn-only. |
| `cargo fmt --check` reported formatting differences | 1 | Run the repository formatter, then re-run the check. |
| Full binding suite cascaded into shutdown timeouts | 1 | Confirmed another worktree was running tests concurrently. After it completed, the serial binding suite passed all 25 checks. |
