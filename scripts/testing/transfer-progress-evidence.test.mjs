import assert from 'node:assert/strict'
import test from 'node:test'
import { verifyTransferProgress } from './transfer-progress-evidence.mjs'

function fixture({ legacyReceiver = false, legacySender = false, negotiated, change } = {}) {
  const view = {
    entry_id: 'PRIVATE_SOURCE', target_device_id: 'PRIVATE_PEER', items_total: 1,
    items_completed: legacyReceiver ? 0 : 1, items_unconfirmed: legacyReceiver ? 1 : 0,
    items_failed: 0, items_cancelled: 0, items_running: 0,
    state: legacyReceiver ? 'unconfirmed' : 'completed', freshness: legacyReceiver ? 'unconfirmed' : 'settled',
    attempt_id: legacyReceiver ? null : 'PRIVATE_ATTEMPT',
    items: [{ kind: 'file', index: 0, state: legacyReceiver ? 'unconfirmed' : 'completed',
      round: legacyReceiver ? null : 0, completed_bytes: legacyReceiver ? null : 50, total_bytes: legacyReceiver ? null : 50 }],
  }
  change?.(view)
  const sender = { label: 'A', hostIdentity: { capabilities: legacySender ? [] : ['transfer-progress'] },
    async call(command) {
      if (command === 'outbound_progress') return view
      assert.equal(command, 'reconcile_outbound_progress')
      return legacyReceiver ? 'unsupported' : 'settled'
    } }
  const receiver = { label: 'B', id: 'PRIVATE_PEER', hostIdentity: { capabilities: legacyReceiver ? [] : ['transfer-progress'] },
    async call(command) {
      if (command === 'outbound_progress_list') return []
      assert.equal(command, 'receive_progress')
      return { state: 'completed', items: [{ state: 'completed' }] }
    } }
  return { sender, receiver, sent: { entry_id: 'PRIVATE_SOURCE' }, receivedEntry: 'PRIVATE_RECEIVED',
    mode: 'direct', until: async predicate => { assert(await predicate(), 'predicate did not establish the required outcome') },
    protocols: async () => (negotiated ?? [legacyReceiver || legacySender ? 'transfer_progress_v1' : 'transfer_progress_v2']).map(protocol => typeof protocol === 'string' ? {protocol, selected_path: 'direct'} : protocol) }
}

test('current facts require a real protocol observation and omit identifiers from evidence', async () => {
  const proof = await verifyTransferProgress(fixture())
  assert.equal(proof.source.completed, 1)
  assert.equal(proof.negotiated_protocol, 'transfer_progress_v2')
  assert(!JSON.stringify(proof).includes('PRIVATE'))
  await assert.rejects(verifyTransferProgress(fixture({ negotiated: ['blob', 'other'] })), /actual progress ALPN/)
  await assert.rejects(verifyTransferProgress(fixture({ negotiated: [{protocol: 'transfer_progress_v2', selected_path: 'relay'}] })), /actual progress handshake path/)
})
test('legacy outcomes remain unconfirmed and cannot invent byte facts or a v2 handshake', async () => {
  const proof = await verifyTransferProgress(fixture({ legacyReceiver: true }))
  assert.equal(proof.source.unconfirmed, 1)
  assert.equal(proof.source.reconciliation, 'unsupported')
  await assert.rejects(verifyTransferProgress(fixture({ legacyReceiver: true, change: view => { view.items[0].completed_bytes = 99 } })))
  await assert.rejects(verifyTransferProgress(fixture({ legacyReceiver: true, negotiated: ['transfer_progress_v1', 'transfer_progress_v2'] })), /unexpectedly negotiated v2/)
  await assert.rejects(verifyTransferProgress(fixture({ legacyReceiver: true, change: view => { view.state = 'completed' } })), /predicate/)
})
test('old sender needs local saved facts and must not appear as a new outbound owner', async () => {
  const input = fixture({ legacySender: true })
  const proof = await verifyTransferProgress(input)
  assert.equal(proof.local_receiver_completed, true)
  assert.equal(proof.no_fabricated_outbound, true)
  assert.equal(proof.frozen_sender_query, 'not_required')
  const call = input.receiver.call
  input.receiver.call = command => command === 'outbound_progress_list' ? [{ entry_id: 'PRIVATE_SOURCE' }] : call(command)
  await assert.rejects(verifyTransferProgress(input), /invent a local outbound/)
})
