# Receive observation module selection: first slice

Status: PASS for this slice. This does not complete the wider fault-injection refactor.

## Change

Two ordinary receive callers now use one private observation interface. The clipboard
module selects the existing enabled implementation or a synchronous disabled
implementation. Seven feature attributes were removed from the two callers.

The disabled implementation stores no state and returns no receive attempt. It adds
no allocation, lock, clock, task, fault control, dependency or public Engine API.
The enabled implementation and its `pre_write` module retain identical bytes.
Native RESET, task-local scopes, cancellation, ACK barriers, final receive gates,
retry budgets and error conversion remain at their existing positions.
Code generation, future layout and performance equivalence were not measured.

## Source and executable identity

The frozen candidate parent is `b5537ad1232549fce71c5039422ea9f952c3eebe`.
The candidate was dirty because it contained this four-file source slice.
Its [source manifest](receive-probe-cfg/source-manifest.json.gz) includes the new disabled
file and records 3,544 file or symlink identities. The manifest SHA256 is
`318f14a9f3ffb2cff52397d441ee7db384150233b33a1cb884e7c97fc420f3c7`.
All entries matched before building, after both builds, after E2E and in the local
publication tree. The publication adds validation documents to that source.

Iroh remains `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`.
No Iroh file, dependency manifest, lockfile or existing E2E driver changed.
The existing driver SHA256 remains
`ff1673d04b854ace7ae5a38fa29215561fdda08529b8e3094fd59349ca692fba`.

| Frozen Linux host | SHA256 |
| --- | --- |
| Normal | `ff3d56fb97c14a4402c6187f4fd80a4a17807439e767d56034c13681b5109938` |
| Fault controls | `eba878727cf5bbf032e64a5d8396154e2b76157c07d61df97ca1b22fba7864a2` |

The [compiled feature provenance](receive-probe-cfg/compiled-feature-provenance.json)
matches each frozen executable hash to its Cargo deps executable and compilation
fingerprint. The normal host uses `production-e2e`. The fault host adds only
`clipboard-stream-fault,network-test-probe` to that selection. Inverse graphs exclude
Engine `dev-tools` and Infra P2P `test-util` from both hosts. Normal compilation excludes
both fault features. The enabled feature list is not inferred from an unarmed host.

## Executed acceptance

All cases used separate real Engine processes and the unchanged Linux namespace
runner. The fault cases used its existing VPN topology. The normal control used LAN.
Original assertions, repetition counts, retry delays, timing limits and quiet windows
were retained. There were no failed E2E attempts in this run.

| Case | Verified boundary | Result |
| --- | --- | --- |
| L04, normal host | Changed ports; bidirectional text and file hash readback | 3/3 |
| R01 | Native push RESET, receiver absence, attributed timed retry commit | 3/3 |
| R02 | Native push/pull isolation, three retries, receiver absence and quiet window | 3/3 |
| R02A | Independent ActivePull commit attribution and hash readback | 3/3 |
| R05A | Actual push commit before ACK hold; cancellation/resume and duplicate settlement | 3/3 |
| R05G | Final receive gate, actual clipboard-port write calls and hold cleanup | 3/3 |

Total: **18/18**. Every run passed profile cleanup and the plaintext-canary scan.
All 36 network journals retain their original decoded SHA256, byte count and row
count. The journals, result JSON files and source manifest use deterministic gzip to reduce
repository size and review noise.
The [normal result](receive-probe-cfg/normal-l04/result.json.gz) and
[causal result](receive-probe-cfg/receive-causal/result.json.gz) retain the original
causal observations, readbacks, process windows, cleanup and privacy fields.

A separate [normal-host process check](receive-probe-cfg/feature-off-process.json)
confirmed that disabled capabilities were absent and `stream_fault`, `address_probe`
and `subnet_probe` commands were rejected. Startup, shutdown and profile removal
passed. Failure modes were listed before writing this process driver: accidental
feature unification, advertised disabled capabilities, accepted disabled commands,
and incomplete startup or cleanup. Public Rust export gates remained unchanged;
no separate downstream compile-fail consumer was built.

The local macOS checks passed with the shared target and cache:

- Application checks with the fault feature disabled and enabled.
- Locked Cargo metadata and workspace/all-target checks.
- Workspace formatting, Rust style and repository preflight.
- Diff whitespace validation.

The [gate receipt](receive-probe-cfg/gate-results.json) records the exact commands,
log identities and source comparisons. Compiler/cache warnings remain in the local
logs; all required commands exited successfully. No new unit tests were added.
The SSH transport closed during the first Linux build. Its original container
process continued. Both Cargo JSON streams ended with successful build records.
No completed build or test was restarted.

## Verify and reproduce

At this publication source tree, run:

```bash
python3 docs/exec-plans/completed/receive-probe-cfg/verify.py
```

The verifier checks the complete artifact file set, hashes, frozen source identities,
compiled feature selections, exact case/iteration coverage, all decoded journal
identities, readback results, cleanup and privacy. The
[artifact manifest](receive-probe-cfg/artifact-manifest.json) includes the verifier.
Raw build executables and large build logs are not committed.

To reproduce runtime acceptance, use an isolated Linux environment with WireGuard,
network namespaces, nftables, conntrack, Python3, Node18 and the Rust toolchain in the
recorded environment files. Use `/work/source` for this source tree and its canonical
`target`. Use `/work/evidence` for new output. Copy the five recorded helper recipes
from this artifact directory to `/work`, and decompress `source-manifest.json.gz` to
`/work/candidate-source.json`. Then run:

```bash
bash /work/build-linux.sh
bash /work/run-linux-e2e.sh
```

The build recipe sources the repository cache entry before Cargo, builds serially,
freezes both hosts and records the original feature selections. The runtime recipe
verifies compiled provenance, tests feature-off controls and calls the existing
runner with `L04 --repeat 3` and `R01,R02,R02A,R05A,R05G --repeat 3`.
Use a new output directory for each attempt. Retain failures. A fresh build must
record its own executable hashes; do not attach these historical results to it.
These recipes describe this fixed acceptance run, not the proposed general network
E2E framework.

The dedicated validation container and directory were removed after artifact copy,
hash verification and an idle-process/empty-namespace check. Shared compilation
caches, existing Tailscale nodes and other running containers were preserved.

## Limits

The remaining full PR343 matrix, complete-Engine suite and Tailscale matrix were
NOT_RUN for this candidate. Historical results belong to their earlier source.
Physical devices, product GUI/native clipboard integration and platform keychains
were NOT_RUN. Linux host clipboard-port observations do not prove those layers.
Other fault-injection feature gates remain for later slices.
