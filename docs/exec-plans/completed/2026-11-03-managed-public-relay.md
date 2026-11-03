# Anonymous managed public relay (#371)

Status: Engine implementation, local acceptance, and live managed-relay
connection acceptance complete. Native devices and publication remain skipped.

## Ownership and contract

The existing network node owns one managed-relay runtime. The host starts and
stops the Engine as before. Core selects Disabled, Custom, or BuiltIn routing.
The Infra runtime owns Cloud HTTP, the in-memory catalog, credentials, renewal,
and relay-map reconciliation. It does not orchestrate a business workflow.
No account, persisted token, shared secret, or new stable Engine API is required.

Disabled and Custom routing never contact the relay control plane. BuiltIn
routing starts with the Shanghai candidate (`cn-shanghai-1`, `cn-shanghai`,
`https://sha1.uc-relay.z2blog.com/`). A successful catalog replaces that candidate,
including an empty catalog. An unsuccessful request preserves the last valid
catalog. Only a credential response authorizes a destination. Credentials are
bound to the current endpoint and the requested stable relay ID. Their response
URL is authoritative and must be an HTTPS origin without credentials or query.

One sequential worker owns the cache and at most one selected managed relay.
Catalogs are limited to 32 nodes and 64 KiB; credentials to 16 KiB with a 4 KiB
token. HTTP calls have a five-second timeout and reject redirects. Catalog
refresh is at most once per 30 seconds. Renewal starts before the final two
minutes. Failures use bounded exponential backoff, honor bounded Retry-After,
and never block direct connectivity. Credentials expire in memory. Shutdown
cancels and joins the worker before closing the endpoint. Restart requests new
credentials using the current identity. There is no persisted recovery state.
The installed transport configuration also carries absolute credential expiry.
Every reconnect checks wall-clock expiry. A status change wakes the worker, so
a reconnect after suspend requests renewal even when the monotonic renewal
deadline has not elapsed. Removed relay actors exit rather than accumulating.

## Failure modes before implementation

- Cloud timeout, redirect, non-JSON, oversized body, duplicate IDs, malformed
  origins, invalid timestamps, empty tokens, or expired credentials.
- 429 with Retry-After, 403 block, 503 unavailable, and repeated outages.
- Fresh empty catalog, node removal, node URL change, and a different
  credential response URL.
- Identity replacement, credential renewal, reconnect after renewal, and
  expiry while issuance is unavailable.
- Token exposure through Debug, errors, settings, artifacts, or relay URLs.
- Existing iroh relay actors retain a token copied at construction. Updating
  RelayMap alone does not update those actors. The network-stack dependency
  must observe configuration changes before this feature can safely renew.
- Cancellation during HTTP, partial construction, and normal node shutdown.
- Custom relay or LAN-only selection accidentally starts Cloud work.

## Validation

Write transport and control-plane acceptance tests before implementation.
First prove credential replacement on a real authenticated relay connection.
Then exercise mock Cloud HTTP through the real node lifecycle, including
catalog replacement/removal, identity changes, renewal, expiry, outages, and
routing precedence. Emit a redacted repeatable acceptance artifact. Run the
repository delivery checks serially through `just cargo` and the shared target.
Native devices, cross-network routing, and publication remain skipped unless
actually executed. Live Cloud checks must never print or persist credentials.

## Executed acceptance

- Managed control-plane and node acceptance: 10 passed; live acceptance is a
  separate opt-in scenario.
- Real authenticated relay: credential replacement, expiry gating, and removal
  passed. The existing static-token transport acceptance also passed.
- Node regression: 41 passed, one existing ignored scenario; live acceptance
  was excluded from this regression command.
- Complete Engine settings acceptance: four passed, including actual restart
  through BuiltIn, Custom, and Disabled routing with real SQLite and host
  secure-storage adapters.
- Locked metadata, workspace/all-targets check, formatting, Rust style,
  repository architecture, and whitespace checks passed.
- Native devices, cross-network device matrix, publication, and resource
  benchmarks were skipped. Capacity bounds are design bounds, not measured
  memory or CPU improvements.

The iroh fix is pinned at
[`65f265d1b64114f500be1eeca90763de535be212`](https://github.com/UniClipboard/iroh/commit/65f265d1b64114f500be1eeca90763de535be212),
published on `UniClipboard/iroh` branch `fix/relay-credential-refresh`.
It adds no production log calls and
redacts relay tokens in Debug. Its configuration notification retains only one
change signal and no historical credentials.

## Live acceptance

The initial 2026-11-03 run installed an anonymous credential but did not select
a home relay within 45 seconds. The public gateway returned 404 for `/ping`.
After the gateway deployment added `/ping`, the same Engine commit
`de9edfac71dfabe2a3aa5fc417ad16e73333d1cd` passed the live acceptance in
5.59 seconds without an Engine code change. The public `/ping` returned 200.

The real node installed its anonymous credential and established a relay
connection. Its status contained one relay, no authentication rejection, and
no relay error. Shutdown completed and joined the managed worker. This proves
anonymous issuance and relay connectivity; it does not prove clipboard payload
transfer, credential renewal against the live service, or native device behavior.

Reproduce this focused check from the repository root:

```bash
UC_MANAGED_RELAY_ARTIFACT_DIR="$PWD/target/managed-relay-live-retry" \
  just cargo test -p uc-infra-p2p \
  live_managed_relay_accepts_current_endpoint_credential --lib --locked -- --ignored
```

The retry produced redacted `live.json`, `live.log`, and a source-bound manifest
under `target/managed-relay-live-retry/`. The earlier failed run remains separate.
The gateway route blocker is resolved. Native devices, cross-network device
matrix, publication, and resource measurements remain skipped.

## Reproduction and artifacts

Run `bash scripts/testing/check-managed-relay.sh` for local acceptance. Set
`UC_MANAGED_RELAY_LIVE=1` to include the real Cloud and Relay scenario. The
script writes redacted JSON, logs, source hashes, and a manifest under
`target/managed-relay-acceptance/`. A live failure preserves its evidence and
returns a failing exit code.

The runner serially reuses the repository target. Switching between the two
Cargo workspaces requires targeted rebuilding of iroh-relay outputs: its
cdylib and rlib use unhashed filenames, so one feature set can otherwise
overwrite another feature set's artifact alias. The runner preserves the MBX
action cache and rebuilds only that package at each workspace boundary.

A final repeated workspace check encountered a transient compiler-shim launch
failure (`ENOENT`) before that compiler process ran. The same shim path was
present on inspection. Repeating the unchanged check passed in 13.44 seconds.
Both the interrupted and successful logs are retained; this was not a source
compilation failure. The shared MBX wrapper remained enabled.
