// 仅从稳定完整操作和实际握手日志验证；工件不复制业务身份、文件名或内容。
import assert from 'node:assert/strict'

const capable = node => node.hostIdentity?.capabilities?.includes('transfer-progress') === true

export async function verifyTransferProgress({ sender, receiver, sent, receivedEntry, until, mode, protocols }) {
  assert(typeof sent?.entry_id === 'string' && sent.entry_id.length > 0, 'the actual send report must provide the source entry')
  assert(typeof receivedEntry === 'string' && receivedEntry.length > 0, 'exact readback must provide the local entry')
  const newSender = capable(sender)
  const newReceiver = capable(receiver)
  assert(newSender || newReceiver, 'progress evidence requires a current endpoint')
  const proof = { sender: sender.label, receiver: receiver.label, new_sender: newSender, new_receiver: newReceiver }
  if (newReceiver) {
    const local = await receiver.call('receive_progress', { entry: receivedEntry })
    assert(local && local.state === 'completed' && local.items.length === 1)
    assert(local.items.every(item => item.state === 'completed'))
    proof.local_receiver_completed = true
  }
  if (newSender) {
    let view
    await until(async () => {
      view = await sender.call('outbound_progress', { entry: sent.entry_id, peer: receiver.id })
      return view?.state === (newReceiver ? 'completed' : 'unconfirmed')
    }, 20_000, 'source progress did not reflect the actual peer capability')
    assert.equal(view.entry_id, sent.entry_id)
    assert.equal(view.target_device_id, receiver.id)
    assert.equal(view.items_total, 1)
    assert.equal(view.items.length, 1)
    assert.equal(view.items[0].kind, 'file')
    assert.equal(view.items[0].index, 0)
    const reconciliation = await sender.call('reconcile_outbound_progress', {
      entry: sent.entry_id, peer: receiver.id, attempt: view.attempt_id,
    })
    assert.equal(reconciliation, newReceiver ? 'settled' : 'unsupported')
    const after = await sender.call('outbound_progress', { entry: sent.entry_id, peer: receiver.id })
    assert.equal(after.state, view.state)
    assert.equal(after.items_total, 1)
    assert.equal(after.items_failed + after.items_cancelled + after.items_running, 0)
    if (newReceiver) {
      assert.equal(after.items_completed, 1)
      assert.equal(after.items_unconfirmed, 0)
      assert.equal(after.freshness, 'settled')
      assert.equal(typeof after.attempt_id, 'string')
      assert.equal(after.items[0].state, 'completed')
    } else {
      assert.equal(after.items_completed, 0)
      assert.equal(after.items_unconfirmed, 1)
      assert.equal(after.freshness, 'unconfirmed')
      assert.equal(after.attempt_id, null)
      assert.equal(after.items[0].state, 'unconfirmed')
      for (const field of ['round', 'completed_bytes', 'total_bytes']) assert.equal(after.items[0][field], null)
    }
    proof.source = { state: after.state, freshness: after.freshness, completed: after.items_completed,
      unconfirmed: after.items_unconfirmed, total: after.items_total, reconciliation }
  } else {
    const outgoing = await receiver.call('outbound_progress_list')
    assert(outgoing.every(view => view.entry_id !== sent.entry_id && view.entry_id !== receivedEntry),
      'receiving an old source must not invent a local outbound intent')
    proof.frozen_sender_query = 'not_required'
    proof.no_fabricated_outbound = true
  }
  const current = newSender ? sender : receiver
  const negotiated = await protocols(current)
  const expected = newSender && newReceiver ? 'transfer_progress_v2' : 'transfer_progress_v1'
  const matches = negotiated.filter(row => row.protocol === expected)
  assert(matches.length > 0, 'actual progress ALPN was not observed')
  assert(matches.every(row => row.selected_path === mode), 'actual progress handshake path was not established')
  if (!(newSender && newReceiver)) assert(!negotiated.some(row => row.protocol === 'transfer_progress_v2'), 'frozen endpoint unexpectedly negotiated v2')
  return { ...proof, negotiated_protocol: expected, selected_path: matches[0].selected_path, actual_handshake_observed: true, actual_path_observed: true }
}
