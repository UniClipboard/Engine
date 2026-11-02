# Rust fault injection in the PR343 follow-up

Status: research recommendation; no refactor implemented or tested.
Date: 2026-11-02.
Task: `pr343-rust-fault-cfg-research`.
Workflow: Engine PR343 follow-up architecture research.

## Decision

Reduce the conditional-compilation scatter, but retain probes at the actual owner and native-stream boundaries. Use small, private, domain-specific enabled/disabled modules for observational state and context propagation. Retain explicit conditional blocks where removing them would introduce default-path expression evaluation, asynchronous selection, or altered cancellation. Do not introduce a general instrumentation framework, public workflow-stage API, or new dependency for this change.

This is an architecture recommendation, not an official Rust rule. Rust specifies conditional compilation and Cargo feature resolution. It does not prescribe a universal fault-injection architecture.

## Scope and evidence identity

The inspected repository is UniClipboardEngine, at HEAD `dfcd32195c7d1e463b54b7ee059336258837a7db`. The branch is `test/engine-343-e2e`. Initial tracked changes were empty. The existing untracked path `target` was preserved. Raw subprocess output confirmed the branch and status after the initial condensed display joined those two values. The Iroh submodule status matched `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`. No fetch, remote shell, Cargo command, build, test, or E2E run was performed.

