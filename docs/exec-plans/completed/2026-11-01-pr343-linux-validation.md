# PR343 Linux validation

Status: PASS for the recorded Linux laboratory scope.

## Changes and ownership

Application owns delivery recovery, terminal writes, receive gates and Blob retry deadlines.
Infra supplies native streams, discovery and the final LAN transport policy.
Engine assembles these capabilities and supplies complete two-Engine acceptance scenarios.
Callers keep the existing capture, receive, retry and suspend operations.
Successful delivery requires receiver hash readback and the recorded sender settlement.
Recovery keeps the existing retry limits and original deadlines.

This follow-up repairs network-change discovery, LAN transport policy, delivery-result fencing,
receive-disable races, response timeout recovery and independent Blob/presence lifetimes.
Private feature-gated fault controls exercise real streams and native acquisition boundaries.
They do not add stable Engine workflow queries.

## Accepted source

The executed source was frozen from Engine baseline
`032d273656cb3e366f9c057a82f86f4bdaa1295d`, the head of upstream PR343.
Its reconstructed root tree is `86abdd6f435c246621df1953202a12c9c02ab41f`.
The 174-file source manifest SHA256 is
`d77589ca363497bcbaa41a16b7b6e202ea547de2e4e0f5819d272b91aa04e540`.
Iroh is pinned to `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`.
The runner SHA256 is `ff1673d04b854ace7ae5a38fa29215561fdda08529b8e3094fd59349ca692fba`.

PR preparation updates documentation status and adds this public summary and matrix.
These documentation changes do not change the executed Rust, dependency or runner bytes.
The committed source manifest retains the exact pre-publication source identities.

## Results

| Gate | Result |
| --- | --- |
| Main matrix, 40 mandatory cases with three fresh repetitions | 120/120 |
| Nested direct, known-peer, relay and both legacy directions | 69/69 |
| Normal-feature and same-binary observer-disabled controls | 12/12 |
| Complete two-Engine causal scenarios | 7/7 |
| Diagnostic JSONL integrity checks | 264/264 |
| Required repository checks and independent reviews | Passed |
| Task environment cleanup | Passed |

The [fixed matrix](pr343-linux-validation/mandatory-matrix.json) and
[acceptance ledger](pr343-linux-validation/acceptance-ledger.json) retain the required cases,
individual outcomes, result hashes and binary identities.
The complete Engine scenarios verify distinct-content cancellation, shared-content cancellation,
partial-resource readability, transient read recovery, response timeout recovery,
presence-failure isolation, and timeout/sibling/healthy-reuse behavior.
All seven ran in one process: zero failures, zero ignored, 164.78 seconds.

Presence isolation records actual sender `Duplicate`, which is a positive settlement under the
existing Core contract. It does not relabel that outcome as `Delivered`.
The response and sibling A/C flows retain exact `Delivered` assertions.
The internal local-owner release edge was not directly observed.
R05I proves a registered acquisition with an initial native `Pending` poll, not a backend that
necessarily remains pending at suspension. R05A records cancellation of the ACK wait.
The dynamic subnet capture conclusion covers the checked A-to-B global UDP windows.
It is not a claim of zero traffic in both directions or throughout the entire mutation window.

## Build identity and checks

| Executable | SHA256 |
| --- | --- |
| Narrow production-policy host | `c99c036650c479a832ce0d98a2a6689e3cf27aa5c2f89192d5b431bf7892522b` |
| Normal host | `8bdf1f5f15a05278ddaf5bf814d879aafd7b2d52e3a626c473e146f098a6f09d` |
| Transport regression host | `8e8a6505619e630c5e63975bb73163d913c7b70a905e64d82a4443bb24bc3956` |
| Relay | `38be88534852b2a64f24cc31f17537c996dc3a671f34f799c8cd47e9074e1c74` |
| Complete Engine executable | `768e1b0d973d547c589fd4d1fa88f7731d109629156e3bfe25723e0b838c1f1e` |

The environment used x86_64 Linux and Rust1.95.0 in a disposable bridge-networked container,
with six CPU and6GiB limits. Cargo used one shared target with the compiler cache preserved.
Narrow and normal compiled fingerprints and inverse feature graphs exclude Infra test-util
and Engine dev-tools. Forward dependency trees alone do not prove root feature selection.

Passed: locked metadata; locked workspace/all-target check; workspace formatting;
Rust style; repository preflight; diff check. The preflight includes privacy rules for1395
observability callsites and the existing OpenMLS verification fixtures.
Independent source and runtime evidence reviews found no remaining blocking findings.

## Reproduction and evidence

Follow the [network laboratory guide](../../../scripts/testing/pr343-e2e.md).
Run the fixed main matrix three times and both companion controls three times.
Keep S04 repetitions serial because their child suites share a fixed bridge address and relay port.
Use fresh profiles, retain failed attempts, and reconcile every required case against the matrix.
Run the seven cases under the `testing::host_adapter_contract::receive_retry::stream_isolation`
selector using the [Blob causal design](../../design-docs/blob-response-causal-e2e.md):

```bash
just cargo test -p uc-engine --locked --features dev-tools --lib \
  testing::host_adapter_contract::receive_retry::stream_isolation -- --test-threads=1
```

The structured Blob evidence files are selected with `UC_PRESENCE_BLOB_E2E_EVIDENCE`,
`UC_BLOB_RESPONSE_E2E_EVIDENCE` and `UC_BLOB_SIBLING_E2E_EVIDENCE` as defined by the test source.

The sanitized raw results, checksums, build records, source patch, Iroh bundle and eight frozen
ELF files remain in the coordinator's local validation archive. The binary archive SHA256 is
`5d30def5f1003e0c9c82bd597413fbe8c527276d841fc267a451ced0821e6c75`.
Large binaries, raw profile data and credentials are not included in Git.
Historical failed runs remain preserved and are not counted as final passes.

The task container, bind directory and temporary archives were removed after artifact verification.
Unrelated services and shared caches were preserved.
Native clipboard, keychain, GUI, real OS sleep/wake, mobile devices and actual Tailscale are NOT_RUN.
This report does not establish those platform acceptance results.
