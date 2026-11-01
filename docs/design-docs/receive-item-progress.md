# Host receive item snapshots

## Owner and query

`QueryEntryReceiveProgress` returns the current durable receive attempt, including
terminal attempts. It returns `None` when no receive attempt exists.
`ListEntryReceiveProgress` returns nonterminal attempts only. Both operations read
the same Application receive owner and the existing bounded progress catalog.
They do not replay events or maintain a separate progress ledger.

## Content association

`item_id` remains opaque and bound to `attempt_id`. Hosts must not parse it.
`content` has one of these forms:

- `file { index }`: zero-based position in the original incoming file list. This
  list is the original envelope's blob references with representations excluded,
  in original order. Failed, missing, cancelled, and unselected files retain their
  positions. Duplicate names, sizes, or hashes do not merge positions.
- `representation { index }`: original envelope representation position. This
  namespace is separate from file positions. Removed failed representations do
  not renumber original positions.
- `directory_unit`: the entire original incoming file set, which can include
  multiple roots. Display this counter for the whole receive unit. It cannot be
  attached to one particular root or member in a multi-root file set.

Hosts can build the canonical original file list from `file` locators, ordered by
`index`, retaining every missing placeholder. A current local history file list
can have a different order: saved files precede missing placeholders.
`history_file_index`, when present, addresses that current `text/uri-list` order.
It does not address representation arrays or the original sender list. It is
unavailable before the first saved history snapshot. Requery history after a
receive snapshot revision changes; do not retain a history index across changes.
Display names remain in authorized content metadata, outside progress events.

## Bytes and results

`transfer` is optional. When available, it contains `received_bytes`, optional
`total_bytes`, and `round`. Counters describe the current fetch round. They are not
cumulative bandwidth. A representation total uses protected transfer bytes;
plain file totals use the declared file size. A directory aggregates its root.
Unknown totals remain `None`; an empty file has `Some(0)`. A host must not divide
by zero or display a percentage when the total is unknown.

Aggregate `completed_bytes` continues to count saved bytes. Reaching the transfer
total only means that the bytes have arrived. Verification, decryption, export,
or save can still fail. The item remains nonterminal until the receive owner
confirms materialization. Failed and cancelled items keep their individual
outcomes. Selective retry preserves saved unselected items.

## Correlation and ordering

`snapshot_version` contains `producer_id`, `attempt_revision`, and
`snapshot_revision`. Revisions come from the original progress catalog. The
producer identifies one runtime catalog lifetime. Within that producer,
`attempt_revision` orders accepted attempts; UUID text is never sortable authority.
`snapshot_revision` orders the original transfer facts, including round changes
and confirmed materialization results. Identical transfer facts keep the same
revision. An already known local failure can appear before cleanup settles; the
parent remains nonterminal. Query generations also order these business fields.

A host applies whole snapshots. Within one producer it rejects an older attempt
revision or an older snapshot revision. A changed producer invalidates all old
runtime snapshots; a host accepts it only from a new authoritative query context.
The host must discard replies from a retired query/session context. Serialize
queries for a detail pane, or reject responses from an older query generation.
Start a new query generation after retry or reconnect. An unavailable snapshot
is not permission to adopt an older response without checking that generation.
Within one
attempt, `round` advances when the original fetch retries. Directory rounds
advance when any member retries.

For a planned ordinary file or representation fetch,
`TransferProgress.transfer_id` equals the item's opaque `item_id`. Entry and
attempt must also match. A directory member transfer does not identify the unit.
Provisional transfers do not participate in this projection. Their events have
no host file association. Adoption does not authorize a host to relabel an old
provisional transfer ID. The subsequent query establishes the accepted plan
association and current facts; it does not provide a provisional-ID mapping.

`ReceiveProgressChanged`, existing transfer/attempt events, and `RefreshRequired`
are invalidation signals. A host requeries the whole snapshot. It must not apply
event byte counters to an item, even when an old transfer ID matches. A late
previous-attempt/round event can cause a redundant query but cannot advance the
wrong file. No host timer or host progress ledger is required. On reconnect or
opening a detail pane, query immediately without replaying previous events.

## Unavailable runtime facts

When the original plan or progress catalog is unavailable, `snapshot_version`,
`content`, `transfer`, and `history_file_index` are `None`. Existing durable item
states and saved-byte aggregates remain available. This applies to legacy
records, unsupported identities/plans, catalog eviction, superseded source
mappings, session retirement,
and process restart. Do not reconstruct counters or positions from durable
opaque IDs. The catalog retains at most 256 tasks globally and 64 per peer.
A reconnect without Engine restart retains facts while the task remains retained.
There is no new plaintext progress store or persistence format.

## Bindings

UniFFI exposes a typed content enum and optional transfer/version records. OHOS
exposes a content object with `kind` and optional `index`; `directory_unit` has no
index. OHOS encodes every 64-bit byte counter and revision as a decimal string.
All bindings preserve absent values and the Rust result semantics.

## Repeatable evidence

Run the real Engine receive scenarios from the repository root:

```bash
UC_RECEIVE_PROGRESS_EVIDENCE="$PWD/.artifacts/receive-item-progress/evidence.json" \
  just cargo test -p uc-engine --features dev-tools --lib real_engine_file_addressable_query --locked -- --nocapture
just cargo test -p uc-engine --features dev-tools --lib receive_retry --locked
```

The evidence contains synthetic query snapshots, opaque item identities, results,
and SHA-256 file hashes. It contains no credentials, names, paths, or real content.

Run the complete Engine-to-binding evidence pipeline with:

```bash
scripts/testing/run-receive-item-progress-e2e.sh
```

The binding checks start the real Engine transfer scenario, then validate its
query results through the production binding projections. They also validate the
mixed-content, directory, and zero-byte artifacts from the Engine scenarios.
The final SHA-256 manifest identifies the evidence files. This checks binding
projection code on the host platform; it does not claim native device execution.
