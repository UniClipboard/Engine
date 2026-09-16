# Findings

## Confirmed Current State

- Commit `098d806` deleted the sponsor inbound orchestrator and introduced `HandleSpaceAdmissionMessageUseCase`, but only exposed the endpoint; no network caller was wired.
- `IrohPairingSessionAdapter` still emits `PairingSessionEvent`, and current Engine still passes the event port, but application has no subscriber.
- `PrepareJoinSpacePort` and `PrepareSpaceAdmissionMessagePort` have no production implementations.
- `PairingAdmissionOutboxDelivery` only handles `CancelRequested`; every other purpose is deferred.
- The current endpoint flattens message type and predecessor information into opaque bytes and returns opaque bytes, so it cannot drive the typed wire protocol.
- Spec 027 explicitly excluded Core, Infra, Engine, bindings, database, and network integration, leaving the vertical slice incomplete.
- The current Engine check fails in unfinished Infra migration code before Engine assembly can validate admission wiring.

## User Direction

- Treat the new protocol as a clean start.
- Do not preserve legacy protocol behavior, adapters, wire compatibility, fallback, dual writes, or parallel implementations.
- Research first and deliver a spec before implementation.

## Authoritative Requirements

- ADR-017 keeps pairing private: it verifies invitation, secret, peer identity, KeyPackage, and transports decisions, but it never owns membership persistence or declares success.
- Admission success means the joiner verified full signed history, both sides saved the same AddDevice fact and target security state, and the activation boundary completed; transport or handshake success is insufficient.
- A pairing session is not durable state. Recovery must use a high-entropy attempt-bound continuation credential and may not depend on the original invitation or an open connection.
- ADR-022 requires every explicit user `JoinSpace` to create a new operation, while retries and restarts keep the same persistent admission identity. Safe supersession ends once Prepared is durably saved.
- All admission records, messages, routes, names, credentials, security material, and histories are sensitive and must use MasterKey AEAD at rest; logs must remain redacted.
- Spec 027's target expected `admission/protocol/`, typed authenticated-message endpoints, and one `SpaceApplication`, but intentionally excluded every external adapter and network integration needed to make that target runnable.
- The new specification must supersede only admission protocol/runtime sections of Specs 017/023/025/027 while retaining their current membership-history, activation, cancellation, supersession, and reset invariants unless explicitly replaced.
- `AddDevice` remains the only positive membership fact; invitation, proof, receipt, completion, presence, profile rows, and transport identity cannot independently grant ordinary access.
- Every reply-producing stage must persist the new stage and exact replayable outbox before sending. Duplicate input replays the saved reply; out-of-order input never synthesizes progress.
- The sponsor is the sole commit/reject authority. Before formal commit a cancel may produce Rejected; after formal commit the same admission moves forward and later user exit is a separate RemoveMember action.
- The local encrypted invitation claim, attempt binding, and consume outbox are one atomic fact; remote rendezvous/mDNS consumption is cleanup, not authority.
- Target OpenMLS state must be staged and exactly replayable without generating new randomness. The application decides when to prepare/commit/activate; Infra performs cryptographic operations.
- Existing established membership history and activation-receipt rules remain authoritative. The clean start removes admission protocol compatibility, not signed membership-history correctness.
- The new implementation may reject/discard pre-cutover in-flight admission records under one explicit reset rule, but it must not silently reinterpret them or maintain a migration reader.

## External Research Decisions

- RFC 9420 defines Add + Commit + Welcome as the standard existing-member-driven MLS join. The new protocol must use OpenMLS for these operations and must not recreate group-change cryptography.
- Iroh routes connections directly to a protocol handler by ALPN. A direct admission `ProtocolHandler` can call the application endpoint and eliminates the lossy `PairingEventPort` subscription channel.
- Iroh's official guidance recommends owned task tracking rather than detached tasks for serious protocols; admission connection tasks must be joined during shutdown.
- For the locked Iroh 1.0.0-rc.1 API, Router already owns each `ProtocolHandler::accept` future and calls/awaits `ProtocolHandler::shutdown` before aborting remaining accepts. The new handler must not add its own TaskTracker/JoinSet; its shutdown hook only coordinates application stop and commit completion.
- RFC 9807 OPAQUE provides mutual password authentication without sending or storing the plaintext password and is suitable for the sponsor/joiner asymmetric roles. The Rust `opaque-ke` project is RFC-based and audited; the spec will require a pinned reviewed release, Argon2 support, and RFC vectors.
- RFC 9106's memory-constrained recommendation fixes Argon2id at 64 MiB, three passes, four lanes, a 128-bit salt, and a 256-bit tag; Spec 028 uses these exact parameters rather than a runtime-tuned configuration.
- RFC 9382 SPAKE2 is symmetric but the available RustCrypto implementation has unresolved RFC-transcript and memory-hardening concerns. It is not selected.
- Idempotency patterns require one stable operation/message identity for retries and a new identity for a new user operation. This matches ADR-022 and the existing attempt/message model.

