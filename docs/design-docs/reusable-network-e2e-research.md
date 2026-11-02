# Reusable network E2E research and implementation design

Status: implementation proposal; no implementation or runtime acceptance performed.
Date: 2026-11-02.
Task: `reusable-network-e2e-research`.
Workflow: Engine PR343 follow-up, [Engine PR 1](https://github.com/biolimbo/Engine/pull/1).

## Objective and decision

Make the existing complete Engine network scenarios reusable across future source revisions and supported environments. Keep exact delivery, cancellation, recovery, privacy, and cleanup assertions. Replace PR-specific execution assumptions with a small build manifest, an owned environment inventory, and a complete scenario runner. Do not build a framework, scenario DSL, plugin system, or test-only business port.

Recommended default: one `just network-e2e` entry, Node.js orchestration using existing built-in modules, the existing JSON-line connectivity host, explicit environment adapters, and an offline strict verifier. A quick local LAN smoke is the default. Full laboratory coverage and authenticated Tailscale coverage are explicit selections. Python remains only for the existing clock guard or an OS helper that genuinely needs it. Do not maintain a second Python scenario implementation after the Tailscale migration passes.

The complete scenario owner is the network runner's `runScenario` implementation. Its caller submits one suite/case selection and receives a terminal result plus an artifact directory. The owner performs readiness, fresh Engine setup, stimulus, assertions, evidence collection, and per-run cleanup. Environment helpers own resource leases, not scenario order. Application remains the owner of product retry and restart recovery. The harness must never resend or extend a product deadline to manufacture success.

## Scope, source identity, and evidence limits

Verified local repository: UniClipboardEngine, branch `test/engine-343-e2e`, HEAD `b70edf1e6b25f400524b7e7191a394c3d2dcfcf8`. The expected HEAD matches. Initial tracked changes were empty. `git status --porcelain=v1` showed one existing untracked entry, `target`; it was preserved. The initialized Iroh submodule was `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`. No claim is made about cleanliness inside that submodule beyond this checkout identity. The public PR page could not be retrieved by the web tool; remote branch/base/head were not independently verified.

Read-only investigation covered root and documentation rules, [architecture](../../ARCHITECTURE.md), core beliefs, engineering/Rust/error/security rules, the [business observation standard](observability.md#业务记录组织标准), [testing guide](testing-guide.md), [testing architecture](testing-architecture.md), runners, host feature wiring, complete Engine scenarios, committed acceptance ledgers, CI, and `justfile`. The explicit assignment authorizes English for this report. No other file may be edited.

Root supplied the live environment snapshot: `primaryserver`, one 7 CPU/8 GiB VPS; three authenticated Running kernel-TUN clients `uc-pr343-ts-a/b/c`, distinct network namespaces, same tailnet; three sleeping `uc-pr343-ts-engine-a/b/c` workloads; no Engine processes. Node state is private tmpfs. Container restart loses that state. These are supplied operational facts, not live observations by this Researcher. Recheck them before execution. The local Mac has Tailscale, but no cross-host Engine payload case has run.

Historical acceptance is narrower than this proposal:

- The [laboratory report](../exec-plans/completed/2026-11-01-pr343-linux-validation.md), [mandatory matrix](../exec-plans/completed/pr343-linux-validation/mandatory-matrix.json), and [ledger](../exec-plans/completed/pr343-linux-validation/acceptance-ledger.json) record **120 main / 69 nested / 12 controls**, plus **7 complete Engine cases**. Their frozen source manifest is `d77589ca363497bcbaa41a16b7b6e202ea547de2e4e0f5819d272b91aa04e540`. These are accepted historical records, not new results for the current HEAD.
- The [Tailscale smoke report](../exec-plans/completed/2026-11-02-tailscale-smoke.md) records **one accepted smoke12 attempt**, all six directed node pairs, text and 512 KiB files. [Attempt 08 provenance](../exec-plans/completed/pr343-tailscale-smoke/smoke-08-provenance.json) identifies the frozen executable and deployed controller. Attempts 01–07 remain failures. This does not establish repeated stability.
- Dedicated IPv6 payload, continuous Tailscale path classification, cancellation on real Tailscale, full T00–T11, cross-host/NAT/WAN, native clipboard/keychain/GUI, mobile, and physical suspend/wake are not established by that smoke. The earlier laboratory report's `actual Tailscale: NOT_RUN` remains correct for its own run.

Confidence: HIGH for the local source observations below; MEDIUM for the proposed design until migration acceptance; UNKNOWN for current remote readiness and unexecuted coverage. No historical artifact was promoted to a current run.

## Findings and ownership inventory

Line references in this section refer to the verified HEAD. Paths are repository-relative.

| Mechanism and primary evidence | Current owner and useful behavior to keep | Concrete gap or boundary |
| --- | --- | --- |
| [pr343-e2e.mjs](../../scripts/testing/pr343-e2e.mjs), `Host` 211–302, `Run` 304 onward, final loop 2227–2263 | Real subprocesses, encrypted profiles, actual readback, native fault journals, fixed deadlines, cleanup/privacy override, stop repetitions after failure | Scenario logic, network setup, JSON protocol, evidence writing, and lifecycle share one 2263-line file. Pairing/online use `1 - node.index`. CLI selection is a prefix filter. Result completeness is selected-case completion, not mandatory coverage. |
| Same file, `Run.prepare`/`wireguard` 319–381, scenarios 764 onward | Kernel WireGuard and real namespace faults, positive controls, actual multicast/rebind/subnet behavior | `vpn` means synthetic kernel WireGuard, not Tailscale. Fixed subnets, ports, and node count are embedded. L01 is WireGuard, not a minimal plain LAN case. L04 at 792–802 is the existing plain LAN changed-port/reconnect/text/file slice. |
| Same file, `Run.transfer` 497–553 | Random synthetic content, hash readback, explicit sender settlement, bounded readback observations | Requires exactly one delivery row. That assumption fails with three trusted members. Use exact requested target identity; do not relax settlement. |
| Same file, `transportRegression` 726–762, S04 1046–1060 | Child-suite mandatory IDs, child process result, privacy/cleanup and binary hashes checked | Current check proves child bytes match supplied paths, not that those paths came from the requested revision/build role. Requires external regression, relay, legacy artifacts. Preserve the 11 direct + 1 known-peer + 9 relay + 2 legacy cases per repetition. |
| [run-pr343-e2e.sh](../../scripts/testing/run-pr343-e2e.sh), entire file | Sources cache environment; separately builds production-policy fault host; rejects Infra `test-util` in inverse graph; builds invitation fixture | Only a partial build contract. `UC_E2E_SOURCE_REV` can override a label without selecting source. Normal host, baseline, regression, relay, complete7, frozen binaries, and durable manifest are outside this entry. |
| [connection-recovery-network.mjs](../../scripts/testing/connection-recovery-network.mjs), options 17–68, `Host` 117–205, setup 588–614, cleanup 1189–1270 | Existing direct/known-peer/relay/legacy coverage, useful shared delivery/progress/session helpers, interruption handling | Another FIFO protocol/process/cleanup implementation; 130 s command watchdog versus 120 s in the PR runner. Fixed `10.233.0.1` bridge and relay ports 19090/19092 require serialization in the same owning environment. Diagnostic modes must not become acceptance modes. |
| [tailscale-engine-smoke.py](../../scripts/testing/tailscale-engine-smoke.py), constants 16–18, `Host` 39–159, preflight 162–206, transfer 209–247, main 250–423 | Local and mounted binary hash checks; task ownership checks; UID-constrained policy; actual UDP controls; all-member online/usable gate; exact requested-target settlement; distinct failed attempts | Fixed hashes, names, label, UID10001, `/opt/uc`, `/profiles/test`, chain name and ABC population. IPv4 preflight only despite dual-family rules. No immutable general build/run contract. Final artifact is written in `finally`, so controller loss can leave no result. Exceptional name-based `pkill` is too broad for shared future workloads. |
| [connectivity Cargo.toml](../../tests/hosts/connectivity/Cargo.toml), features 12–43; [main.rs](../../tests/hosts/connectivity/src/main.rs), dispatch 288–370, capabilities 785–840, startup 858 onward | Existing stable Engine operations through a private pipe protocol; feature-only native controls; in-memory secure storage carried over controlled host restarts | Default `current-engine` enables `uc-engine/dev-tools`. Production acceptance must explicitly disable defaults. Capability strings and `UC_HOST_ENGINE_REV` are diagnostics, not build attestation. No public TCP control listener is needed. |
| [stream_isolation.rs](../../crates/uc-engine/src/testing/host_adapter_contract/receive_retry/stream_isolation.rs), modules 29–38, cases 511–528; sibling response/presence files | Seven complete real Engine scenarios, real QUIC/blob/storage, original attempt/deadline and sibling/native ownership assertions | Keep them in their private Rust test assembly. `--lib` does not make these unit-level substitutes. Four stream cases use assertions; three response/presence cases write dedicated structured artifacts. A uniform outer manifest must bind all seven exact test names and their binary; do not claim seven identical JSON reports already exist. |
| [testing architecture](testing-architecture.md), sections 1–5; [uc-testkit](../../tests/uc-testkit/src/lib.rs); [process fixture](../../tests/hosts/connectivity/tests/common/process.rs) | Existing Scenario identity/budget/resource/report primitives and nextest process scheduling; Rust fixtures already provide private pipes | Do not port shell networking into a new Rust framework. Reuse testkit only for Rust lifecycle/report gaps. Do not expose private Engine internals to move tests into public integration modules. |
| [build-upgrade-anchors.sh](../../scripts/testing/build-upgrade-anchors.sh), 1–14, 56–72, 121 onward | Existing immutable-revision anchor pattern, host adaptation/feature/patch digest, locked build concepts | Its snapshot target policy and cache key are not the complete new manifest contract. Do not invoke its broad cleanup or automatically fetch/build unrelated anchors. Reuse reviewed role metadata/patches only where applicable. |

**HIGH confidence, static gap:** the outer PR result records candidate and driver hashes, while mandatory selection, complete7, controls, and provenance are coordinated externally. The Tailscale script has strong frozen hash checks but cannot accept a future build safely without editing constants. Neither mechanism is a general current-checkout acceptance entry. This is a consolidation need, not evidence that the accepted historical run was invalid.

## Smallest proposed architecture

The following files and commands are **proposed**, not currently available.

| Proposed path | Substantive responsibility |
| --- | --- |
| `scripts/testing/network-e2e.mjs` | CLI parsing and dispatch to build, environment lifecycle, run, verify; no scenario bodies |
| `scripts/testing/network-e2e/run.mjs` | Complete attempt owner, preflight, scenario selection, immutable expected-case set, failure classification, cleanup and finalization |
| `scripts/testing/network-e2e/host.mjs` | Private JSON-line protocol, process generation, bounded requests, secure-storage memory, readers and awaited exit |
| `scripts/testing/network-e2e/build.mjs` | Exact checkout/commit selection, build lock, role build recipes, source snapshot and output manifests |
| `scripts/testing/network-e2e/environment.mjs` | Immutable inventory validation, exclusive leases, readiness/auth state, owned-resource reconciliation |
| `scripts/testing/network-e2e/environments/local-lan.mjs`, `kernel-wireguard.mjs`, `tailscale.mjs` | Actual topology creation/attachment, spawn specification, native route/fault evidence and cleanup; no business assertions |
| `scripts/testing/network-e2e/scenarios/delivery.mjs`, `discovery.mjs`, `address-policy.mjs`, `recovery.mjs` | Existing complete scenario families, ordinary functions, explicit prerequisites and original assertion logic |
| `scripts/testing/network-e2e/verify.mjs` | Offline integrity, expected-source/build/role, exact coverage and causal acceptance verification |
| `scripts/testing/network-e2e/suites.json`, `fixtures.json` | Small finite suite/case table and frozen role inventory; no executable expressions or configurable business steps |

Add files only when their slice needs them. Do not first create empty adapter scaffolding. Reuse `connection-recovery-session.mjs`, `clipboard-delivery-evidence.mjs`, relay/progress helpers where their semantics match. Do not force accepted/duplicate-specific scenarios through a helper that requires `total_accepted == 1`.

Internal essential interfaces, not public Engine APIs:

- `runScenario({caseSpec, build, lease, artifactWriter}) -> CaseResult`: owns the entire case. CaseSpec names a real function and required environment capabilities, host role, node roles, assertions and budgets.
- `environment.acquire(inventory, runIdentity, requiredCapabilities) -> lease`: exclusive ownership plus verified current namespace identities. The adapter returns a spawn specification and narrow named operations such as changing an owned interface or sampling an owned route predicate. It cannot return a synthetic product result.
- `HostSession.start(spawnSpec, startup)`, `call(command, fields)`, `stop()`: one implementation across local process, `ip netns exec`, and `docker exec -i`. Explicit restart retains secure storage only in controller memory and only for the same scenario/profile.
- Evidence writer accepts allowlisted facts and terminal case results. Verifier independently evaluates them against the selected suite and build manifest. The scenario cannot redefine the required matrix after execution.

Keep FIFO protocol version `uc_connectivity: 1` for the first slice. Serialize outstanding requests per HostSession; no request ID retrofit is needed. Bound line/buffer sizes for the existing 512 KiB JSON byte arrays and bounded journals. Reject malformed marked responses, duplicate response, EOF, timeout, process-generation mismatch and output overflow. Do not persist raw requests/responses. Reject all pending requests on exit; drain readers before finalization. On timeout, a late response must not satisfy a subsequent request. Stop the owned process generation. Retain the 120 s PR/Tailscale watchdog and the existing 130 s regression watchdog as explicit role contracts initially; unifying them is a separate behavior change.

### Scenario reuse and support matrix

Use logical node roles and an explicit directed edge list, not arithmetic peer indexing. Resolve device identities in memory after pairing. Check expected identities are distinct, in the same intended space, online where required, and usable in each required direction. Do not use display position as identity. For one-target delivery, require exactly one row for the requested target; record other member states separately. A non-target `pending` row is neither a failed attempted send nor proof of non-delivery. Negative third-member isolation requires its own fresh-content observation and positive control.

| Scenario semantics | Local LAN | Kernel WireGuard lab | Real Tailscale |
| --- | --- | --- | --- |
| Pair, directed text/file hash and target settlement | Supported | Supported | Supported with actual route/confinement controls |
| All six directed pairs with three nodes | Same shared delivery function, new execution evidence needed | Same, new execution evidence needed | Preserve frozen smoke12 semantics |
| Changed-port mDNS rediscovery (L04) | Existing case; first LAN slice | Unsupported without multicast by design | Unsupported as a Tailscale-only mDNS claim |
| Learned/configured address and cold reconnect | Only the original topology-qualified cases | Preserve L02/L03/L03C/L05 semantics | Adapt actual tail address, reverse-dial controls and cold-process proof; not yet passed |
| S03 public candidate and dynamic subnet/NFLOG timing | Only listed native prerequisites | Preserve exact laboratory topology | Do not remap arbitrary public subnets onto a tailnet and call it equivalent |
| R01–R05G native RESET/hold/receive/acquisition gates | Only explicitly supported case variants | Preserve existing definitions | Future extended cases need the same feature-on host and real causal barriers, never a shared UDP drop substitute |
| S04 Engine direct/relay/legacy | Existing child suite semantics | Preserve its own environment | Engine relay and Tailscale transport are separate dimensions; not implicitly portable |
| complete7 private ReceiveStreams assembly | Keep existing full Rust execution | Keep existing full Rust execution | Unsupported initially; a normal tailnet host cannot supply these private fixture seams |

Suite expansion produces an explicit row for every requested combination. Unsupported means `SKIPPED` with a fixed reason and unmet capability. Missing required capability means `BLOCKED`, not a smaller passing suite. `local-lan` means Engine P2P on a local network, not the independent LAN compatibility product.

Proposed suite names:

- `local-smoke` (default): L04, two fresh normal production-policy hosts, one repetition, same original changed-port/text/file assertions. Linux isolated LAN initially; macOS without the adapter is explicitly blocked. No latency claim before measurement; no full rebuild when the exact valid build already exists.
- `lab-full`: exact 40 original main IDs × 3, 23 nested S04 IDs × 3, two normal feature-off and two same-binary observer-disabled controls × 3, plus seven exact complete Engine names. Totals remain separate: 120/69/12/+7. Keep S05 baseline role and both legacy directions.
- `tailscale-smoke`: normal host, three nodes, exactly 12 transfer tuples and the original IPv4 controls. This does not imply dedicated IPv6 or a fixed Tailscale transport class.
- `tailscale-extended`: selected named groups from the existing [T00–T11 plan](../exec-plans/active/2026-11-02-real-tailscale-e2e.md). List unimplemented/unauthorized groups explicitly. No automatic full Cartesian product.

## Proposed developer command contract

One stable public entry: `just network-e2e <action> ...`, backed by the Node CLI. The existing `just cargo`/MBX wrapper remains the sole build path. Examples below assume separately authorized execution, existing local source objects, and an approved environment inventory. They have not been run.

| Action | Contract and output |
| --- | --- |
| `build --source checkout --suite local-smoke` | Freeze current input identity; build only required roles under one Cargo owner; publish immutable `build.json` and role binaries after successful locked builds. Default checkout allows recorded dirty development source; release-like gates separately require clean input. |
| `build --source commit:<full-sha> --suite lab-full` | Resolve an exact locally available commit into an isolated source checkout, never change the user's worktree. Verify initialized submodules. Missing objects/toolchain/fixtures block; no implicit remote fetch. Use canonical shared target/cache serially and freeze each role output immediately. |
| `prepare --adapter tailscale --inventory <private-file>` | Read-only attachment/readiness first. Validate live resources and ownership, report READY/AUTH_PENDING/BLOCKED. Do not recreate authenticated nodes. Creating missing resources requires an explicit `--create` within operator authorization. Local adapters can create their owned disposable resources. |
| `authorize --inventory <private-file> --node <alias>` | Operator-only interactive action after actual pending-state check. Reuse pending daemon. Deliver actual current login URL privately, never via artifact/stdout log/history. No authkey flag. The user completes login. No forced reauth by default. |
| `resume --inventory <private-file>` | Recheck authentication, namespace/process identity and stale leases; return readiness. Does not resume a half-executed product case or silently restart a daemon. |
| `run --suite <name> --build <build.json> [--inventory <file>] --evidence <new-directory>` | Single caller action for complete execution. Locks inventory resources, checks exact requested source/roles, freezes run manifest, runs, cleans, verifies, returns status. Default suite local-smoke and local owned adapter. No build, provisioning, login or retry hidden inside `run`. |
| `verify --run <directory> --expected-build <build.json> --expected-source <source-id> --suite <name>` | Offline, repeatable verification using explicit independent expected identities; no execution or environment mutation. Current checkout source-id can be recomputed locally. Historical verification must name the historical source; it is never presented as a current-checkout pass. |
| `cleanup --run <directory> --inventory <file>` | Reconcile only that run's resources, with current identity checks. Leave authenticated environment alive if still explicitly owned. |
| `cleanup --environment --inventory <file>` | Explicit end-of-task action: stop verified workloads, logout owned clients, remove owned local resources; report admin-node reconciliation separately. Admin removal needs its own authorization and confirmation. |

Use exact case IDs (`--case L04`) and named suites; reject unknown IDs. Compatibility aliases can accept historical prefixes but must print/freeze the expanded set before execution. Disallow `--case` with a claim of `lab-full` acceptance; the result is a subset run.

Exit status proposal: 0 verified selected acceptance; 1 product failure; 2 invalid input/harness/evidence failure; 3 environment blocked; 4 authentication pending; 130 interrupted. Results also retain fixed category and secondary cleanup failures. A valid artifact that reports failure is not a passing acceptance. `SKIPPED`/`NOT_RUN` required rows prevent exit 0. Report execution completion independently from acceptance.

Existing commands, still usable before migration:

```bash
bash scripts/testing/run-pr343-e2e.sh --case L04 --repeat 1 --evidence target/connection-recovery-evidence/new-lan-attempt
python3 scripts/testing/tailscale-engine-smoke.py --binary /verified/integrated-final-normal --fixture /verified/invitation-fixture --evidence /private/new-smoke-attempt.json
just cargo test -p uc-engine --locked --features dev-tools --lib testing::host_adapter_contract::receive_retry::stream_isolation -- --test-threads=1
```

The Python command runs on the authorized Linux Docker host and still requires its exact hardcoded environment. These examples do not authorize this Researcher to execute them.

Proposed ordinary workflow after implementation:

```bash
just network-e2e build --source checkout --suite local-smoke
just network-e2e run --suite local-smoke --build <printed-build.json> --evidence <new-run-directory>
just network-e2e verify --run <run-directory> --expected-build <build.json> --expected-source <printed-source-id> --suite local-smoke
```

Proposed reuse across commits and authorized Tailscale nodes:

```bash
just network-e2e build --source commit:<next-full-sha> --suite tailscale-smoke
just network-e2e prepare --adapter tailscale --inventory <private-inventory.json>
just network-e2e resume --inventory <private-inventory.json>
just network-e2e run --suite tailscale-smoke --build <next-build.json> --inventory <private-inventory.json> --evidence <new-run-directory>
```

If `prepare` reports AUTH_PENDING, the operator runs `authorize` for the affected alias and then `resume`. No expired URL is copied from previous results. `lab-full` uses the same build/run/verify sequence with its named suite; it is not the default developer loop.

## Build and provenance contract

Hashes must bind expected build outputs, not bless arbitrary binaries found at runtime. A runtime `--host` path plus its computed hash is insufficient. A manifest that an untrusted candidate can rewrite is not a security attestation. The trusted build owner freezes the build manifest outside workload write access; the caller retains its expected digest. Runtime compares local, staged, mounted and executed role identities to that manifest before starting and after transfer where applicable. No `--accept-new-hash` escape hatch.

Build identity includes:

1. Repository identity, requested full commit, HEAD tree, actual input file digest set including file mode/symlink identity, index/worktree changes, and explicit dirty state. Include build-relevant untracked source. Record unrelated untracked entries separately; `target` is not clean source by assertion. Never follow it into compiled artifacts. Use a narrow declared exclusion for generated artifact roots, not arbitrary dirty exclusions.
2. Recursive gitlink revisions and actual submodule content/dirty status; lockfile digest; workspace/manifest/build script/config inputs; host and invitation-fixture sources; any exact baseline host adaptation patch and resulting tree. Dirty development snapshots need reproducible source bytes in an approved private source archive, or cannot claim replayability. Do not collect unrelated secrets into that archive.
3. `rustc -Vv`, Cargo version, target triple, profile, exact requested and resolved features per role, relevant Rust/linker flags, source-provenance inputs, cache wrapper identity, platform/compiler/linker or container image digest. Hash runner imports, scenario catalog, verifier, OS helpers and fixture definitions, not only the top-level script.
4. Exact build command/role, successful Cargo output artifact path resolved from build output, length and SHA-256. Snapshot each executable into a content-addressed read-only build directory before the next feature build can overwrite `target/debug/uc-connectivity-host`. Verify source identity before and after building; abort on concurrent mutation. Hold the build lease across each recipe and snapshot.

Build roles are finite and explicit:

| Role | Required provenance/feature rule |
| --- | --- |
| `candidate-normal` | `--no-default-features --features production-e2e`; no `uc-engine/dev-tools`, Infra `test-util`, fault or network probe capability |
| `candidate-fault` | `--no-default-features --features production-e2e,clipboard-stream-fault,network-test-probe`; no dev-tools/test-util |
| `transport-regression` | Existing development feature graph, explicitly labeled; cannot prove production network filtering |
| `complete-engine` | Existing private full Engine test binary with `dev-tools`; exact seven test names and no ignored/filtered-away required case |
| `invitation-fixture` | Codec example built for the selected source; exact source/features/output; identity/expiry-preservation assertions remain mandatory |
| `baseline-profile` | Frozen S05 baseline `80bda137541f1a054897f8d1474634dac9d752e8`, exact host adaptation and submodules; never moving main |
| `legacy-peer`, `relay` | Exact role revision, source/host adaptation/feature recipe, expected binary. Resolve legacy identity from the accepted archive before populating fixture metadata; the hash alone does not invent its source revision. |

Keep the existing frozen normal host `8bdf1f5f15a05278ddaf5bf814d879aafd7b2d52e3a626c473e146f098a6f09d` and invitation fixture `446c03d4726a80fed3f766209b6239ca9eaaa7bb5f96557c956fee1b5a8221c5` as an explicitly historical replay role pair. Preserve their source-manifest association. Do not describe those bytes as rebuilt from HEAD. `fixtures.json` pins historical roles; it does not restrict future candidate commits to a SHA allowlist.

Build normal and fault roles in separate Cargo invocations. Keep inverse feature graphs and actual compiled fingerprints for both. A normal capability response or a forward dependency tree alone is insufficient. Cargo feature unification can enable a dependency feature through another path; resolver 2 avoids only specified classes of unification. This supports retaining separate builds, not trusting feature labels. [Cargo feature documentation](https://doc.rust-lang.org/cargo/reference/features.html#feature-resolver-version-2).

Use the repository canonical target and existing compiler cache. One build owner holds a shared build lease; no parallel Cargo pipelines or `/tmp` target fallback. If target is unavailable, stop and repair it under separate authorization. Rebuild stale input normally; never clear `RUSTC_WRAPPER` to bypass provenance or cache uncertainty. The actual current `target` entry was not validated for building in this research.

## Immutable run artifacts and strict verification

Introduce one independent schema family, `network-e2e/v1`. Do not repurpose old `schema: 1` PR results as if they already implement it. Keep old results unmodified under an explicit legacy importer. Import may support historical replay checks but must not fill missing evidence with invented values. Unknown schema versions fail closed. During unshipped development, evolve this one schema rather than versioning every implementation slice.

Minimal files:

- `build.json`: immutable build identity and role digests, generated by build owner.
- `run.json`: immutable run UUID, attempt UUID, optional previous-attempt reference, build digest, source ID, runner/catalog/verifier digests, suite, exact expected tuples/repetitions, node-role edges, inventory revision digest, topology class, feature/control mode, original budgets. Write before starting Engine.
- `cases/<case>/<iteration>/result.json`: terminal result, exact run/build references, monotonic intervals, requested/actual assertions, synthetic hashes and lengths, target-specific settlements, causal journal references, category, primary and secondary failure, privacy and cleanup.
- `journal.jsonl`: bounded sanitized case start/end and resource phase records. Atomic per-case checkpoints plus a final `result.json`; final completeness is distinct from acceptance.
- `artifact-manifest.json`: relative names, bytes and SHA-256 of every exported artifact except itself. Its digest is retained by the caller/CI artifact record; no circular hash requirement. Paths cannot escape the directory or follow symlinks. A recomputed untrusted manifest alone proves neither provenance nor authenticity.

The inventory's local container names, locations and runtime addresses are private configuration, not exported evidence. Export only opaque aliases, inventory digest and identity-match booleans. Preserve a private inventory copy for operator reuse. Build-source relative paths are provenance metadata; never export runtime user paths, raw identities or content paths. Only synthetic payload hashes are allowed. Keep full invitations, secure-storage values, node keys, auth URLs, raw status, raw errors and packet captures out of artifacts. Scan Engine profiles/database/WAL/logs for canaries before deletion and separately validate exported fields. Do not read credential files merely to scan them.

Verifier algorithm:

1. Resolve the independent expected build/source/suite. Verify manifest and all artifact sizes/hashes; reject unexpected files, invalid paths, malformed/unknown fields, incomplete checkpoints or lost journal/capture data.
2. Check exact source snapshot, dirty-state policy, submodules, lockfile, feature role, host/fixture/runner identities and deployment matches. Historical verification requires an explicit historical expected identity. A changed checkout cannot reuse an older pass.
3. Expand expected tuples before reading observed outcomes. Compare sets **and multiplicities** for `(case, repetition, adapter, role/control, source-node, target-node, kind)` where relevant. Reject duplicates, missing cases, wrong node identity/role, missing controls, mixed run/build IDs, nested artifacts from other attempts, and skips represented as passes.
4. For lab-full, require 40×3 main, 23×3 nested, four controls×3, and the exact seven full Engine tests. Verify child-case semantics and binary roles, not only aggregate counts. Do not count S04's parent as its 23 children. For smoke12 require six directed edges×two kinds and all original route/readback/settlement/privacy/cleanup facts.
5. Check case-specific causal predicates and original deadlines, not just `passed: true`. Preserve exact Delivered requirements where the case demands them; allow Duplicate only where the original contract permits it. Validate cancellation/release ordering, original attempt identity, unchanged retry deadline, sibling completion, capture accounting and clock envelopes as applicable.
6. A single attempt must satisfy its complete selected matrix. Do not take successful rows from failed retries to construct a pass. Preserve failed attempts and stop repetition of a failing case. A later complete attempt may pass, but report prior failure and do not infer stability. Separate approved shards, if ever added, must be declared in the initial manifest; no first-slice sharding is needed.
7. Require per-run cleanup/privacy success and a terminal result. Missing finalization is interrupted/evidence failure. Environment retention is not per-run cleanup failure, but it must have a named active owner and explicit later teardown status.

Failure-mode list for verifier acceptance, defined before implementation: stale-source manifest; swapped normal/fault/legacy role; changed fixture/helper; missing/duplicate tuple; wrong target settlement; truncated journal; symlink escape; missing cleanup; false skip/pass; mixed runs; successful-row retry union; zero selected tests; silent ignored test. Exercise these through copied real E2E artifacts and the actual verifier CLI, retaining each rejection result. This is end-to-end artifact verification, not post-code unit testing.

## Reusable environment ownership and recovery

The simplest secure default is to reuse live authorized clients for the bounded task and keep daemon state in private tmpfs. Do not restart or replace them to make preparation idempotent. Node loss requires new actual authorization. A persistent plaintext Docker state volume is not an approved credential-store exception.

An immutable environment inventory needs only: schema/revision, environment/task owner alias, expiry/review time, adapter, physical-host/topology aliases, node aliases, existing container/resource handles and ownership label, expected images, workload UID, interface role, profile mount root, supported capabilities and maximum concurrent leases. No secret material or login URL belongs in it. Runtime IPs, PIDs and network namespace inodes are discovered and validated, not treated as stable configuration. Changing membership or image creates a new inventory revision. Store leases and run journals separately; do not mutate the inventory with progress.

Lease rules:

- Default one run per environment. Acquire an exclusive OS-backed lock plus an owner record before policy/profile/process mutation. For Linux, use existing `flock` through an owned supervisor rather than a new lock service. Lock lives on the environment owner host; a controller-local lock alone cannot exclude another controller.
- Persist only sanitized ownership references; inspect actual container ID, task label, process start time, cgroup and network namespace identity before acting. PID/name alone is insufficient. Detect namespace replacement and duplicate node identities. Unknown processes/policies block reuse; never delete them.
- Fixed S04 bridge/relay resources get one additional environment-wide lease shared by old and new invocation paths. Default serial scheduling also protects the current 7 CPU/8 GiB task host. A future disjoint-node allocation may run concurrently only after proving exclusive namespace and address ownership; do not add a scheduler now.
- Create fresh run/case Engine profiles in private tmpfs, separate from Tailscale state. In-memory secure-storage maps remain in the controller across intentional Engine restarts. Workload processes run under the inventory's protected UID, without NET_ADMIN, Docker socket, sensitive host mounts or tailnet-state mounts. Verified read-only build-artifact mounts and each workload's private profile tmpfs are permitted. Root's trusted helper controls only the inventoried namespace.
- Policy tables and helper processes get per-run identities. Installation records an ownership intent before mutation, then actual identity after success. Reconciliation handles a crash between these records by checking ownership and exact definitions. A pre-existing unknown table is a blocker, not a reason to flush it.
- Namespace faults never affect host firewall or shared services. No broad `pkill`, Docker prune, `conntrack -F` outside the owned namespace, or delete-by-prefix cleanup. A workload supervisor uses verified process-group/cgroup membership and waits for actual exit. Raw `docker exec` client termination alone is not proof Engine stopped.

Readiness states: `ABSENT`, `AUTH_PENDING`, `READY`, `LEASED`, `STALE_OWNED`, `BLOCKED`. Missing tools/TUN, wrong namespace, image mismatch or tailnet policy denial are environment failures. Authentication pending occurs before product timing starts. A product timeout after a valid start stays failed even if readiness later returns. Fixed host command error codes stay distinct from auth state.

On SIGINT/SIGTERM, stop starting cases, record interruption, release native holds under existing cleanup rules, await Engine and helper exit, drain bounded readers, scan, remove owned per-run policies/profiles, and finalize failure. A small trusted supervisor on the environment owner handles controller loss/lease expiry. Its watchdog does not reset product deadlines. For SIGKILL/host crash, `resume` first inventories resources and reconciles the stale run; it cannot continue an in-flight case as if uninterrupted. Loss of the controller's in-memory secure storage means that scenario must restart with a fresh profile and new attempt. A deliberately tested Engine restart is different: the live controller keeps the scenario storage map and records both process generations.

Per-run cleanup leaves live authorized nodes available only while the task owner retains the environment lease/expiry responsibility. End-of-task cleanup stops owned workloads, logs out each test client, deletes owned local resources and separately reconciles administrative node records. `tailscale down` is not logout; ordinary logout is not proof an ordinary admin device record was removed. [Official CLI behavior](https://tailscale.com/docs/reference/tailscale-cli). An expired ownership record triggers inventory/escalation, never blind deletion or indefinite retention.

Durable auth state is not required for the initial implementation. If later approved, an operator must define an OS secure-storage or secret-manager backend, encryption key custody, daemon materialization into private tmpfs, access controls, revocation/expiry, backup exclusion and cleanup verification. Authorization must cover that credential lifecycle. Encryption with a key beside the state file and mode-0600 plaintext are not sufficient. No such backend or authorization is assumed here.

## Real Tailscale evidence and execution policy

Use actual kernel-TUN nodes. Official container documentation says userspace mode is the default and kernel mode needs TUN/capabilities; persistent daemon state preserves identity across restarts. These are reasons to verify actual daemon mode and to avoid an unapproved disk state volume, not instructions to change current nodes. [Docker parameters](https://tailscale.com/docs/features/containers/docker/docker-params).

For every claimed address family, require before Engine execution:

1. Exact tail destination route and local source selection in each owned namespace.
2. Actual UDP echo under the same workload UID and routing conditions as Engine. Cover each required directed route or explicitly freeze the tested control edge set.
3. Positive tail path and negative physical/underlay bypass controls, including IPv6 where available. Verify drop counters and absence of successful bypass. Absence of an IPv6 underlay route must be reported; do not fabricate a successful negative control.
4. Input listener confinement and output enforcement for both families, loopback allowance, exact peer sets, distinct tailscaled UID/control path, and no unintended default routes, proxy mode, Engine relay or other address family escape.
5. Establish a post-preflight counter baseline. During actual hash-verified payload transfer, retain protected-UID predicate growth, family-specific evidence and listener/process identity. A daemon aggregate byte counter or preflight echo alone does not identify Engine traffic.

Frozen smoke replay retains its original IPv4 control scope. New IPv6 acceptance is a separate extension: force IPv6 candidate/route use, exclude IPv4 payload escape, verify actual IPv6 readback/settlement, then repeat controls after restoration. Dual-family nft rules alone never imply IPv6 acceptance.

Record two separate transport axes: Engine/Iroh direct versus Engine relay, and Tailscale direct versus DERP versus peer relay. Engine relay disabled can still carry Engine QUIC through Tailscale DERP. Official Tailscale documentation distinguishes all three overlay connection types and notes that connections can change type after initial establishment. [Connection types](https://tailscale.com/docs/reference/connection-types).

A direct/DERP/peer-relay claim requires a sustained, bounded payload window: bracket actual dispatch/readback with monotonic timestamps, sample transport state throughout, correlate peer-scoped traffic deltas with protected-UID payload evidence, retain transitions/gaps, and verify a before/after positive control. Status sampling alone supports only sampled classification; if exclusivity cannot be established with the pinned client's diagnostics and owned policy/counters, report `mixed` or `unknown`, not an exclusive path pass. Do not infer sustained DERP from the first ping. Any forced-underlay fault must be inside owned namespaces, preserve control-plane connectivity, and not be substituted for a native content-stream cancellation fault. Peer relay is not DERP. No relay infrastructure or ACL editing is required for the first slice.

Label physical topology explicitly: current three namespaces are one physical VPS. A Mac-to-VPS probe is not an Engine payload transfer. Cross-host/NAT/WAN claims require authorized independent hosts, route/NAT placement evidence and the same Engine readback/settlement controls. Cross-host native clock comparisons require bounded clock evidence; otherwise compare only local monotonic intervals. Retain the original NFLOG userspace timestamp envelope and clock-discontinuity guard for S03N; do not rename it kernel admission time.

ACL experiments are optional, separately authorized, with exact scope and rollback. If absent, record blocked/skipped coverage. Auth URLs travel only through a private transient operator channel. `prepare` checks actual pending state before `authorize`; `resume` reuses the same pending daemon. Container restart/state loss cannot be repaired with an old URL. Do not add authkeys to files, arguments, Docker logs, Git or public CI output.

### CI and developer integration

Current evidence: [justfile](../../justfile), lines 1–25, only wraps Cargo/MBX; [PR checks](../../.github/workflows/pr-check.yml), lines 339–343 and 670–713, already run isolated network smoke and direct/known-peer/relay/legacy jobs; [real-environment workflow](../../.github/workflows/engine-real-environment.yml), lines 1–45 and 50–115, has scheduled/manual modes, read-only contents permission, fixed repetitions and evidence upload. No authenticated Tailscale CI registration is established by those files.

Recommended integration after local proof:

- Quick developer loop: cached exact build plus local-smoke, no account dependency. Report build time separately from readiness/scenario/cleanup time. Unsupported local platforms are blocked, not silently redirected to a different semantic test.
- PR gate: secret-free disposable Linux runner/VM for local smoke and existing required network jobs. Add the production-policy subset after migration parity. Do not replace or weaken unrelated existing gates.
- Scheduled full: trusted default-branch execution of `lab-full`, one shared Cargo owner, serialized S04, original repetition counts and independent verifier. Upload sanitized artifacts on failure as well as success. Schedule absence is NOT_RUN, not a pass.
- Authenticated Tailscale: explicit operator opt-in against an exact reviewed source and trusted runner/build manifests. Initially manual on the existing owned environment. Later CI requires a separately approved account/runner/auth design; do not assume an environment name creates such a platform.

Untrusted PR code includes Rust build scripts and candidate binaries. A separate unprivileged process alone does not secure a shared privileged VPS. Do not execute public fork PR code automatically on authenticated nodes or a persistent self-hosted runner. Build untrusted candidates in disposable secret-free workers; no tailnet state, SSH credentials, Docker socket, write cache tokens or production network. Authentication environments run only explicitly approved exact source with a trusted helper that is not loaded from the candidate branch. Keep candidate workload privileges narrow and namespace requests constrained to the owned inventory. Do not use `pull_request_target` to check out and execute untrusted head code with secrets. Environment review gates do not by themselves isolate a self-hosted runner. [GitHub secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use).

No release/publish workflow changes belong to this work.

## Causal invariants that migration must preserve

Use the existing [fault-injection research](rust-fault-injection-research.md) as the hook-boundary design input. This runner migration does not require its Rust refactor. Keep actual native RESET, registered acquisition/first Pending poll, post-commit/pre-ACK hold, final receive gate and task-local attribution at their original owners. No new public Engine step/state API.

Retain original budgets: 120 s PR/Tailscale command watchdog; 30 s join convergence; 5 s identity lookup; 20 s online and membership gates; 20 s text and 60 s file readback; 10 s settlement; 15 s shutdown. Preserve case-specific exceptions, such as original 25 s/75 s recovery observations and regression transport readiness budgets, rather than replacing them with those smoke defaults. Keep 30 s pre-write hold expiry, capacity 64, stream journal 256-event bounds and existing overflow/epoch checks. The complete response case retains its actual 30 s response input, original logical deadline and absolute 60 s action limit. Authentication waiting is outside scenario clocks.

R05I proves registered acquisition plus an initial native Pending poll, not a backend that must still be pending at suspend. R05A retains the actual ACK-wait cancellation and duplicate confirmation. S03N retains its exact direction/window and capture accounting. Presence isolation preserves actual Duplicate; response/sibling cases preserve exact Delivered. Preserve complete source-chain classifications, not injected error strings. An outer watchdog triggers failed cleanup; it cannot create a passing timeout or a new retry budget.

## Vertical migration and acceptance plan — NOT RUN

All slices use E2E as the validation mechanism. No post-implementation unit tests are proposed. Before any isolated helper/control work, record its concrete failure modes; the verifier list above is the initial list. Keep the working runners until replacement coverage is demonstrated.

| Slice | Minimal change and ownership | Required repeatable artifact and exit condition |
| --- | --- | --- |
| 1. Frozen replay plus one LAN case | Add entry, HostSession, run owner, build-import manifest, minimal inventory/lease and two environment adapters. Move the existing smoke12 and L04 bodies with shared target-aware delivery/identity assertions. Keep native host unchanged. | Re-run exact frozen normal/fixture smoke12 on currently authorized nodes and existing L04 in isolated LAN. Compare every original assertion and budget with old-runner results. New run/build/artifact manifests must verify offline. Preserve topology distinctions and failures. Demonstrate cleanup without logging out nodes. No parity claim from static comparison alone. |
| 2. Next-source rebuild/replay | Implement current-checkout and exact-commit build selection, immutable role outputs and stale-source rejection. Keep same inventory and new profiles. | Build a second authorized revision through canonical target/cache; repeat both cases without editing hashes, container names or source allowlists. Verifier rejects prior-build reuse, changed fixture and wrong role. Controlled interruption and stale-lease reconciliation produce incomplete failure evidence and safe cleanup. No controller-crash continuation of secret-dependent profile state. |
| 3. Original laboratory matrix | Move remaining scenario families with a mechanical old-ID mapping. Preserve fault host, controls, baseline role and S04 child execution first. Bind complete7 exact names and existing structured evidence. Reuse uc-testkit only for missing outer Rust lifecycle artifacts. | Full single-attempt 120/69/12/+7, both feature graphs/fingerprints, all original deadlines/barriers, privacy/cleanup, 264 historical diagnostic checks reproduced where the original acceptance requires them. Export each new check result; do not simply copy the historical count. Run verifier adversarial artifact cases. Missing frozen archive/legacy identity blocks the relevant gate. |
| 4. Real Tailscale extensions | Add IPv6, learned cold reconnect, owned-daemon/network lifecycle, target isolation, and production-safe cancellation host dispatch only when its case is implemented. Transport classification/cross-host follow actual availability. | Each named extension has its own prerequisites, native positive/negative controls, payload/settlement and cleanup artifact. Remaining T00–T11 rows stay NOT_RUN/SKIPPED/BLOCKED. No all-topologies equivalence claim for complete7 or mDNS. |
| 5. Consolidate entry and CI | Migrate live callers/docs/CI to stable entry after their suite passes. Remove duplicated scenario/protocol code. | Exact old-to-new case list, equivalent selected sets and outcomes, actual downloadable sanitized CI artifacts, no lost required gate. Approved Tailscale infrastructure is optional and separately accepted. |

Compatibility/removal rule: historical artifacts and frozen source archives remain immutable. During slices 1–3 the old runners are the parity oracle. After a family's parity passes, its old executable entry becomes a thin argument-translation alias to the single implementation; it must not retain its own scenario body. In slice 5, update all in-repository callers and remove `run-pr343-e2e.sh`, `pr343-e2e.mjs`, and `tailscale-engine-smoke.py` executable aliases in the same integration change once no caller remains. Historical reproduction guides keep the frozen revision and old commands as historical references. Rename the invitation example to a descriptive name only with its caller/build-role migration; its frozen fixture hash stays historical. Move shared `connection-recovery-network.mjs` implementation only after all its non-PR callers are migrated; unrelated coverage is not deleted. No indefinite dual stack or indefinite deprecation timer.

Do not dismantle the old suite merely because smoke12 passed. If a slice fails, keep its failed artifact, repair the new implementation, and use a new attempt. An unresolved parity gap blocks deletion of that family's old runner.

## Open questions, impact, and execution status

Implementation prerequisites and material unknowns:

1. Root must make the frozen binary/source archive available and confirm role metadata, especially legacy host revision/adaptation. This research read committed summaries; it did not access that archive. Historical manifest incompleteness must remain explicit.
2. Confirm canonical target availability and actual build output discovery/cache behavior for source switching. Source labels and cached filenames cannot substitute for immutable role provenance.
3. Validate the pinned Tailscale 1.104.4 diagnostics needed for sustained classification. Current official docs may describe newer client behavior. Unsupported diagnostics require `unknown` claims, not a guessed parser.
4. Confirm local Linux runner availability for the default smoke and the exact resource inventory on the VPS before reuse. No live command was run here.
5. Production-safe cancellation dispatch is still missing from the normal process host. Existing stable cancellation operations can be adapted narrowly; private complete7 fixture seams must not become a stable Engine API.
6. The first four complete7 cases need consistent failure/cleanup evidence if the new full-suite artifact contract requires more than libtest termination. Add it around the existing real scenarios using existing testkit support; do not replace causal tests with simplified mocks.
7. A controller-private login URL channel and crash supervisor need concrete operator integration. No Lody/CI authorization platform is presumed. Root owns implementation coordination and publication.

Impact is limited initially to test orchestration, documentation and `justfile`; later slices touch host dispatch, private test evidence and existing CI callers. Production business APIs, persisted user formats, release workflows and runtime recovery ownership remain outside this design. The main tradeoff is retaining three explicit environment adapters and a few topology-specific cases rather than pretending every scenario is portable. This keeps meaningful native evidence while avoiding a general configuration platform. Keeping tmpfs credentials favors secure task reuse over unattended restart convenience.

Completed in this research: local HEAD/branch/status/submodule identity checks, targeted source and test/CI inspection, committed historical evidence inspection, current official Tailscale/Cargo/GitHub documentation research, and this English report. The PR web fetch failed; no remote-state inference replaced it.

Not executed: builds, Cargo, tests/E2E, remote commands, provisioning, authentication, firewall or account changes, code refactor, commits, pushes, PR comments, publication, or delegation. The only new file is this report. Repository build/test delivery gates are intentionally not run under the assignment's explicit limit. All migration acceptance above remains **NOT_RUN**.

External sources were retrieved on 2026-11-02. Recommendations are repository-specific inferences from those maintained primary sources and the inspected code, not claims that the proposed commands or infrastructure already exist.

## Coordinator review and first-slice clarifications

The coordinator checked L04 against the current runner: it is the plain LAN
changed-port/reconnect case with text and file readback. The proposed command entry
does not exist yet. The local source and authenticated PR readback both identified head
`b70edf1e6b25f400524b7e7191a394c3d2dcfcf8`, with base
`032d273656cb3e366f9c057a82f86f4bdaa1295d` on
`biolimbo/Engine:fix/lan-sync-wireguard` before this document was published.
This closes the Researcher's remote-identity limitation for that snapshot only.

The first vertical slice runs the trusted command on the Linux environment owner.
A macOS developer may invoke that entry through an already authorized SSH connection.
This is not an implemented remote-execution adapter, source-transfer mechanism or
cross-compilation path. Exact-commit work requires the exact source objects on that
build owner. Dirty development source requires an explicit verified source snapshot.
A label copied from the developer's different checkout is insufficient.

Before executing a role, reject a build whose OS, architecture or required runtime/libc
does not match the selected environment. A macOS executable cannot be treated as the
Linux candidate merely because its source revision matches. Keep this platform check
in the build/deployment manifest contract rather than adding a new transport framework.

The initial120/69/12/+7 and smoke12 counts are migration-parity targets. They are not
permanent constants for all future development. The reviewed scenario catalog defines
the expected cases, node edges, controls and repetitions. Freeze that catalog digest
and expanded expected set before each attempt. Adding a reviewed case changes the
expected set; the verifier still rejects omitted or silently skipped required cases.

The recommended first implementation is the frozen smoke12 plus L04 slice. It must
then build and execute a different source revision without editing candidate hash
constants. Only those passing artifacts justify migrating the remaining matrix and
removing duplicated runners. No implementation or runtime result is added by this review.

Publication checks passed under the coordinator: locked metadata, locked workspace/all-target
check, formatting, Rust style, repository preflight, relative document links and diff checks.
Only this report changed. No runtime suite was rerun and no proposed command was implemented.
