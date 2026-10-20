import assert from 'node:assert/strict'
import test from 'node:test'
import { ensureSessionReady } from './connection-recovery-session.mjs'

test('a restored session is used without unlocking it again', async () => {
  const host = { async call(command) {
    assert.equal(command, 'observe', 'an already restored old host rejects redundant unlock')
    return { encryption: { session_ready: true } }
  } }
  assert.equal(await ensureSessionReady(host), true)
})

test('a locked session must unlock successfully and report ready', async () => {
  const commands = []
  const host = { async call(command) {
    commands.push(command)
    return { encryption: { session_ready: commands.length === 3 } }
  } }
  assert.equal(await ensureSessionReady(host), false)
  assert.deepEqual(commands, ['observe', 'unlock', 'observe'])
})

test('unlock errors, missing state and a still locked session remain failures', async () => {
  const failure = new Error('old host unlock failed (code 1213)')
  await assert.rejects(ensureSessionReady({ async call(command) {
    if (command === 'unlock') throw failure
    return { encryption: { session_ready: false } }
  } }), error => error === failure)
  await assert.rejects(ensureSessionReady({ async call() { return {} } }), /session readiness query failed/)
  await assert.rejects(ensureSessionReady({ async call() { return { encryption: { session_ready: false } } } }), /session did not become ready/)
})