## Current Assembly Findings

- Current Iroh `ProtocolHandler` already owns the accepted connection and can obtain the authenticated remote endpoint identity; the new handler can invoke the application endpoint directly without a subscriber runtime.
- `IrohNode::install_pairing` currently registers the old session/event adapter before the router starts. The new assembly should instead install one admission handler after the application endpoint exists and before `Router::spawn`.
- Current Engine starts the Iroh router before constructing `SpaceFacade`, so assembly order must change or the handler must support a one-time endpoint binding before accepting traffic. The spec will require reorder-and-install, not a mutable runtime slot.
- Stable Engine `JoinSpace` and binding contracts already expose the required product outcomes and errors. Internal protocol replacement should preserve those public operation/result shapes unless the spec explicitly removes obsolete error codes.

## Clean-Start Model Decisions

- The current `SpaceJoinRecord` is a wide optional-field bag that permits impossible combinations. The new Core aggregate will use role/stage enum variants carrying only the data legal at that stage; the old persisted decoder and compatibility padding are deleted.
- One typed `SpaceAdmissionEnvelopeV1` replaces `PairingSessionMessage`, `DurableAdmissionFrame`, `DurableAdmissionMessageKind`, `AdmissionOutboxPurpose`, and opaque reply bytes as competing protocol descriptions.
- Every durable exchange is request-response over one bounded Iroh bidirectional stream. The receiver saves the exact reply before sending; retrying the same message id and digest replays the same reply.
- Admission correctness never depends on a live connection. The initial OPAQUE exchange derives an attempt-bound continuation key; later streams authenticate with a transcript-bound HMAC using the encrypted continuation key.
- The short invitation code is discovery and lookup only, not an authentication secret. OPAQUE authenticates the shared Space passphrase and binds protocol version, admission id, invitation id, both Iroh endpoint ids, and role.
- A temporary password-equivalent needed for pre-authentication restart recovery is encrypted under `ProfileAdmissionMasterKey`, zeroized after continuation-key persistence, and never logged. Sponsor OPAQUE server setup/registration material is encrypted under the Space MasterKey.
- `SpaceAdmissionProtocol` in Application is the sole full-result owner. Application only knows transport-agnostic routes, opaque channel peer ids, typed authenticated messages/replies, and an abstract one-shot exchange port.
- Infra owns Iroh connection/ALPN/stream/frame/deadline plus OPAQUE/continuation authentication. After auth and wire decode, the Iroh handler calls the Application typed-message endpoint once per business message. `PairingEventPort`, `PairingSessionPort`, subscriber channels, and their lifecycle handles are deleted rather than adapted.
- The clean cutover follows ADR-025/Spec 027's next-version Space rebuild rule: old profiles rebuild to a new single-device Space and re-pair. No old admission record, old membership branch, old invitation, or old protocol state is imported into the new runtime.
- There are currently two incomplete persistence directions: the old `admission_repository_state` repository and the new application `LoadedMembershipLedger` that embeds admission records. Neither has a complete production contract. The new spec selects one encrypted membership ledger and deletes the separate admission repository.
- The new encrypted ledger may use several ciphertext rows inside one SQLite immediate transaction, but Application sees one verified snapshot and one conditional commit. Profile admission metadata uses `ProfileAdmissionMasterKey`; active/target Space history and security payloads use the corresponding Space MasterKey; no plaintext mirror is allowed.
- The existing 4 MiB absolute wire cap is retained as a hard upper bound, with smaller per-frame caps for authentication/control messages. Length is validated before allocation and all connection/exchange deadlines are fixed, not configurable.
- Stable product operations `JoinSpace`, `CancelJoinSpace`, invitation actions, `JoinSpaceStatusSummary`, and device-trust projections remain. They are product contracts, not protocol compatibility. No protocol stages or transport actions become binding APIs.
- The existing Engine error table contains historical synchronous handshake errors. Spec 028 will retain only errors still reachable before the new join is durably Pending; asynchronous remote outcomes are projected as Pending/Rejected state instead of returned as a second synchronous flow.
- Architecture checks must be extended to reject `PairingEventPort`, `PairingSessionPort`, `PairingSessionMessage`, `DurableAdmissionFrame`, old ALPN constants, old admission store ports/tables, `PrepareJoinSpacePort`, `PrepareSpaceAdmissionMessagePort`, opaque reply bytes, and multiple protocol implementations.
- The full implementation is one unreleasable cutover branch until Core, Application, Infra, Engine, bindings, migrations, and two-device tests all pass. Spec 027's intentional outer-layer incompatibility is not an acceptable intermediate release state for Spec 028.
