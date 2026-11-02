# Real Tailscale Engine E2E execution plan

## Status, scope, and ownership

- Task: `pr343-real-tailscale-design`.
- Workflow: Engine PR343 follow-up; Engine `biolimbo/Engine#1`.
- Status: design ready; the full proposed matrix remains **NOT_RUN**. A separately scoped [coordinator smoke result](../completed/2026-11-02-tailscale-smoke.md) records the later same-host execution.
- Frozen Engine source: `cd774b4c5db78c0470d2cdbefd2e4bef1c1cb36f`.
- Frozen Iroh source: `UniClipboard/iroh#2`, `1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5`.
- Researcher inspected local source and official documentation. Researcher may write only this plan. No remote command, installation, authentication, credential read, build, test, commit, push, PR creation, or delegation is part of this research execution.
- Root owns provisioning, actual login URL delivery, remote mutations, credentials, execution, evidence export, and cleanup. The user owns account/tailnet authorization and any ACL approval.
- The future E2E runner owns each complete scenario: setup, stimulus, assertions, failure classification, evidence, and teardown. The caller requests one case. It returns a terminal result and a verifiable artifact. Engine owns application recovery and retry. The runner must not resend silently to turn a failure into success.
- The user explicitly requested this document in English. This overrides the repository document-language default for this file.

The initial environment is three isolated kernel-networked Tailscale clients on one Docker VPS, reached by Root through SSH alias `primaryserver` (7CPU/8GiB). This is **same-host tailnet integration**. Independent Tailscale identities and network namespaces do not make it a different-host, different-NAT, or WAN test. Those claims require later execution on independently located hosts.

## Verified source findings

All source references below refer to the frozen Engine revision. Local HEAD and the Iroh gitlink and checkout matched the revisions above. The initial working tree had an untracked `target` entry; it was not modified or inspected for runtime evidence. No existing acceptance result was rerun.

| Finding | Primary evidence | Confidence and limitation |
| --- | --- | --- |
| The current driver has no real Tailscale provisioning/backend option. Its VPN topology creates kernel `wg0`. | [driver](../../../scripts/testing/pr343-e2e.mjs), `Run.prepare`, `Run.wireguard`, lines 319–381 | HIGH, static source; current commands cannot run the proposed Tailscale matrix unchanged. |
| Real receiver bytes and sender settlement are already mandatory. | Same driver, `Run.transfer`, lines 500–562; [runner contract](../../../scripts/testing/pr343-e2e.md) | HIGH; preserve these assertions. |
| Persisted address coverage already separates learned hints from configured hints and blocks reverse new dials. | Driver, `learnedAddressReconnect`, lines 691–727 | HIGH; adapt interface/address selection, not the recovery algorithm. |
| Production port configuration still binds wildcard IPv4 and IPv6 sockets. There is no exact-IP setting in this path. | [node builder](../../../crates/uc-infra-p2p/src/network/iroh/node.rs), lines 963–984 | HIGH; do not invent `bind_ip`, `bind_addr`, or `--bind-tail-ip` support. |
| Overlay publication is opt-in. LAN-only dial filtering allows CGNAT/ULA but still filters disallowed global candidates. | [address policy](../../../crates/uc-infra-p2p/src/network/iroh/addr_filter.rs), `is_virtual_nic_ip`, `is_local_network_ip`, `restrict_to_local_network`; [settings](../../../crates/uc-core/src/settings/model.rs), `allow_overlay_network_addrs` | HIGH; publication filtering is not an outbound deny rule for already supplied overlay hints. |
| A four-text concurrent command exists, but it is gated by `current-engine`, which enables `uc-engine/dev-tools`. | [host](../../../tests/hosts/connectivity/src/main.rs), lines 415–416; [concurrent sender](../../../tests/hosts/connectivity/src/concurrent_send.rs); [feature graph](../../../tests/hosts/connectivity/Cargo.toml) | HIGH; do not enable development policy to obtain this command in production acceptance. |
| Stable cancellation operations exist, but this host does not dispatch them. | [operations](../../../crates/uc-engine/src/contract/operation.rs), `CancelEntryReceive`, `CancelInboundTransfer`, lines 947–963; host command dispatch | HIGH; cancellation needs a small host adapter before execution. |