The supplied [Engine PR URL](https://github.com/biolimbo/Engine/pull/1) could not be retrieved by the web tool. Its current remote base and head are therefore unverified. Local history establishes the following narrower comparison:

- `032d273656cb3e366f9c057a82f86f4bdaa1295d` is the parent of follow-up commit `cd774b4c5db78c0470d2cdbefd2e4bef1c1cb36f`.
- No Rust occurrence of this feature exists at that parent, or at the laboratory comparison base `80bda137541f1a054897f8d1474634dac9d752e8`.
- All current Rust occurrences were introduced in `cd774b4c`. They are follow-up additions, not inherited occurrences in the upstream PR343 head. The task's current baseline already contains them.

Repository references below use paths relative to the repository root and line numbers at the inspected HEAD. The root rules, Application rules, documentation rules, architecture, Rust style, error handling, security, and observability rules were read. The explicit task authorization requires this report in English despite the repository's default Chinese documentation rule.

## Exact inventory

**HIGH confidence.** Enumerate tracked Rust files with `git ls-files '*.rs'`. Read their text and select lines containing both `#[cfg(` and `clipboard-stream-fault`. Verify each match against its enclosing code. Count attributes, not affected statements or semantic hook invocations. All matching attributes occupy one line. A second search for the feature string identifies the separate `cfg!` capability check and manifest wiring. This method excludes ignored artifacts, third-party submodule contents, documentation, and this report.

There are **103 cfg attributes in 18 Rust files**: 93 positive and 10 negative. Application has 80, Infra P2P 18, Engine 1, and the connectivity host 4. The host additionally has one `cfg!` at line 803 and its capability-name string at line 804. Thus a raw string-line search gives 105 Rust lines, not 105 cfg attributes. Four Cargo manifests declare/forward the feature: Application:10, Infra P2P:12, Engine:12, connectivity host:35.

The categories below are mutually exclusive bookkeeping labels. A single block may contain multiple operations:

- **M (13):** module/export/host control wiring, including the host import and command branches.
- **I (16):** non-host imports.
- **S (5):** fields, initializer field, parameter, and matching call argument.
- **W (18):** paired enabled/disabled future or task-local wrappers.
- **J (44):** observation/journal calls, helper definitions, context captures, observation locals, and one feature-off unused-parameter binding.
- **B (7):** behavioral probe blocks: async hold, RESET, preparation gate, acquisition wait, or ACK hold.

| File | Attributes | Category and exact attribute lines |
| --- | ---: | --- |
| `crates/uc-application/src/clipboard/active/mod.rs` | 3 | I: 11; J: 432, 452 |
| `crates/uc-application/src/clipboard/active/transport/pull.rs` | 3 | I: 7; B: 43; J: 67 |
| `crates/uc-application/src/clipboard/inbound/runtime.rs` | 4 | I: 4; J: 181, 187, 204 |
| `crates/uc-application/src/clipboard/mod.rs` | 2 | M: 32, 35 |
| `crates/uc-application/src/clipboard/sync/active_state/apply_inbound.rs` | 17 | I: 31; W: 256, 260, 468, 472, 507, 509, 607, 609; J: 466, 474, 511, 593, 599, 635, 644; B: 601 |
| `crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs` | 4 | I: 11, 17; J: 473, 507 |
| `crates/uc-application/src/clipboard/sync/sync_runtime/recovery.rs` | 10 | I: 1; W: 464, 474; J: 364, 403, 435, 479, 514, 523, 603 |
| `crates/uc-application/src/clipboard/sync/sync_runtime/recovery/schedule.rs` | 17 | I: 16, 18; S: 38, 47, 75; J: 70, 72, 88, 96, 154, 192, 210, 220, 233, 244, 255, 277 |
| `crates/uc-application/src/clipboard/write/coordinator.rs` | 7 | I: 17; W: 269, 271; J: 259, 338, 343, 346 |
| `crates/uc-application/src/connectivity/acquisition.rs` | 3 | I: 6; W: 76, 78 |
| `crates/uc-application/src/connectivity/manager.rs` | 5 | I: 9; W: 518, 520; B: 352; J: 513 |
| `crates/uc-application/src/connectivity/mod.rs` | 1 | M: 41 |
| `crates/uc-application/src/deps.rs` | 4 | M: 674, 680, 687, 694 |
| `crates/uc-engine/src/lib.rs` | 1 | M: 40 |
| `crates/uc-infra-p2p/src/network/clipboard/dispatch.rs` | 12 | I: 2, 9; S: 44, 155; B: 46, 56, 171; W: 132, 135; J: 116, 119, 140 |
| `crates/uc-infra-p2p/src/network/clipboard/mod.rs` | 1 | M: 16 |
| `crates/uc-infra-p2p/src/network/clipboard/receiver.rs` | 5 | I: 2, 11; J: 238, 240; B: 303 |
| `tests/hosts/connectivity/src/main.rs` | 4 | M: 36, 263, 287, 863 |

The four gated implementation roots are `clipboard/testing_receive.rs` (including `pre_write.rs`), `clipboard/testing_recovery.rs`, `connectivity/testing_fault.rs` in Application, and `network/clipboard/stream_fault.rs` in Infra. Their implementation bodies contain no occurrences of the feature name. Counting only cfg lines understates the implementation surface.

**Quantified recommendation, MEDIUM confidence:** 21 sites (16 imports and 5 type/argument sites) are straightforward candidates for stable private hook types. Another 18 attributes describe nine wrapper pairs and are candidates only after future construction and scope equivalence review. Thus 39 of 103 attributes describe structural duplication rather than distinct fault boundaries. This is not a promise of 39 fewer total attributes: enabled/disabled module selectors must replace them. The 44 J sites can often lose local cfg spelling, but their observation positions must remain. The seven B blocks represent important causal boundaries; there is no evidence-based reason to delete them merely to meet a count target.

## Causal boundaries that must survive

**HIGH confidence for source placement; runtime equivalence remains untested.** The [runner contract](../../scripts/testing/pr343-e2e.md) and actual hook sites establish these requirements.

| Cases / assertion | Necessary source boundary | Why an outer test facade is insufficient |
| --- | --- | --- |
| R01/R02 bounded recovery; independent pull isolation | Infra `clipboard/dispatch.rs:46-76` and Application `active/transport/pull.rs:39-58` | RESET must affect the actual content send half. Active pull has a separate real stream; ordinary push failure does not prove pull failure. Keep native reset errors and existing frame I/O. |
| R02A positive attribution | `active/mod.rs:432-452`, `inbound/runtime.rs:181-216`, `testing_receive.rs:168-245` | Successful readback must identify whether push or active pull actually committed. A transport response alone cannot establish persistence. |
| R03 bounded second online window | `recovery/schedule.rs:27-33,60-98,140-168` and `recovery.rs:396-495` | Retry counts and origin belong to the recovery owner. Keep three delays of 2/5/10 seconds and exhausted-state handling. |
| R04/R04F/R04E late success/failure and result fence | Infra `dispatch.rs:171-184`; Application `delivery.rs:269-328,468-503`; `recovery.rs:403-438` | Hold the actual completed result before return. Observe the actual denied write under the delivery write lock. Do not synthesize a success/failure or let a late result reset a newer budget. |
| R05T terminal refusal | Real receiver ACK mapping and delivery/recovery classification | A fake transient error does not prove terminal refusal survives the quiet window and resume. |
| R05O acquired-before-open | Infra `dispatch.rs:44-54` | The connection is acquired, but no content stream is opened. A facade pause before acquisition is a different experiment. |
| R05I acquisition/suspend | Application `manager.rs:340-366,513-530`, `acquisition.rs:76-79`, `testing_fault.rs:165-286` | Register the real flight, propagate context into its worker, observe the backend's first poll, then hold the caller. Preserve cancellation and the real shared result. First Pending does not prove Pending at suspend. |
| R05A applied-before-ACK | Infra `receiver.rs:238-241,303-326`; Application `testing_receive.rs:294-332` | Capture the cursor before broadcast; correlate a later push Applied event for the authenticated peer/content; hold the actual ACK after persistence. Preserve cancellation outcome and duplicate confirmation after resume. |
| R05G prepared/final gate/native write | `apply_inbound.rs:593-657`, `testing_receive/pre_write.rs:115-160`, `write/coordinator.rs:335-363` | Preparation must precede the final settings gate. Invocation evidence must surround the actual system clipboard call, inside the blocking worker, not only the coordinator call. |

The lifecycle owner still owns suspend/close; hooks only expose or pause those existing transitions. The connectivity manager owns flight registration, leases, worker cancellation, and settlement. The recovery owner owns order, budgets, retry selection, and restart scanning. The delivery write owner owns the current-attempt token and terminal persistence fence. The write coordinator owns serial write and echo-guard cleanup even if its caller stops waiting. No hook may take over those responsibilities.

The stream controller's `Attempt` and `HoldWait` Drop implementations (`stream_fault.rs:391-418`) remove pending completion reservations and held tokens. Acquisition `Wait` and pre-write `Wait` provide their corresponding cleanup. Keep each guard in the same lifetime as the paused operation. Preserve independent hold aliases: releasing an old token must not release a later barrier on the same attempt.

## Feature-off behavior is not the old revision

**HIGH confidence, source comparison only.** Removing all fault attributes from the discussion does not make this follow-up behavior-neutral:

- `apply_inbound.rs:144-147,306-312,456-463,632-642` adds SettingsPort and global receive checks, including the last check before write handoff. `receive_gate.rs:135-160` rejects disabled or unreadable settings. These changes apply with the feature off.
- `delivery.rs:242-328,468-503` introduces shared `DeliveryWriteOwner` state and terminal recovery settlement under the same write lock. Late-result fencing is production behavior, not a journal implementation detail.
- Infra `dispatch.rs:49-54` uses acquisition classification before stream open. `map_acquisition_err` distinguishes denied from unavailable requests. These changes also survive feature-off compilation.
- The write coordinator already used its spawned serial worker and guard cleanup before `cd774b4c`. The diff here adds context capture and recording; it does not establish that cancellation-safe write ownership was newly introduced by this feature.

These are verified examples in the inspected paths, not an exhaustive behavioral audit of the 174-file follow-up. Keep them separate from the proposed instrumentation-only migration.

Feature-on but unarmed is also not literally the same execution: journals use locks, clocks and allocations; task-local wrappers are installed; the manager adds a stop-versus-hook selection; accepted ACK handling adds an outer absolute deadline around its hold and ACK operation. `receiver.rs:303-326` can classify an outer expiry as abandoned, while `emit_ack` has its own timeout and handles I/O internally. Preserve this meaning unless a separate behavior change is approved. Do not claim that enabling the feature has zero overhead or identical scheduling.

## What Rust and Cargo actually guarantee

Sources were retrieved on 2026-11-02. The official documentation URLs are rolling references; the ecosystem examples below are pinned release snapshots, not claims about the latest release.

1. A false `#[cfg]` removes the annotated form. `cfg!` yields a boolean; it does not remove surrounding branches from type checking. `cfg_attr` conditionally applies attributes and can select a module path. These are language mechanisms, not advice to minimize cfg counts. [Rust Reference: conditional compilation](https://doc.rust-lang.org/reference/conditional-compilation.html)
2. Cargo features combine requests for a package. The Cargo Book recommends additive features. `default = []` and `--no-default-features` do not veto another dependency's request for the same feature. The current host forwards this capability through Engine to Application and Infra; a feature name is not an access-control boundary. [Cargo features](https://doc.rust-lang.org/cargo/reference/features.html)
3. This workspace uses resolver 2 (`Cargo.toml:30`). Resolver 2 separates some build/dev/target feature contexts. It does not prevent unification when dev targets are actually built; `--all-targets` can bring that context in. Inspect both forward and inverse graphs for each intended artifact. [Cargo resolver](https://doc.rust-lang.org/cargo/reference/resolver.html)
4. Integration tests are separate crates linked to the library's public API. The dependency library is not automatically compiled as its unit-test target. Replacing this feature with `cfg(test)` would remove controls from the separately built connectivity executable. [Cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)
5. Dev-dependencies are available for tests/examples/benchmarks, are not ordinary library build dependencies, and do not propagate to consumers. A test-support crate can own a driver or fixture; it cannot make a separately compiled library's private stream boundary interceptable. [Cargo development dependencies](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#development-dependencies)
6. Await suspends only when polling returns Pending. An immediately ready empty async function does not inherently yield. This does not guarantee unchanged future size, argument evaluation, drop order or auto traits. [Rust Reference: await](https://doc.rust-lang.org/reference/expressions/await-expr.html)

### Maintained ecosystem patterns, not universal mandates

[Tokio 1.48.0 `loom/mod.rs`](https://github.com/tokio-rs/tokio/blob/tokio-1.48.0/tokio/src/loom/mod.rs) selects private std/mock modules and re-exports their names at one boundary. This supports centralizing implementation selection behind consistent internal names. Its `all(test, loom)` condition serves Tokio's in-crate model checks; copying that condition into this external host would be wrong. Loom also replaces synchronization for model exploration; it does not prove real Iroh stream behavior.

[TiKV 8.5.0 Cargo.toml](https://github.com/tikv/tikv/blob/v8.5.0/Cargo.toml) has a normal `fail` dependency and an explicit propagated `failpoints` feature, separately from `testexport`. This demonstrates an established opt-in test capability, not immunity from feature unification.

[fail-rs 0.5.1 implementation](https://github.com/tikv/fail-rs/blob/v0.5.1/src/lib.rs) has an empty disabled `fail_point!` expansion and an enabled expansion that can return from the enclosing function. Its pause uses a condition variable; sleep blocks a thread. Its callbacks are synchronous. Consequently it is not a drop-in replacement for these asynchronous, cancellation-aware holds. Blocking a runtime worker would distort these experiments. An empty macro excludes argument evaluation at that site; it does not remove unrelated registry APIs from a linked dependency. Native RESET, typed error sources, per-attempt aliases, capacity checks and RAII journals would still need custom integration.

Recommendation: do not add fail-rs now. Existing Tokio, tokio-util CancellationToken, standard synchronization, UUID, serde and error crates already implement the required mechanisms. No Engine-owned Cargo manifest declares fail-rs. Its general configuration strings, global registry and early-return behavior would add a second control model without eliminating the native hooks. If later required for synchronous failure points, evaluate it separately and preserve concrete source errors rather than replacing them with string messages.

## Concrete alternatives

| Alternative | Fit here | Cost or hazard | Recommendation |
| --- | --- | --- | --- |
| Module-level enabled/disabled private implementation, stable hook names | Observational state, capture, results, narrow scopes | Eager arguments still execute; future wrappers can change lifetimes; inline is not a codegen guarantee | Primary approach, with restrictions below |
| Private port/decorator on existing seams | Complete capability input/output; system clipboard call recording | Cannot see flight registration, post-persistence/pre-ACK, or final receive gate from an outer facade | Reuse only where it observes the exact existing boundary; no fake ports |
| Probes retained inside business/native owners | Seven behavioral blocks and journal decision sites | Some explicit cfg remains | Necessary; keep exact causal ordering |
| Separate test-support crate/dev-dependency | Driver utilities, fixture serialization, result validation | Does not grant library-private access; normal host binary cannot use only a dev-dependency | Defer; no benefit sufficient to justify new dependency wiring now |
| `cfg_attr` / `cfg_if` | Local module selection or several exclusive configurations | Changes presentation, not ownership; another macro dependency unnecessary for two branches | Two plain cfg selectors are sufficient |
| Local helper macros | Expensive observation expressions must disappear entirely when off | Hidden await/return/RESET and opaque captures make control flow harder to audit | Use only a narrow, explicit erasure need; never a generic stage DSL |
| Generic hook policy | Compile-time selection without vtable | Spreads type parameters through owners; code growth/compile cost must be checked | Avoid owner-wide generics; generic Future adapters may remain local |
| Dynamic dispatch | Existing ports already use it | New hook vtables, allocations or runtime flags remain in default path unless cfg removed | Do not add a new dynamic instrumentation port |

### Minimal proposed design

Each existing domain keeps its own hooks. Keep `mod.rs` as a selector and exports, with implementation in sibling files. For example, Application's existing `clipboard/testing_receive.rs` can select `testing_receive/enabled.rs` and `testing_receive/disabled.rs`. Move existing state, journals and `pre_write` under the enabled side. Do the equivalent only where useful for `testing_recovery`, `connectivity/testing_fault`, and Infra `stream_fault`. Do not combine them into a cross-domain service.

The disabled side contains private zero-sized tokens and synchronous no-op observation methods. It must contain no global registry, mutex, clock read, UUID generation, serialization, retained content identifier, environment parser, fault command, or cancellation token. Public control exports stay behind the existing feature gates. Keep the existing feature-on test surface compatible during migration; do not promote `ReceiveStage`, `RecoveryEventKind`, or any stage query into the stable Engine contract. The current `testing_stream_fault` module is already public Rust surface when enabled; its test-only intent does not make it private to Cargo consumers.

Illustrative migration at `clipboard/active/mod.rs:432,452`:

```rust
// Current source (two separate positions around real apply):
#[cfg(feature = "clipboard-stream-fault")]
current_stage(ReceiveStage::ApplyStarted);
// ... actual apply ...
#[cfg(feature = "clipboard-stream-fault")]
current_apply_outcome(&outcome);
```

```rust
// Proposed pseudocode; names and visibility are not implemented.
// The private implementation maps these named operations to existing events.
receive_probe::apply_started();
// ... unchanged actual apply ...
receive_probe::apply_finished(&outcome);

// Disabled implementation:
#[inline]
pub(crate) fn apply_started() {}
#[inline]
pub(crate) fn apply_finished(_: &InboundClipboardApplyOutcome) {}
```

This hides the event vocabulary and behavior selection, not the actual business decision. It is not a new dependency-injected empty business capability. The module has the substantive responsibility of owning fault state, correlation, and observation; avoid chains of pure forwarding aliases, per the Rust style rules.

Current `connectivity/acquisition.rs:76-79`:

```rust
#[cfg(feature = "clipboard-stream-fault")]
let connected = testing_fault::poll_backend(connecting).await?;
#[cfg(not(feature = "clipboard-stream-fault"))]
let connected = connecting.await?;
```

Possible replacement, explicitly pseudocode:

```rust
let connected = testing_fault::poll_backend(connecting).await?;

// Disabled implementation returns the SAME future rather than awaiting in a new one.
#[inline]
pub(super) fn poll_backend<F: Future>(future: F) -> F {
    future
}
```

The enabled implementation retains the current first-poll observation. A matching signature need not imply an identical concrete return type across separate builds. Keep the existing Future construction before this call. Check Send, borrow lifetimes and drop behavior in both builds. An identity helper is justified here as configuration selection, not as an extra business wrapper.

In `recovery/schedule.rs`, replace the two optional observation fields with private observation state types whose disabled form is zero-sized. Keep rounds, due, entry and selection logic in RetrySchedule. Move event construction and clocks inside enabled methods. Do not eagerly build `EventInput` or call `Instant::now()` solely to pass it to an empty function. Preserve previous-due capture before mutation and origin assignment only when the deadline changes. Keep source-event capture/reset semantics, including early return paths; redesigning them requires a separate causal review.

### Explicit exceptions and semantic traps

Keep cfg at module selection and feature-only exports/host commands. Initially keep the actual hold/RESET/ACK branch cfg blocks. Do not replace a removed block with `select!` containing an always-ready no-op hook: that can poll the close branch, change branch priority, and alter observable behavior even when the hook itself cannot wait.

A no-op async function still evaluates its arguments and constructs a future. Moving an expression into an async body defers its evaluation until poll. Moving a clone, lock, timestamp, error conversion, or guard across that boundary can change behavior. A lazy closure avoids eager work but can still capture/move values and change drop timing. Prefer borrowed existing values and synchronous no-op observations. Retain cfg when exact erasure matters more than reducing syntax.

Task-local context is not a process-wide value and must be propagated into newly spawned and blocking work deliberately. Tokio documents initialization at polling and scoped lifetime: [Tokio 1.48.0 LocalKey](https://docs.rs/tokio/1.48.0/tokio/task/struct.LocalKey.html). Preserve the current capture-before-spawn points in manager and coordinator. Do not place the scope only around future creation, omit the worker scope, or spawn a new worker just to hide cfg.

Keep the original absolute budgets and guard ownership. Pre-write holds retain the 30-second expiry, shutdown/reset priority, capacity 64 and cancellation record. Stream holds retain limit 64 and the 256-event overflow accounting; acquisition/receive journals retain their existing limits and fail-closed overflow/epoch handling. Do not convert native errors into injected strings. No new await belongs between the final settings gate and write handoff.

Feature-off zero capability means there is no arm/release/snapshot/control surface and no state capable of inducing a fault. It does not mean a source-level `#[inline]` proves zero instructions, identical future layout, or identical performance. Those require separate codegen/measurement evidence. Feature-on privacy remains essential: in-memory maps correlate real identifiers, but exported journals must contain only existing opaque aliases and fixed categories. Do not persist maps or export raw peer/content/path values.

## Migration slices and acceptance risks

All slices below are future work. Do not mix production fixes with the hook migration.

| Slice and files | Principal risk | Required acceptance evidence |
| --- | --- | --- |
| 1. `clipboard/testing_receive*`, `clipboard/active/mod.rs`, `clipboard/inbound/runtime.rs` | Losing path attribution or constructing identifiers while disabled | Both type configurations; real push/pull attributed commit and hash readback; no disabled registry/API |
| 2. `testing_recovery.rs`, `recovery.rs`, `recovery/schedule.rs`, `delivery.rs` | Moving observation across mutation/lock; changing retry source or result fence | R01–R04E exact budgets, schedule causes, old-result fence, exhausted/new-item cases |
| 3. `connectivity/testing_fault.rs`, `manager.rs`, `acquisition.rs` | Task-local loss; first-poll substitution; different cancellation | R05I registered flight + native first Pending; same stopped/shared-source outcomes and cleanup |
| 4. Infra `clipboard/stream_fault.rs`, `dispatch.rs`, `receiver.rs`; Application `active/transport/pull.rs` | Fake RESET or ACK result; widened deadlines; hold leakage | R01/R02/R02A, R04 family, R05O/R05A, real streams and exact terminal events |
| 5. `apply_inbound.rs`, `write/coordinator.rs`, `testing_receive/pre_write.rs` | Gate moved behind wait; serial/echo guard released early | R05G preparation, final rejection, actual write invocation/readback, cancel cleanup |
| 6. Existing manifest/export/host wiring only if needed | Accidental feature propagation or larger Engine API | Narrow and normal compiled graphs; capability rejection when off; no dev-tools/test-util leakage |

Start with one complete observation slice and its real E2E evidence. Preserve the working implementation until the replacement proves the same causal chain. Do not add unit tests after implementation. Do not invent fake ports or isolated tests to meet a style target.

## Verification plan — NOT RUN

A separately authorized implementer must use the shared repository target and cache, serialize Cargo work, and record the final source SHA, dirty state, Iroh SHA, runner hash, executable hash and compiled feature identity. Required checks include:

1. Run locked metadata, workspace/all-target checks, formatting, Rust style, repository checks and diff checks under the repository commands. This research task intentionally does not execute them.
2. Type-check and build the connectivity host separately with `--no-default-features --features production-e2e`, and with `--no-default-features --features production-e2e,clipboard-stream-fault,network-test-probe`. Check affected library configurations too. A combined all-features/all-target run cannot substitute for the off build. Retain resolved inverse graphs and compiled fingerprints, not just forward dependency trees. Neither narrow nor normal host may include Infra `test-util` or Engine `dev-tools`.
3. Verify absent feature-off control APIs and host commands. If a zero-codegen claim is required, inspect release output separately; do not infer it from inline annotations or absent strings alone.
4. Run each slice's causal cases, retaining original timeouts, caps, assertions, native streams and failed attempts. Reconcile journal overflow, release versus cancellation, task-local attribution, original error chains and cleanup. Unarmed feature-on and actual feature-off controls are distinct.
5. Re-run the final fixed **120 main / 69 nested / 12 companion** matrix and the **complete7** Engine scenarios. Preserve case identity and independent ledgers. The seven cover distinct-content cancellation, shared-content cancellation, partial-resource readability, transient read recovery, response timeout recovery, presence-failure isolation, and timeout/sibling/healthy reuse. Do not replace complete workflows with unit tests merely because the existing Rust harness uses `--lib`.
6. Retain reproducible sanitized `result.json`, source and binary hashes, causal journals and per-case cleanup/privacy results. Use the [committed validation summary and fixed matrix](../exec-plans/completed/2026-11-01-pr343-linux-validation.md) and [runner guide](../../scripts/testing/pr343-e2e.md). Recorded historical totals are inputs to this plan, not new passes or proof of the future refactor.
7. Keep **real Tailscale smoke12** separate, under its [own plan](../exec-plans/active/2026-11-02-real-tailscale-e2e.md). It cannot replace the laboratory fault cases. Native GUI, platform keychain, physical sleep/wake and mobile acceptance remain separate as well.

## Limitations and open decisions

- The remote PR comparison is unavailable. Local parent-based attribution is exact; remote PR membership is not independently confirmed.
- No runtime or codegen equivalence was measured. The recommendation has MEDIUM confidence until both configurations and causal E2E pass.
- The precise net cfg reduction depends on the chosen private module interfaces. Removing 39 structural sites is a candidate surface, not an acceptance quota.
- Decide whether a documented no-state/no-control default is sufficient, or whether the product requires measured release-codegen equivalence. The latter favors retaining explicit cfg around more call sites.
- The public feature-on testing export already exists. Keep it out of the stable contract and do not enlarge it. Any future removal or relocation needs a separate host migration decision.
- No generic release policy was changed. Production artifact selection should explicitly exclude the fault feature; resolver 2 and naming conventions cannot enforce this alone.

## Execution status

Completed: local state/history checks; complete textual cfg inventory and categorized line map; targeted owner/native-boundary tracing; source comparison for relevant feature-off behavior; official Rust/Cargo research; pinned Tokio, TiKV and fail-rs examples; this report.

Not executed: code refactor, dependency changes, Cargo/build/type checks, tests/E2E, remote commands, commits, pushes, PR comments, publication or delegation. Only this English report was written. The recommendation is to reduce structural scatter while preserving every causal boundary and the existing production owners.

## Coordinator verification

The coordinator independently recounted103 cfg attributes in18 tracked Rust files,
including10 negative attributes and one separate cfg macro. The per-file counts match
the inventory above. All relative document links resolve.

Before publishing this report, authenticated GitHub CLI readback confirmed
`biolimbo/Engine#1` targets `fix/lan-sync-wireguard` at
`032d273656cb3e366f9c057a82f86f4bdaa1295d`, with head
`UniClipboard/Engine:dfcd32195c7d1e463b54b7ee059336258837a7db`.
This closes the remote-identity limitation for that snapshot; the Researcher did not
perform that remote verification. Adding this report does not change the inspected Rust.

The coordinator checked the Cargo feature-unification rule and fail-rs blocking pause
against their primary sources. The recommendation to concentrate configuration selection
is accepted as repository-specific architecture guidance. It is not a promise to remove
exactly39 attributes. Async and cancellation equivalence still require implementation
and the planned E2E evidence. No refactor or new runtime validation is part of this report.

Publication checks passed under the coordinator: locked metadata, locked workspace/all-target
check, workspace formatting, Rust style, repository preflight and diff checks. These checks
validate the current checkout, not the proposed refactor. No E2E was rerun for this document.
