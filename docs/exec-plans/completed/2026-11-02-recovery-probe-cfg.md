# Recovery observation module selection: second slice

Status: PASS for this slice. This is the second bounded fault-injection refactor slice.

## Change and boundaries

The recovery owner now keeps observation state and event construction in private
selected implementations. `RetrySchedule` retains sole ownership of targets,
rounds, due times, entries, exhaustion and reconciliation. The direct feature-attribute
count across the five edited existing files changes from 32 to 8, plus the module
selector's cfg_attr. This count is not an acceptance target.

Eight source files form the slice: five existing files and three new private files.
The new files contain the disabled fenced-result observer and the enabled/disabled
schedule observations. Their logical module belongs to the recovery owner. No
getter, facade, public business type, port, generic probe framework or dependency
was added. Public feature-on control signatures and wire fields remain compatible.

Previous due is captured before source consumption and target mutation. Origin is
updated only when the actual due changes. Source capture, consumption, early returns
and final reset retain their previous order. Settlement observation precedes target
removal. The fenced-result event remains under the same current-attempt lock and
rejection branch. Persistence, token revocation and real notifications retain their
original order. The [source review](recovery-probe-cfg/root-review.md) records the
coordinator's independent inspection.

RoundStarted, DispatchStarted, RoundFinished and scope/direct-await expressions keep
explicit gates. They preserve argument erasure, future construction, polling,
task-local propagation and cancellation. The two other gates select the private
schedule implementation. Native RESET, ACK, acquisition and clipboard write boundaries
are unchanged. All 2/5/10-second retry delays, three-retry caps, 500ms actual-dispatch
limits, quiet windows and original runner assertions remain unchanged.

Disabled markers have no fields or observation state. Disabled methods are synchronous
and contain no clock, registry, allocation, clone, identifier retention, control or
fault behavior. Code generation, future layout and performance equivalence were not
measured. The first receive-observation slice and its evidence were not changed.

## Source and compiled identity

Frozen candidate parent: `7359c9044c105edd65dede3bd6ba00f7811d2bd6`.
The candidate was dirty because it contained this eight-file slice.
The decoded [source manifest](recovery-probe-cfg/source-manifest.json.gz) SHA256 is
`7f46829510af42f420bb08ecd049b3ba388de72ad6f1a224f765c4a903c12d30`.
Its 3,614 file/symlink identities matched before builds, after each build, after E2E
and in the publication source tree. Publication adds validation documents to that tree.
Iroh remains `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`.
No manifest, lockfile, Iroh, host or existing driver changed.
The driver SHA256 remains
`ff1673d04b854ace7ae5a38fa29215561fdda08529b8e3094fd59349ca692fba`.

| Linux executable | SHA256 |
| --- | --- |
| Normal | `123bb451adb0df2688ab34664f176be92576f0fe05cbeba9479cc3fb6c81055f` |
| Fault | `1c363c642be4b576a0e5cb8df6ea623b9f042d53068ee979917e997a5c678c4c` |

[Compiled provenance](recovery-probe-cfg/compiled-feature-provenance.json) matches each
frozen ELF hash to its Cargo deps executable and compilation fingerprint. Normal uses
`--no-default-features --features production-e2e`. Fault adds
`clipboard-stream-fault,network-test-probe`. Inverse graphs exclude Engine dev-tools
and Infra P2P test-util from both. Normal excludes both fault features.

## Acceptance

Each existing case ran three times, in separate real Engine processes with the
unchanged Linux runner. Normal L04 used LAN. Causal cases used the existing VPN topology.

| Case | Boundary | Result |
| --- | --- | --- |
| L04, normal | Changed ports; bidirectional text/file readback | 3/3 |
| R01 | Native RESET; timed retry cause and attributed commit | 3/3 |
| R02 | Native push/pull isolation; retry cap, absence and quiet window | 3/3 |
| R02A | Independent ActivePull commit attribution and readback | 3/3 |
| R03 | New offline/online window with its independent bounded budget | 3/3 |
| R04 | Old success fenced; current new-item budget preserved | 3/3 |
| R04F | Old recoverable result fenced; active new-item budget preserved | 3/3 |
| R04E | Old recoverable result fenced; exhausted new-item budget preserved | 3/3 |

Total: **24/24**. There were no failed E2E attempts. Every run passed cleanup and
plaintext-canary checks. All 48 network journals retain their original decoded
SHA256, byte count and row count.

The original [normal result](recovery-probe-cfg/normal-l04/result.json.gz) and
[causal result](recovery-probe-cfg/recovery-causal/result.json.gz) retain full journals,
readbacks, process windows, source/cause/origin references and cleanup/privacy fields.
Network journals, result JSON and the source manifest use deterministic gzip to keep
review noise small. Verification checks the decoded journal hashes, sizes and rows.

The [feature-off process check](recovery-probe-cfg/feature-off-process.json) confirms
absent fault capabilities and rejected stream_fault/address_probe/subnet_probe commands,
with successful shutdown and profile removal. No separate downstream compile-fail
consumer or codegen experiment was run. Existing Engine export gates are unchanged.

Root independently passed Application checks with the feature off/on, locked metadata,
workspace/all-target check, workspace format, Rust style, repository preflight and diff
checks. [Gate receipts](recovery-probe-cfg/gate-results.json) record commands and log
identities. Compiler/cache warnings remain in logs; required commands exited zero.
No new unit tests were added.

Engineer self-reported an accidental repository preflight despite its no-Cargo
assignment. That helper invokes Cargo metadata and six OpenMLS revocation tests.
No raw accidental-execution log was supplied. Those reported passes are excluded
from this candidate acceptance. Root independently reran its own required gates.

## Verify and reproduce

At this publication source tree, run:

```bash
python3 docs/exec-plans/completed/recovery-probe-cfg/verify.py
```

The verifier checks the full [artifact file set](recovery-probe-cfg/artifact-manifest.json),
source identities, compiled features, exact case/iteration coverage, decoded journals,
readbacks, cleanup and privacy. Large executable/build outputs remain outside Git.

Use an isolated Linux environment with the recorded Rust and Node versions, WireGuard,
network namespaces, nftables, conntrack and Python3. Mount this source at `/work/source`
with its canonical `target`, and create fresh `/work/evidence`. Copy `build-linux.sh`, `run-linux-e2e.sh`, `verify-source.py`,
`record-provenance.py` and `check-feature-off.py` from the artifact directory to `/work`.
Decompress source-manifest.json.gz to
`/work/candidate-source.json`. Run:

```bash
bash /work/build-linux.sh
bash /work/run-linux-e2e.sh
```

The build sources the repository cache entry and serially freezes normal and fault
hosts. The runtime recipe verifies compiled provenance and feature-off rejection, then
runs L04 three times and the R01,R02,R02A,R03,R04 prefixes three times. R04 selects all
three late-result cases. Record fresh executable hashes for every new build. Keep
failed attempts separately. Do not attach these historical results to a later build.
These fixed recipes are not the proposed general E2E framework.

Root uses one shared canonical Linux repository target, mounted at the default
`/work/source/target`; no CARGO_TARGET_DIR override is used. The dedicated container
and task directory are removed only after evidence copy, integrity verification and
idle-process/empty-namespace checks. Shared target/dependency/MBX caches, existing
Tailscale nodes and all other running containers are preserved.

## Limits

This slice does not complete the wider cfg cleanup. The remaining full PR343 matrix,
complete-Engine suite and Tailscale matrix are NOT_RUN for this source. Historical
passes remain associated with their earlier source. Native GUI/clipboard integration,
physical devices, platform keychains and performance/codegen measurement are NOT_RUN.
