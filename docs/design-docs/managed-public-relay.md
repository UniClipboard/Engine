# Anonymous managed public relay

The default P2P relay uses the Cloud control plane without a Cloud account.
The initial candidate is Shanghai (`cn-shanghai-1`, region `cn-shanghai`,
`https://sha1.uc-relay.z2blog.com/`). Core owns this bundled candidate and the
unchanged Disabled > Custom > BuiltIn routing precedence. The candidate is
neither an access credential nor a permanent catalog entry.

## Ownership and lifecycle

The Infra network node owns one sequential managed-relay worker. The Engine
composition root enables it only for BuiltIn routing. Custom relays retain
their existing secure-storage credentials. LAN-only and Custom routing never
request the managed-relay catalog or anonymous credentials.

The worker reads `https://cloud.uniclipboard.app/api/relay/v1/nodes` at most
once per 30 seconds. A valid response replaces the entire in-memory catalog.
An empty response removes the managed relay. An unsuccessful response retains
the last valid catalog, including a previously empty catalog. Restart starts
with the bundled candidate and requests fresh credentials; there is no disk
cache or persistent credential.

The worker prefers Shanghai while it remains in the valid catalog, otherwise
the first published candidate. It requests a credential from `/credentials`
using the selected stable relay ID and the current endpoint's lowercase
64-hex ID. The credential response URL is authoritative, even when it differs
from the catalog URL. Only HTTPS origins without user information, paths,
queries, or fragments are accepted. HTTP redirects are disabled.

Before the final two minutes of the ten-minute credential lifetime, the worker
requests a replacement. If renewal fails, it withdraws the installed credential
early. It never treats a cached URL as authorization. Changing the catalog
selection or the credential destination removes the old relay configuration.
No credential is sent to another relay. A network-stack RelayMap change wakes
an active connection and replaces its client. Each reconnect reads the latest
configuration; removed credentials cannot be reused by that actor.
The installed credential also carries absolute expiry. Each reconnect checks
wall-clock expiry before dialing. Relay status changes wake the worker, which
checks wall-clock renewal as well as the monotonic deadline. Suspend therefore
cannot turn an expired credential into permission to reconnect. Removed actors
exit; an actor waiting for renewal belongs only to the currently installed relay.

The single worker uses separate bounded backoffs for catalog and credential
requests: five seconds doubling to five minutes; a 403 waits five minutes.
429 and other failures honor Retry-After seconds or HTTP dates, capped at five
minutes. Catalog retries also obey the 30-second minimum. Failure leaves direct
P2P and local discovery available. It does not enable LAN compatibility or
silently select a user-configured relay.

Node shutdown aborts and joins the worker before closing the endpoint.
Dropping a partially constructed node starts no worker. Dropping a live node
also aborts the worker through its owned task handle.

The existing relay overview receives only actually installed URLs. A bundled
candidate is not marked in effect before credential issuance. Catalog-driven
URL changes do not imply that a settings restart is pending. Managed tokens
never become user-configured credentials.

## Resource and security bounds

There is one HTTP client, one worker, and one selected managed relay per node.
There are no request queues or concurrent Cloud calls. Each HTTP request has a
five-second timeout. Catalogs contain at most 32 nodes and 64 KiB of JSON.
Credential responses contain at most 16 KiB and tokens at most 4 KiB. URLs,
relay IDs, regions, and catalog versions have bounded lengths.

Credentials remain in memory and native iroh sends them only in the
Authorization header. They are not account access tokens. Debug output for
the controller, RelayConfig, and relay client builders redacts tokens.
Diagnostics use fixed failure categories and never include response bodies,
tokens, endpoint IDs, or destinations. No shared relay secret is compiled into
Engine. Gateway signature, identity, expiry, and policy checks remain required.

## Acceptance

The transport acceptance test uses a real authenticated relay server to prove
live credential replacement and removal. Control-plane acceptance uses actual
HTTP and a real endpoint to exercise catalog replacement/removal, authoritative
credential URLs, renewal, endpoint identity changes, backoff, and invalid input.
Node lifecycle acceptance checks routing isolation and worker settlement.
Run `bash scripts/testing/check-managed-relay.sh`. Its optional live scenario
requires the deployed gateway to forward iroh's `/ping` probe route. Cloud
catalog health alone does not prove that a native endpoint can select a relay.

The external contract is maintained in
[Cloud's relay control plane](https://github.com/mkdir700/uc-cloud/blob/main/docs/RELAY_CONTROL_PLANE.md).
Execution status belongs in the [implementation plan](../exec-plans/active/2026-11-03-managed-public-relay.md).
