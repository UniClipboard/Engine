# Progress

## 2026-08-26

- Traced the missing caller to commit `098d806` and reconstructed the removed event-to-orchestrator-to-session-send path.
- Confirmed the new application endpoint was introduced without a production protocol preparer, network caller, or complete outbox transport.
- Confirmed the user requires a clean single protocol rather than restoration or compatibility.
- Started repository and requirements research for Spec 028.
- Read ADR/Spec 017, ADR 022, ADR/Spec 025/027, and encrypted-persistence rules; extracted the current non-negotiable ownership, success, supersession, recovery, and privacy requirements.
- Read Spec 023 ownership, data model, workflow, edge-case, testing, and acceptance sections; separated durable membership invariants from protocol-version compatibility requirements.
- Researched RFC 9420 MLS, RFC 9807 OPAQUE, RFC 9382 SPAKE2, official Iroh protocol-handler guidance, and idempotency practice; selected MLS + OPAQUE + direct Iroh handler and rejected SPAKE2 for this implementation.
- Inspected current Iroh handler and Engine assembly order; identified the required direct endpoint installation and router-start reorder.
- Inventoried the current Core record/outbox model, Application admission modules, Infra store/wire code, Iroh ProtocolHandler API, Engine wiring, and stable bindings; selected a stage-carrying aggregate and one direct connection endpoint.
- Confirmed the separate admission repository and embedded application ledger are competing incomplete stores; selected one encrypted membership ledger and a mandatory single-device Space rebuild/re-pair cutover.
- Reviewed Spec 027 deletion/acceptance constraints, current Space maintenance guidance, architecture checks, and Engine/binding contracts; fixed the stable product surface and the new mandatory forbidden-symbol checks.
- Wrote `docs/specs/028-single-space-admission-protocol.md` with all 11 required sections, exact cross-layer components, data model, interfaces, J0-J3 workflow, implementation steps, deletion checklist, edge cases, tests, acceptance criteria, risks, and fixed decisions.
- Added Spec 028 to the docs index and architecture related-documents list; added a maintenance record stating that the spec itself does not change current runtime architecture.
- Corrected the shutdown design against the locked Iroh ProtocolHandler contract: Router owns accept futures, so the new handler must not create a second task-tracking layer.
- Corrected the layer boundary after user review: Application no longer receives a duplex or knows Iroh. Infra completes connection, OPAQUE/continuation auth, wire conversion, and typed reply sending; Application handles one authenticated typed message at a time through a transport-agnostic endpoint.
- Verified required headings, 30 edge scenarios, 47 acceptance items, local document links, tracked diff whitespace, locked metadata, and architecture preflight.
- Repository-wide format remains blocked by existing differences in `crates/uc-engine/src/assembly/sync_engine.rs` and `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs`.
- Workspace all-target check remains blocked by the existing unfinished Infra migration: 17 library errors and 31 library-test errors, including removed store/completion ports and incomplete outbox error mappings.
- Final spec check passed: all 11 sections, 30 edge scenarios, 47 acceptance items, local links, untracked-file whitespace, and the explicit no-Iroh-in-Application rule.
- Final architecture preflight, locked metadata, and tracked diff checks passed.

## Verification

| Check | Result |
| --- | --- |
| Current application library tests | 662 passed before the spec-only task began |
| Current Engine library check | Failed in Infra with 17 unfinished migration errors |
