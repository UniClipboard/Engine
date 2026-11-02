# Real Tailscale three-node smoke result

Status: PASS for one three-node same-host smoke attempt. The full
[Tailscale execution plan](../active/2026-11-02-real-tailscale-e2e.md) remains open.

## Executed scope

The coordinator provisioned three independent kernel-TUN Tailscale clients through real
login URLs approved by the user. The clients used the same tailnet and distinct node
identities. All three clients ran on one7CPU/8GiB Linux VPS in separate Docker network
namespaces. Tailscale1.104.4 used the pinned official image recorded in the provenance.
The coordinator did not change tailnet ACLs, host firewall rules or existing services.
Node state stayed in private tmpfs; Docker logging was disabled.

Engine used the previously frozen normal production-policy executable, SHA256
`8bdf1f5f15a05278ddaf5bf814d879aafd7b2d52e3a626c473e146f098a6f09d`.
The original executed174-file source-manifest SHA256 is
`d77589ca363497bcbaa41a16b7b6e202ea547de2e4e0f5819d272b91aa04e540`.
Engine publication commit `cd774b4c5db78c0470d2cdbefd2e4bef1c1cb36f` retains those
runtime-source bytes; its later changes are documentation. Iroh remained
`1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`.

The new [controller](../../../scripts/testing/tailscale-engine-smoke.py) SHA256 is
`1acae46a54ca8ee37edafdbb2f455ef1a7a7b6956056ff7f576776c8c612c9d3`.
Its deployed bytes match the committed file. No Rust source, dependency or original
laboratory driver was changed for this smoke.

## Result

| Assertion | Observed result |
| --- | --- |
| Same-UID real UDP echo, A→B, B→C, C→A |3/3 |
| Exact tail IPv4 route/source checks |3/3 |
| Same-UID physical-address UDP drop controls |3/3 |
| Three Engine members online and mutually usable | Passed |
| Text, all six directed pairs |6/6 hash readbacks and requested-target settlements |
|512KiB file, all six directed pairs |6/6 hash readbacks and requested-target settlements |
| Engine relay fallback | Saved/applied Disabled on all three nodes |
| UID-constrained overlay packet growth after preflight | A1028, B891, C866 |
| Profile plaintext-canary scan |115 files, no matches |
| Engine shutdown, profile removal, owned policy removal | Passed |

The accepted attempt took90.888 seconds including setup, preflight, transfers and cleanup.
It contains exactly12 unique `(source,target,content-kind)` tuples. Every tuple has
actual receiver hash readback and exactly one positive settlement for its requested
target. Other members' delivery-view states remain recorded separately. A trusted
member without a delivery row appears as Pending in the existing view; that is not a
failed attempted delivery to the explicitly selected target.

Namespace policy permits Engine UID10001 UDP only through `tailscale0`, from its exact
tail address to the exact peer tail addresses, plus loopback. IPv4 and IPv6 rules are
installed. Listener rules reject physical-interface input to the Engine ports.
Non-overlay output attempts were dropped; A1654, B1180 and C989 were observed after
the preflight baseline. This is enforced confinement, not a zero-attempt claim.
Dedicated IPv6 application transfer was not isolated in this attempt.

The [accepted result](pr343-tailscale-smoke/smoke-08.json),
[source/deployment provenance](pr343-tailscale-smoke/smoke-08-provenance.json), and
[artifact manifest](pr343-tailscale-smoke/artifact-manifest.json) are committed.
Recompute artifact SHA256 and length, verify the controller hash, check all12 required
tuples, receiver matches, target settlements, route controls, cleanup and privacy fields.
Use the [reproduction instructions](../../../scripts/testing/tailscale-engine-smoke.md)
with new profiles and a new result path for each attempt.

## Historical attempts

Attempts01–07 remain in the same artifact directory and are not accepted as passes.
They used earlier controller revisions; the final controller hash does not identify
those earlier revisions.

- 01: legacy IPv6 filter backend unavailable; binary mount also lacked test-UID access.
- 02: nftables rule syntax rejected before Engine execution.
- 03: the negative UDP control hit a drop, but the controller rejected its native EPERM.
- 04: the controller sent null secure-storage state on first startup. Shutdown evidence
  failed for the exited startup process; that cleanup field remains false.
- 05: the controller treated transient setup code1103 as terminal instead of waiting
  under the existing pairing deadline.
- 06: the controller required every displayed member to settle for a single-target send.
  The final controller matches the requested target ID and preserves the other states.
- 07: B→C receiver readback timed out after three earlier text transfers succeeded.
  The controller had omitted the existing all-member usability gate. Attempt08 adds
  that gate and passes. The historical run did not capture B/C eligibility at failure,
  so the unique cause of that timeout is not established. One passing run does not
  prove absence of intermittency.

The coordinator separately rechecked that no Engine process or owned policy remained.
No failed attempt was relabeled passed or combined with attempt08 to form a stability claim.

## Remaining scope and resources

This is a real Tailscale integration result, not a replacement for the previous
synthetic WireGuard matrix. It is also not completion of T00–T11.
Restart/reconnect, learned-address cold start, concurrent cancellation, third-member
negative isolation, continuous direct/DERP classification, different-host/NAT/WAN Engine
transfer, native platforms and account ACL experiments remain NOT_RUN.
The local Mac-to-VPS Tailscale probe succeeded separately; it was not an Engine transfer.

All Engine processes stopped. Temporary profiles and policy tables were removed.
The three authenticated Tailscale nodes and sleeping workload containers remain owned
by this task for the next planned cases. This is not complete task-resource teardown.
The binary mount contains only verified executables and sanitized controller/results.
No auth URL, node key, Engine secure-storage value or raw invitation is committed.
Final task cleanup must log out the test daemons and separately reconcile administrative
test-node records, as specified by the execution plan.
