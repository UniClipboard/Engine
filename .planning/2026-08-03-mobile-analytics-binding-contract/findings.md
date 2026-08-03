# Findings

- Engine already defines `AnalyticsPort`, `AnalyticsIdentityPort`, and `DefaultAnalyticsFacade`.
- `HostCapabilities::with_analytics` connects those capabilities to application use cases.
- The current UniFFI `BindingHost` exposes only directories, secure storage, files, and clipboard.
- `host_capabilities` constructs `HostCapabilities::new(...)` without analytics, so mobile always receives no-op analytics behavior.
- Mobile currently consumes Engine `v0.20.0-rc.18`; the missing boundary is present in the current released artifact, not caused by an old mobile pin.
- Preserve `MobileEngine::start` as the compatible no-op constructor and add `start_with_analytics` for hosts that supply the capability.
- Use one `BindingAnalyticsHost` callback interface for capture, identify, group identify, and identity persistence.
- Send event-specific properties as a JSON object string. Engine remains the schema owner, while foreign hosts can forward the structured payload without mirroring every Rust enum.
- Re-export the necessary analytics interfaces through `uc-engine`; the binding must not depend directly on the internal observability crate.
- A single adapter can implement both Engine analytics interfaces over one foreign host callback, preserving one mobile capability owner.
- The compatible constructor remains no-op; only `start_with_analytics` installs the adapter.
- Foreign delivery errors must be warn-only. Identity persistence errors map to the existing Engine identity error so setup/pairing behavior remains authoritative in Engine.
- Returned identity UUIDs must be parsed and an adopted identity must match the Engine-requested Space person id.
