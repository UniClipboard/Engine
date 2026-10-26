import assert from 'node:assert/strict'
import test from 'node:test'
import { verifyProgressCell } from './verify-progress-connectivity.mjs'

function cell(legacy = false) {
  const mode = 'direct'
  const options = { head: 'HEAD', mode, ...(legacy ? { oldRevision: 'OLD', side: 0 } : {}) }
  const host = { anchor: 'progress-current', engine_rev: 'HEAD', capabilities: ['transfer-progress'] }
  const source = { state: 'completed', freshness: 'settled', completed: 1, unconfirmed: 0, total: 1, reconciliation: 'settled' }
  const proof = { actual_handshake_observed: true, negotiated_protocol: 'blob', paths: [{ selected_path: mode }], hosts: [host, legacy ? { anchor: 'old', engine_rev: 'OLD', capabilities: [] } : host],
    exact_bytes_verified: true, direction_count: 2, original_received_files_readable: true, exact_bidirectional_files: true, independent_original_copies: true,
    retained: Array.from({ length: 2 }, () => ({ identity: true, transport_identity: true, members: true, content: true })) }
  const transfers = ['A', 'B'].map(sender => ({ sender, receiver: sender === 'A' ? 'B' : 'A', new_sender: !legacy || sender === 'A', new_receiver: !legacy || sender === 'B', local_receiver_completed: true,
    negotiated_protocol: legacy ? 'transfer_progress_v1' : 'transfer_progress_v2', selected_path: mode, actual_handshake_observed: true, actual_path_observed: true,
    ...(!legacy ? { source } : sender === 'A' ? { source: { state: 'unconfirmed', freshness: 'unconfirmed', completed: 0, unconfirmed: 1, total: 1, reconciliation: 'unsupported' } } : { frozen_sender_query: 'not_required', no_fabricated_outbound: true }) }))
  return { options, result: { failed: false, cleaned: true, plaintext_clean: true, progress_baseline: true, faults: [], progress_transfers: transfers,
    nodes: ['A', 'B'].map(label => ({label, network_facts: [{'event.name': 'connection.protocol.negotiated', protocol: legacy ? 'transfer_progress_v1' : 'transfer_progress_v2', initial_path: mode}]})),
    records: [{ id: legacy ? 'P00-prepaired-upgrade' : 'E02-file-transfer', outcome: 'passed', proof }, { id: 'P00-prepaired-upgrade-old-pairing', outcome: 'passed' }] } }
}
for (const legacy of [false, true]) test(`strict cell gate accepts the complete ${legacy ? 'mixed' : 'new/new'} proof and rejects weaker substitutes`, () => {
  const { result, options } = cell(legacy)
  verifyProgressCell(result, options)
  for (const corrupt of [
    value => { value.progress_transfers.pop() },
    value => { value.progress_transfers[0].actual_path_observed = false },
    value => { value.progress_transfers[0].selected_path = 'relay' },
    value => { value.records[0].proof.hosts[0] = { anchor: 'progress-current', engine_rev: 'STALE', capabilities: ['transfer-progress'] } },
    value => { value.cleaned = false },
    value => { value.nodes[0].network_facts = [] },
    value => { value.records.push(value.records[0]) },
    value => { value.progress_transfers[0].source.completed = legacy ? 1 : 0 },
  ]) {
    const changed = structuredClone(result); corrupt(changed)
    assert.throws(() => verifyProgressCell(changed, options))
  }
})
test('relay proof requires real packet drops in both families on both nodes', () => {
  const { result, options } = cell()
  options.mode = 'relay'
  for (const row of result.progress_transfers) row.selected_path = 'relay'
  result.records[0].proof.paths[0].selected_path = 'relay'
  for (const node of result.nodes) node.network_facts[0].initial_path = 'relay'
  assert.throws(() => verifyProgressCell(result, options))
  result.faults = ['A', 'B'].map(node => ({ node, action: 'direct_paths_blocked', ipv4_drop_packets: 1, ipv6_drop_packets: 1 }))
  verifyProgressCell(result, options)
  result.faults[1].ipv6_drop_packets = 0
  assert.throws(() => verifyProgressCell(result, options))
})
