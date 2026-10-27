import assert from 'node:assert/strict'
import test from 'node:test'
import { receiveFailureKind, summarizeDeliveryReport, verifyDelivery } from './clipboard-delivery-evidence.mjs'

const facts = { kind: 'file', from: 'B', to: 'A', scenario: 'P00-prepaired-upgrade-old-pairing' }
const accepted = { total_accepted: 1, per_target: [{ outcome: { kind: 'accepted' } }] }

test('legacy receive records retain fixed branch evidence from both JSON layouts', () => {
  assert.equal(receiveFailureKind({ fields: { error_kind: 'inbound_clipboard_apply_failed' } }), 'apply_failed')
  for (const row of [
    { message: 'inbound: blob materialize stopped' },
    { fields: { message: 'inbound: blob materialize stopped' } },
  ]) assert.equal(receiveFailureKind(row), 'blob_materialize_stopped')
})

test('receive evidence never derives a branch from raw errors or partial message matches', () => {
  for (const row of [
    { fields: { error: 'inbound: blob materialize stopped: sensitive-path' } },
    { message: 'inbound: blob materialize stopped sensitive-content' },
    { fields: { error_kind: 'inbound_clipboard_apply_failed_sensitive-token' } },
    { message: { payload: 'inbound: blob materialize stopped' } },
    null, undefined, {},
  ]) assert.equal(receiveFailureKind(row), undefined)
})

test('receive branch evidence cannot export identities, payloads, timestamps or error bodies', () => {
  const kind = receiveFailureKind({
    timestamp: 'sensitive-timestamp', device: 'sensitive-device', path: 'sensitive-path',
    fields: { error_kind: 'inbound_clipboard_apply_failed', error: 'sensitive-error', payload: 'sensitive-content' },
  })
  assert.equal(kind, 'apply_failed')
  assert(!JSON.stringify(kind).includes('sensitive'))
})

test('recognized failures retain only their fixed category, including legacy file rejection', () => {
  for (const [message, category] of [
    ['peer rejected', 'peer_rejected'], ['stream io', 'stream_io'], ['internal', 'internal'],
    ['local policy rejected payload before dispatch', 'local_policy_rejected'],
    ['peer version is incompatible with confirmed clipboard delivery', 'incompatible_version'],
    ['target device offline or unreachable', 'unreachable'],
  ]) {
    const result = summarizeDeliveryReport({
      total_accepted: 0, total_errored: 1, entry_id: 'sensitive-entry',
      per_target: [{ device_id: 'sensitive-device', outcome: { kind: 'error', message: `${message}: sensitive-content/path/token` } }],
    })
    assert.deepEqual(result, { total_accepted: 0, total_errored: 1, causes: [category] })
    assert(!JSON.stringify(result).includes('sensitive'))
  }
})

test('unknown messages, kinds and invalid counts cannot enter evidence', () => {
  assert.deepEqual(summarizeDeliveryReport({
    total_accepted: 'sensitive-content', total_offline: -1, total_errored: Infinity,
    total_pending: 0.5, total_duplicate: { path: 'sensitive-path' },
    per_target: [
      { outcome: { kind: 'sensitive-kind', message: 'sensitive-error' } },
      { outcome: { kind: 'error', message: { path: 'sensitive-path' } } },
      { outcome: { kind: 'duplicate' } }, {},
    ],
  }), { causes: ['unknown', 'error', 'duplicate', 'unknown'] })
})

test('rejected file dispatch keeps direction and category and never starts readback', async () => {
  const evidence = []
  await assert.rejects(verifyDelivery(evidence, facts, async () => ({
    total_accepted: 0, total_errored: 1,
    per_target: [{ outcome: { kind: 'error', message: 'peer rejected: sensitive-detail' } }],
  }), async () => assert.fail('rejected delivery must not read back')), /B to A: file not accepted .*causes=peer_rejected/)
  assert.equal(evidence[0].stage, 'dispatch')
  assert.equal(evidence[0].failed, true)
  assert.equal(evidence[0].scenario, facts.scenario)
  assert(evidence[0].dispatch_ms >= 0)
  assert(!JSON.stringify(evidence).includes('sensitive'))
})

test('dispatch exceptions retain their original identity without copying their text', async () => {
  const evidence = []
  const failure = new Error('sensitive host error')
  await assert.rejects(verifyDelivery(evidence, facts, async () => { throw failure }, async () => {}), error => error === failure)
  assert.equal(evidence[0].stage, 'dispatch')
  assert.equal(evidence[0].failed, true)
  assert(!JSON.stringify(evidence).includes('sensitive'))
})

test('accepted dispatch with failed exact readback remains a readback failure', async () => {
  const evidence = []
  const failure = new Error('exact file bytes did not arrive')
  await assert.rejects(verifyDelivery(evidence, facts, async () => accepted, async () => { throw failure }), error => error === failure)
  assert.equal(evidence[0].total_accepted, 1)
  assert.equal(evidence[0].stage, 'readback')
  assert.equal(evidence[0].failed, true)
  assert(evidence[0].readback_ms >= 0)
})

test('only actual exact readback completes a delivery and preserves action order', async () => {
  const evidence = [], actions = []
  await verifyDelivery(evidence, facts, async () => { actions.push('send'); return accepted }, async () => {
    actions.push('readback')
    assert.equal(evidence[0].stage, 'readback')
  })
  assert.deepEqual(actions, ['send', 'readback'])
  assert.equal(evidence[0].stage, 'verified')
  assert.equal(evidence[0].failed, undefined)
  assert(evidence[0].dispatch_ms >= 0 && evidence[0].readback_ms >= 0)
})