Follow the existing [security contract](../../SECURITY.md), [engineering principles](../../design-docs/engineering-principles.md), and [business observation standard](../../design-docs/observability.md#业务记录组织标准). No new production facade, internal-stage query, business orchestration, or persistent data format is required for this plan.

## Failure modes to define before isolated controls

1. Userspace networking or a missing TUN device produces a proxy test instead of kernel routing.
2. Shared state creates cloned node identities. Recreating state creates duplicate administrative nodes.
3. Pending login, device approval, Tailnet Lock, or existing policy blocks reachability before Engine runs.
4. A shared Docker bridge, IPv6 route, mDNS, cached connection, or Iroh relay lets Engine bypass the intended overlay.
5. A configured address silently repairs a test intended to prove persisted learned addresses.
6. Tailscale discovery packets or aggregate counters are mistaken for Engine payload transport.
7. An initial DERP ping is mistaken for a sustained DERP transfer. A peer relay is mislabeled DERP.
8. Default overlay publication filtering removes the needed candidate. Broadening LAN-only policy hides a real regression.
9. A timed-out or completed file is mistaken for an in-flight cancellation. A sibling is canceled by a shared network fault.
10. A reused canary or stale history entry falsely proves delivery. Send acceptance is mistaken for receiver readback or sender settlement.
11. Restart loses the in-memory secure-storage adapter. Re-authentication creates a different node or invalidates cached routes.
12. Capture loss, counter overflow, clock mismatch, process loss, or raw diagnostic export invalidates evidence.
13. Host firewall changes interrupt SSH or unrelated workloads. Partial cleanup leaves authenticated nodes or secret state.

Each fault-control case must have a positive control before and after the fault. An untriggered fault is inconclusive, never passed. No unit tests are proposed.

## Minimum infrastructure and resumable authorization

Root's deployment boundary is confirmed by the user (not independently inspected by Researcher): containers `uc-pr343-ts-a`, `uc-pr343-ts-b`, and `uc-pr343-ts-c` on the primaryserver VPS use `tailscale/tailscale@sha256:ed6480b33c6eb67c9cbd203ed42b9fb57e361986725b6856def71b7a3d9e276c`, actual version `1.104.4`. Each has its own Docker network namespace and kernel TUN, container-scoped `NET_ADMIN`/`NET_RAW`, a 256 MiB memory limit, and a 1 CPU limit. Docker logging is disabled. Both `/var/lib/tailscale` and `/run/tailscale` are mode-0700 tmpfs. Node keys must not be written to ordinary host disk.

Use these existing task containers; do not provision duplicate nodes from the illustrative template below. Tailscale SSH, subnet routing, exit nodes, DNS acceptance, and route acceptance are disabled. Do not modify existing services. All faults must remain inside owned container namespaces; host-level faults and host firewall changes are prohibited. No fourth service or dedicated DERP is needed. Root provides actual login URLs directly to the user as soon as available, without waiting for this plan or recording URLs in reports or Git.

For a straightforward manual-login lifecycle, run the pinned image's `tailscaled` directly. This avoids an automatic-auth entrypoint. The following are **Root command templates, not executed commands**. Root validates binary paths and flags against the pinned image before use. Variables are non-secret inventory values.

```bash
docker run -d --name "$TS_A" --hostname "$NODE_A" \
  --network "$NETWORK_A" --log-driver=none \
  --cap-add=NET_ADMIN --cap-add=NET_RAW --device=/dev/net/tun \
  --memory=256m --cpus=1 \
  --tmpfs /var/lib/tailscale:rw,nosuid,nodev,mode=0700 \
  --tmpfs /run/tailscale:rw,nosuid,nodev,mode=0700 \
  --entrypoint tailscaled "$TS_IMAGE_DIGEST" \
  --state=/var/lib/tailscale/tailscaled.state \
  --socket=/run/tailscale/tailscaled.sock --tun=tailscale0

docker exec "$TS_A" tailscale up --hostname="$NODE_A" \
  --accept-dns=false --accept-routes=false --ssh=false --timeout=30s
```

Repeat independently for B and C. Use a fresh owned Docker network per client if practical; Docker-managed networking is allowed, but do not hand-edit host firewall rules. The authoritative anti-bypass rules live only in client namespaces. If using the official container entrypoint instead, explicitly set `TS_USERSPACE=false`, `TS_AUTH_ONCE=true`, and `TS_STATE_DIR`; do not combine both startup strategies. Official Docker parameters describe the kernel and restart behavior [1].

Do not supply `TS_AUTHKEY`, OAuth credentials, or authkeys. Do not put authorization URLs in this plan, a repository file, Docker logs, shell history, or an evidence artifact. Root captures the actual CLI login URL through a private transient channel and sends it promptly to the user, labeled A/B/C. The user opens the real URL and chooses the account/tailnet. A generic admin-console URL is not a login authorization URL. Researcher cannot produce those URLs without provisioning and authentication actions outside this task.

Keep the same live containers while authorization is pending. CLI timeout is `AUTH_PENDING`, not a product failure. Resume by querying the same daemon, rather than recreating the node. Query `tailscale status --json` in memory; retain only a fixed state and node-alias match. Check for Running, device approval, and Tailnet Lock requirements. Do not automatically approve devices or change policy. If a URL expires, Root requests another from the same daemon and sends that actual URL. Start scenario deadlines only after authorization and network preflight complete.

Daemon state contains private keys even without authkeys. Keep it in tmpfs and do not inspect or export it. Container restart, deletion, or recreation loses this deployment's tmpfs state and requires fresh user authorization. Never call container restart a persistent-identity cold start. Engine secure-storage values and invitations travel only through existing private pipes and controller memory.

Treat the two restart classes as separate experiments:

1. **Same-identity daemon restart:** keep the container and its tmpfs alive. Record the node identity comparison in private memory. Restart only the tailscaled process using a supervisor or a persistent container anchor that Root has verified. Keep the same state path and socket path. Verify the same node identity and tail addresses, repeat route proof, and require Engine recovery without login or re-pairing. If tailscaled is PID 1, stopping it ends the container; this deployment cannot perform this case by simply killing PID 1. Mark the case blocked until Root provides an owned process lifecycle that keeps the container alive. Do not substitute `docker restart`.
2. **New-node authorization after state loss:** deliberately restart/recreate only an inventoried test container in a separately labeled case. Preserve no Tailscale key backup. Root immediately sends the new actual login URL to the user. After authorization, compare the new node identity and actual addresses in private memory, rebuild exact-address route controls, and record whether Engine recovers with its own unchanged profile. Address changes can invalidate configured hints; preserve that failure, then use a separately labeled configured-hint control if needed. Reconcile the old administrative node as well as the new node during cleanup. This proves recovery after fresh authorization, not Tailscale identity persistence.

A CLI `--force-reauth` against a surviving daemon is a third, optional authentication experiment. It does not substitute for either restart class. Initial authorization and renewed authorization remain outside product deadlines; resume the original recovery deadline only after readiness and route preflight.

## Source and binary freeze

Before execution, Root records Engine commit, clean/dirty source state, Iroh gitlink and actual checkout, other submodule identities, lockfile hash, toolchain, target triple, host feature graph, driver and fixture hashes, binary hashes, image digest, and `tailscale version`. Source labels compiled into a host are not sufficient provenance.

Use the existing production acceptance graph: `--no-default-features --features production-e2e,clipboard-stream-fault,network-test-probe`. Keep `uc-infra-p2p/test-util` and `uc-engine/dev-tools` absent. A separate `production-e2e` build is the feature-off control. Build serially through the repository cache and shared target under Root's separate execution scope. Do not run the build wrapper as a provisioning shortcut.

Read back SHA-256 of the deployed executable inside each execution environment. Tie each process start to that hash and its source manifest. If host adapters are needed, retain the original frozen source identity plus the exact adapter diff hash and resulting source-tree/binary hashes. A modified host must not be described as byte-identical to the frozen revision. Freeze before the first case; any repair starts a new immutable evidence attempt.

## Route proof and exact address boundary

Start with IPv4 and repeat the overlay smoke over IPv6 only after the first pass. Obtain each actual tail address in memory using `tailscale ip -4` or `tailscale ip -6`. Bind **configured peer hints and route assertions** to that exact address, never a guessed `100.x` address or a broad CIDR.

The existing Engine cannot literally bind its socket to the exact local tail IP. Record `socket_binding=wildcard`, verify it with `ss`, and mark literal exact-IP binding **UNSUPPORTED_AT_FROZEN_REVISION**. Do not claim otherwise from a successful route query. The initial smoke can prove exact source-address traffic confinement without changing production code. If literal binding remains an acceptance requirement, keep that requirement blocked for a separately authorized implementation and source freeze.

Run Engine as a dedicated non-root numeric UID inside each Tailscale network namespace. Tailscaled uses a different UID. Root can enter the namespace with a small pinned execution container that has the required libc and the frozen binary; no package installation is needed in the Tailscale image. Share the network namespace only. Do not share A/B/C profiles or secure storage. Use stdin/stdout pipes for the host protocol, not a listening management port.

Before Engine starts, install an owned namespace-only nftables table for both address families:

- Engine UID output: allow exact peer tail addresses through `tailscale0`; reject all other IP output. Count allowed overlay packets and rejected bypass attempts separately. Restrict source to the actual local tail IP for each family.
- Engine listener input: allow only the expected peer tail addresses arriving on `tailscale0`; reject physical-interface traffic to the actual Engine listener ports, including changed ports.
- Leave tailscaled control, DNS, encrypted tunnel traffic, and Docker exec management unaffected. Include IPv6 and multicast in the deny coverage. Do not block physical traffic globally: the overlay needs its underlay.
- Validate the UID rule using same-UID positive and negative probes before starting Engine. Validate that physical-address delivery to the Engine port fails. A failed ICMP probe alone is not proof of UDP isolation.

Root first identifies client container PID and verifies its namespace differs from the VPS host namespace. Templates for the disposable namespace are:

```bash
nsenter -t "$CLIENT_PID" -n ip route get "$PEER_TAIL4" from "$LOCAL_TAIL4"
nsenter -t "$CLIENT_PID" -n ip -6 route get "$PEER_TAIL6" from "$LOCAL_TAIL6"
nsenter -t "$CLIENT_PID" -n ss -Hlunp
nsenter -t "$CLIENT_PID" -n nft -j list table inet uc343_tailproof
docker exec "$TS_A" tailscale status --json
docker exec "$TS_A" tailscale ping --c=3 "$PEER_TAIL4"
```

The nftables table must be rendered from Root's validated in-memory address/UID inventory. These commands are templates, not evidence. Do not copy a broad host `nft flush ruleset` or `conntrack -F` into this design. Any conntrack removal for direction proof must target only the disposable namespace and the relevant Engine tuple.

Require route output interface, exact source-address match, positive same-UID tail reachability, failed physical UDP control, Engine relay-disabled saved/applied readback, and per-window overlay packet growth. Stream packet metadata through an in-memory parser. Export only aliases, timestamp/sequence, protocol class, lengths, and counts. Never retain pcap, raw packet text, addresses, keys, or payload. Counter-only proof establishes enforced confinement; a zero-attempt assertion additionally requires zero bypass-attempt counters, not merely zero delivered bypass packets. If unrelated tailscaled traffic can contaminate a predicate, repair the predicate before acceptance.

## Two independent transport classifications

Report these dimensions separately for every payload window:

| Dimension | Values | Evidence |
| --- | --- | --- |
| Engine/Iroh transport | direct socket, Engine relay, unknown | Existing sanitized Iroh diagnostics plus saved/applied relay policy and namespace enforcement |
| Tailscale transport | direct, DERP, peer-relay, mixed, unknown | Peer-specific status/ping observations before/during/after traffic, correlated with actual transfer window |
| Test topology | same-host tailnet, different-host same-LAN, different-NAT/WAN | Frozen infrastructure inventory |
| Application route label | `overlayP2P`, `privateLAN`, `wireguard`, unknown | Proven interface, address scope, and policy, not UI text alone |

`Engine relay off + Tailscale DERP` is still an `overlayP2P` candidate: Iroh uses a direct tail-address socket while Tailscale relays the underlying encrypted tunnel. It is not Engine relay fallback. Tailscale can begin through DERP and later become direct. It also supports peer relays; do not label these DERP. These distinctions follow the official connection-type documentation [2].

A DERP-specific case must sustain DERP throughout the payload window. Root may block outer direct UDP only inside the disposable Tailscale namespace, preserving DNS and control/DERP HTTPS. Block relevant IPv4 and IPv6 outer paths; keep inner `tailscale0` UDP intact. Validate the rule with a positive control, then require observed DERP and payload readback. If a peer relay or mixed path remains, record that class and keep the DERP-only case unproved. No custom DERP deployment or account-wide policy change is needed.

## Existing host interfaces and required small adapters

Reuse [the JSON-lines host](../../../tests/hosts/connectivity/src/main.rs). Its initial request is `{ "root": "<private-profile>" }`; retain returned `secure_storage` only in memory and pass it back on restart. Every ordinary request uses `command`. Correlate responses in order. Do not pipeline multiple requests on one host as a substitute for a concurrent Engine operation.

Example request shapes, with all placeholders resolved in private memory:

```json
{"command":"settings_update","patch":{"network":{"allow_relay_fallback":false,"allow_overlay_network_addrs":true,"bind_port":23110}}}
{"command":"create","name":"A"}
{"command":"invite"}
{"command":"join","invitation":"<private invitation>","name":"B"}
{"command":"settings_update","patch":{"network":{"peer_addresses":{"<peer-id>":{"ip":"<exact-tail-ip>","port":23111}}}}}
{"command":"send","peer":"<peer-id>","text":"<fresh synthetic canary>"}
{"command":"send_file","peer":"<peer-id>","handle":"<synthetic-handle>","display_name":"synthetic.bin","bytes":[1,2,3]}
{"command":"history"}
{"command":"entry","entry":"<entry-id>"}
{"command":"read_file","entry":"<entry-id>"}
{"command":"delivery","entry":"<sender-entry-id>"}
{"command":"receive_progress","entry":"<entry-id>"}
{"command":"outbound_progress","entry":"<entry-id>","peer":"<peer-id>"}
{"command":"flush"}
{"command":"shutdown"}
```

Use `setup`, `peers`, `eligibility`, `settings`, `relay_overview`, and `events` as the existing driver does. Use fresh random text and a 512 KiB synthetic file for the smoke, not the three-byte illustrative JSON payload above. `peer_addresses` updates must preserve unrelated intended entries. Restart after the network configuration and verify applied settings.

Minimal future implementation work belongs to the test harness:

1. Add a real-Tailscale execution adapter to the existing driver. Attach to Root-provisioned client namespaces; replace hard-coded WireGuard addresses and counters with the alias inventory. Keep existing scenario assertions and cleanup ownership. Do not let the old cleanup destroy externally owned nodes between cases.
2. Allow explicitly targeted sender/receiver pairs and a third observer. Do not spread `1 - index` assumptions into three-node scenarios.
3. Expose the existing four-text concurrent helper under production-safe feature gating without `current-engine` or `dev-tools`. If concurrent files need a helper, call existing `SendFiles` operations and return their reports; do not create production workflow APIs.
4. Add thin testhost commands for stable `CancelEntryReceive { entry_id, attempt_id }` or `CancelInboundTransfer { transfer_id, reason }`, using the actual operation/result types. Obtain identifiers from existing progress/results. Define their JSON contract before implementation. These commands do not exist today.

No new CLI flags are claimed as available. Until the adapter is implemented, use the existing host protocol directly from one controller. Existing `node scripts/testing/pr343-e2e.mjs --host ... --case L01` runs WireGuard, not Tailscale.

## Staged matrix and original deadlines

Run one minimum smoke first. Freeze its artifact before escalation. The initial smoke uses only A/B; C may already be authenticated but does not join their Engine space. Stop escalation on a smoke failure and preserve the failed attempt.

| Case | Action and required proof | Stage |
| --- | --- | --- |
| T00 | Confirm unique authenticated nodes, kernel `tailscale0`, source identities, namespace isolation, exact routes, relay policy, and positive/negative route controls. | Gate |
| T01 | Fresh A/B pairing; A→B and B→A text and 512 KiB file; actual readback hashes and sizes; sender delivered/duplicate settlement; exact overlay confinement. Record observed Tailscale class without requiring direct. | Minimum smoke |
| T02 | Restart A, then B, with fixed ports, same profiles, and in-memory secure storage. Require passive reconnect and all four transfer directions/kinds. No manual RecoverNetwork rescue in this case. | Escalation |
| T03 | Reuse L03 sequence: change B port to random; prove port changed; let A learn it through real traffic; flush and cold-restart A; deny B's new reverse Engine dials; require A-origin reconnect plus text/file. Repeat separately with a correct configured hint as L03C control. | Escalation |
| T04 | Tailscale down/up on B, leaving Engine running. Prove tunnel loss and no physical or relay escape; restore and require bounded recovery and new bidirectional transfers. | Escalation |
| T04D | Restart only tailscaled while the container and tmpfs stay alive. Require a verified supervisor/anchor, the same node identity, route proof, and Engine recovery. A PID-1 daemon without such lifecycle support blocks this case. | Same-identity escalation |
| T05 | Restart/recreate B container with lost tmpfs state. Root sends a new actual URL; user authorizes the new node. Record identity replacement, refresh exact-address route controls, and test the unchanged Engine profile without re-pairing. Preserve stale-hint failures and isolate any configured-hint repair control. Remove old and new test-node registrations during cleanup. | New-node, user-dependent escalation |
| T06 | Simultaneous distinct text sends in both directions; then two concurrent file receives on one receiver. Cancel one observed in-flight receive; assert the sibling hashes and settles, cancellation reaches its exact result, presence remains usable, and a later transfer succeeds. | Requires host adapter |
| T07 | Keep C in the tailnet but in a separate Engine space. A/B traffic must not appear in C history, active clipboard, or managed receive progress. Then use fresh profiles with A/B/C admitted together; target B explicitly and verify C remains unchanged during the observation window. Run a B↔C positive control to prove C is functional. | Three-node escalation |
| T08 | Sustain Tailscale direct; separately sustain DERP with Engine relay off. Repeat bidirectional text/file. Classify mixed or peer-relay honestly. | Transport escalation |
| T09 | Separate `privateLAN`-only and kernel `wireguard`-only controls; then dual-path availability and selected-path loss. Keep Engine relay off and prove path counters/readbacks. Do not call a private-IP WireGuard path real Tailscale. | Cross-path escalation |
| T10 | Repeat T01/T02/T03/T04 on a second real host behind another NAT/WAN. Record actual topology and separate outcomes. | NOT_RUN until another host exists |
| T11 | Explicitly authorized narrow ACL denial/reallow between test aliases; prove allowed-before, denied, allowed-after, unchanged bystander access. | Optional; requires explicit user ACL approval |

For T06, establish overlap through existing progress and use a namespace-local rate limit only if needed to keep a file in flight. A shared UDP loss rule cannot prove sibling cancellation isolation. A cancellation issued after completion is an invalid experiment. Do not choose a huge file to defeat the original deadline. Require no completed receive for the canceled attempt if cancellation wins; if completion wins, record the race and rerun a new attempt without overwriting it. Inspect the canceled sender's actual terminal projection; do not demand `delivered` for a canceled operation.

For T07, synthetic content must be unique per explicit target. Inspect C throughout the existing file window and a 20-second quiet interval. Scope the conclusion to targeted sends and these observation surfaces; a general active-clipboard broadcast must not be mislabeled a targeted send. Packet visibility inside a shared VPS is not proof of content disclosure; conversely, content readback by C is a failure even if A/B succeeded.

For T09, use separate owned interfaces/subnets and current LAN-only filtering. Pure privateLAN permits only its selected interface. Pure WireGuard follows existing L01 isolation. Dual-path trials first prove each path independently, then observe actual route selection without assuming priority. Remove the selected path and measure native recovery; verify the restored path in a new window. Add an overlay-publication-off control on fresh profiles without injected/persisted hints; do not claim the flag revokes already supplied dial hints. Retain existing S03 candidate-source and zero-packet assertions; Tailscale tests do not replace them or relax public-address filtering.

Preserve the original budgets and clock boundaries from the frozen driver:

| Assertion | Budget |
| --- | --- |
| Host command watchdog | 120 seconds |
| Pairing setup convergence after join response | 30 seconds |
| Paired identity lookup | 5 seconds |
| Membership usability and passive online convergence | 20 seconds each |
| Text receiver readback after send response | 20 seconds |
| File receiver readback after send response | 60 seconds |
| Sender delivered/duplicate settlement after readback | 10 seconds |
| Peer-stop observation in learned-address restart | 25 seconds |
| Host shutdown wait | 15 seconds |

Also record total send-call and end-to-end elapsed times so sequential budgets are visible. Do not move clocks or extend a budget to make DERP pass. User login waiting is outside scenario clocks and is explicitly `AUTH_PENDING`. After re-authentication readiness, start the original recovery clock. Timeouts remain failed attempts even when later recovery succeeds.

## Pass/fail and artifact contract

A passing case requires all required assertions, correct frozen identities, real receiver bytes, expected sender terminal status, complete route evidence, valid capture accounting, and privacy/cleanup success. Online state, ping, send return, a large byte counter, or `complete: true` alone is insufficient.

Extend the existing sanitized `result.json` with a versioned schema. Keep runtime artifacts outside tracked documentation. Write atomic checkpoints after each case, and never replace an earlier failed attempt with a passing retry. Minimal shape:

```json
{
  "schema": 2,
  "task": "pr343-real-tailscale-design",
  "attempt": "opaque-run-alias",
  "complete": false,
  "execution_status": "NOT_RUN",
  "identity": {
    "engine_commit": "cd774b4c5db78c0470d2cdbefd2e4bef1c1cb36f",
    "iroh_commit": "1ea24ade280dd73fe4e5cfb68e6a657b96be3bb5",
    "source_clean": null,
    "adapter_diff_sha256": null,
    "binary_sha256": null,
    "deployed_binary_matches": null,
    "driver_sha256": null,
    "image_digest": "sha256:ed6480b33c6eb67c9cbd203ed42b9fb57e361986725b6856def71b7a3d9e276c",
    "tailscale_version": "1.104.4",
    "deployment_identity_source": "user_confirmed_root_deployment"
  },
  "topology": "same-host-tailnet",
  "results": [],
  "cleanup": {"local_resources_removed": null,"logout_confirmed": null,"admin_nodes_removed": null},
  "privacy": {"passed": null,"scan_files": 0,"canary_matches": 0}
}
```

Each case result contains ID, iteration, start/end monotonic times, original deadline, outcome (`passed`, `failed`, `test_error`, `skipped`, `auth_pending`), category (`product`, `environment`, `evidence`, `harness`), fixed reason, failed-attempt flag, and dependencies. Include:

- Node and process aliases; image/binary identity matches; authentication state without identity material.
- Engine route label, Iroh direct/relay evidence, Tailscale class per window, and topology class.
- Exact-route/source match booleans; wildcard socket declaration; selected-interface alias; allowed/bypass counters and capture accounting/drop counts.
- Per-transfer source/target alias, synthetic payload size, expected and actual SHA-256, readback match, sender terminal states, cancellation result, and sibling result.
- Port-changed boolean, learned/configured hint distinction, cold-process proof, reverse-dial deny-rule hit and interval, profile continuity boolean, and recovery durations.
- C's negative observations plus positive-control result; policy-control outcomes; cleanup and privacy results.

Hash every exported artifact and write an `artifact-manifest.json` of relative artifact names, lengths, and SHA-256 values. Preserve the commands/templates and non-secret parameters needed to repeat the case. A verifier recomputes hashes, checks source and deployed-binary identity, validates required case coverage, compares expected/actual payload hashes, checks deadlines and route evidence, and rejects missing fields. This is a repeatable E2E artifact, not a unit test.

Only synthetic content hashes may be exported. Do not export secure storage, node keys, authkeys, login URLs, full invitations, endpoint/device IDs, actual addresses, names, filesystem paths, raw CLI status, raw errors, packet captures, profile copies, or daemon state. Parse raw data in memory into allowlisted aliases and fixed categories. Apply existing canary scanning to Engine profile/database/WAL/log files before deletion; separately scan candidate exported artifacts and check forbidden fields. Preserve only counts and hashes. Do not dump Tailscale credential state to scan it. Permission `0600` alone does not make secret exports acceptable.

## Cleanup, interruption, and authorization boundaries

1. On interruption, checkpoint the sanitized result with `complete=false`. Keep live containers only while Root is actively waiting for user authorization or resuming this task. Record ownership aliases and pending phase without secrets.
2. Stop Engine processes gracefully, collect allowed terminal observations, and close/drain all readers before freezing results. Kill only verified owned processes if shutdown exceeds its existing deadline.
3. Remove only namespace-local fault tables/rate limits and owned helpers. Verify no owned process remains. Do not flush host firewall or routing state.
4. Run `tailscale logout` for each authenticated test daemon and verify a logged-out state. `tailscale down` only disconnects; it is not credential cleanup. Logout expires login, but ordinary nodes can remain in the admin console [3].
5. Give the user the private node inventory needed to remove the exact A/B/C test devices from the tailnet admin console. Root performs removal only if separately authorized. Do not remove unrelated devices. Record node-removal confirmation separately from logout and local deletion.
6. Delete only the inventoried test containers, owned networks, tmpfs state, and disposable profiles after privacy checks. Do not prune Docker globally or delete shared images/caches. Confirm resources are absent.
7. If authorization never completes, remove unauthenticated local resources and still reconcile any partially authorized nodes. If logout cannot be reached, report residual administrative cleanup explicitly. Do not claim total cleanup while nodes remain.

Re-authentication templates for Root, after preserving the current settings and notifying the user:

```bash
docker exec "$TS_B" tailscale down
docker exec "$TS_B" tailscale up --accept-dns=false --accept-routes=false --ssh=false --timeout=30s
docker exec "$TS_B" tailscale up --force-reauth --accept-dns=false --accept-routes=false --ssh=false --timeout=30s
docker exec "$TS_B" tailscale logout
```

These are separate lifecycle cases, not one sequence to run blindly. Re-authentication must deliver a fresh actual URL to the user. Do not force re-authentication during resumable initial login. ACL denial is optional and blocked until the user explicitly authorizes the exact narrow policy edit and rollback. No account-wide ACL rewrite is implied by this plan.

## Verification boundaries and next actions

Root should first deliver actual A/B/C login URLs, while preserving the same pending daemons. Then freeze source/artifacts and implement only the execution adapter needed for T00/T01. Run the single smoke and export its verified artifact. Add restart/address and lifecycle cases next. Add production-safe concurrency/cancellation adapters only when reaching T06. Escalate to different-host/NAT/WAN testing only after a second independently located host is authorized.

Open requirements are pinned-image CLI compatibility, a verified live-container daemon supervisor for T04D, actual account policy/device approval, literal exact-IP binding if required, and the missing production-safe host commands. The image digest and version are user-confirmed deployment facts; Researcher did not independently inspect them. No runtime conclusion is available for any of these.

**NOT_RUN:** provisioning, authentication, login URL generation, kernel-network validation, all T00–T11 cases, builds, feature-graph resolution, receiver readback, sender settlement, deployment hash readback, privacy/runtime cleanup verification, native GUI/clipboard/keychain, mobile devices, real suspend/wake, cross-host/NAT/WAN, and ACL changes. Repository Cargo/test gates are intentionally not run under the assignment's explicit no-build/no-test scope. This document is a design deliverable, not an acceptance report.

## Official references

Retrieved 2026-11-02. Current web documentation must be checked against the actual pinned client version before execution.

1. [Docker configuration parameters](https://tailscale.com/docs/features/containers/docker/docker-params): kernel TUN requirements, `TS_USERSPACE`, `TS_AUTH_ONCE`, and state directory. [Standalone Docker setup](https://tailscale.com/docs/features/containers/docker/how-to/connect-docker-standalone) supplies the official container deployment pattern; its authkey example is deliberately not used here.
2. [Connection types](https://tailscale.com/docs/reference/connection-types): direct, DERP, peer relay, transition behavior, and status/ping interpretation. [DERP routing troubleshooting](https://tailscale.com/docs/reference/troubleshooting/network-configuration/derp-routing) supports the route diagnostic distinction.
3. [Tailscale CLI](https://tailscale.com/docs/reference/tailscale-cli): `up`, `down`, `--force-reauth`, `status`, `ping`, and `logout`. Logout removal of ephemeral nodes must not be generalized to ordinary nodes.
