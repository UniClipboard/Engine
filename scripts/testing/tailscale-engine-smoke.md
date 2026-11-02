# Real Tailscale three-node Engine smoke

This is a small first execution of the [Tailscale plan](../../docs/exec-plans/active/2026-11-02-real-tailscale-e2e.md).
It uses three real Tailscale nodes on one physical Linux Docker host.
It does not execute the complete staged plan or prove different-host/NAT/WAN behavior.

## Preconditions

Use an isolated task environment. Keep existing services and the host firewall unchanged.
The controller requires root, Python3, Docker, `nsenter`, nftables, iproute2 and `setpriv`.
It creates one owned inet nftables table inside each task network namespace.
Do not run two copies against the same namespaces.

The already-authorized kernel-TUN clients must be named `uc-pr343-ts-a`,
`uc-pr343-ts-b` and `uc-pr343-ts-c`. Each must carry label
`uc.task=pr343-real-tailscale`, have distinct state, and belong to the same tailnet.
Keep Tailscale state in private tmpfs and Docker logging disabled.
Do not export status JSON, login URLs or node keys.

Prepare a directory containing the frozen `integrated-final-normal` and
`invitation-fixture` executables. Their required hashes are constants in the controller.
The archived normal host uses the production policy without fault/probe features.
The controller verifies both local files and the executables mounted in every workload.
Host revision labels alone are not accepted as source identity.

Create separate libc-compatible workload containers. For the recorded laboratory,
the existing `rust:1.95.0-bookworm` image supplies the execution environment:

```bash
LAB_BIN=/absolute/path/to/verified/binaries
for alias in a b c; do
  docker run -d --name "uc-pr343-ts-engine-$alias" \
    --label uc.task=pr343-real-tailscale \
    --network "container:uc-pr343-ts-$alias" \
    --memory=512m --cpus=1 --log-driver=none \
    --mount "type=bind,source=$LAB_BIN,target=/opt/uc,readonly" \
    --tmpfs /profiles:rw,nosuid,nodev,mode=0700,uid=10001,gid=10001 \
    rust:1.95.0-bookworm sleep infinity
done
```

Make the binary mount directory traversable and the binaries executable by UID10001.
It must contain no private state. Keep profiles in their separate private tmpfs mounts.
No network service exposes the Engine control protocol; Docker exec carries inherited pipes.

## Execute

Run on the Linux Docker host:

```bash
python3 scripts/testing/tailscale-engine-smoke.py \
  --binary "$LAB_BIN/integrated-final-normal" \
  --fixture "$LAB_BIN/invitation-fixture" \
  --evidence /absolute/path/to/new-attempt.json
```

Use a new evidence path for every attempt. Never overwrite a failed attempt.
The controller exports fixed failure categories and sanitized result fields.
It does not export private requests, responses or runtime addresses.

Before Engine starts, require exact tail IPv4 routes, three same-UID UDP echo
controls and three dropped physical-address UDP controls. The namespace rules cover
both IP families. Engine UID10001 can send UDP only to the exact peer tail addresses
through `tailscale0`, with the exact local tail source. Other non-loopback output is
dropped. Expected listener ports reject physical-interface input.
Tailscaled has its own UID and retains its encrypted underlay/control connectivity.

The controller enables overlay address publication and disables Engine relay fallback.
It restarts each Engine with the same profile and in-memory secure-storage values.
The invitation fixture preserves identity and expiry while selecting the real tail address.
All three Engines join one space. Each of the six directed pairs sends fresh text
and a512KiB file. A pass requires all12 exact hash readbacks and the explicitly
selected target's `delivered` or `duplicate` settlement within the existing budgets.

The delivery view also lists trusted members that were not selected for this send.
A missing delivery row appears as `pending`; this is not evidence of an attempted send.
The controller matches `target_device_id` in memory and exports other target states
separately. It does not accept another member's result for the requested target.
This scenario makes no negative claim about third-member clipboard convergence.

After transfer, require actual UID-constrained overlay packet growth after the UDP
preflight baseline. Default source socket binding remains wildcard; routing and
namespace enforcement provide confinement. Dedicated IPv6 transfer, continuous
Tailscale direct/DERP classification, cancellation, restart recovery and cross-host
Engine transfer are separate cases, not inferred from this smoke.

## Cleanup and interpretation

The controller shuts down Engine processes, scans profiles for text/invitation canaries,
removes the profiles, and deletes only its own namespace policy tables.
File canaries are synthetic; full payload hashes and sizes remain in the result.
Cleanup or privacy failure makes the attempt fail. Exceptional cleanup failure requires
an explicit owned-process/resource inventory before retrying.

The controller leaves the authenticated Tailscale nodes and sleeping workload containers
available for the next planned case. Their lifecycle remains the coordinator's task.
Remove only inventoried workloads after collecting evidence. Log out and reconcile
administrative test nodes when the whole Tailscale test task ends.
Never prune unrelated containers, networks, images or shared caches.
