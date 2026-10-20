import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import test from 'node:test'
import { ONLINE_DEADLINE_MESSAGE, measureRecoveryMs, relayTransportReadyAt, terminateRelay } from './relay-recovery-support.mjs'

const ready = child => new Promise(resolve => child.stdout.once('data', resolve))
const fake = script => spawn(process.execPath, ['-e', script], { stdio: ['ignore', 'pipe', 'ignore'] })

test('a TCP probe or incomplete Relay handshake does not start the recovery budget', () => {
  const status = (at, count) => ({ timestamp: new Date(at).toISOString(), fields: { 'event.name': 'relay.status.observed', connected_count: count } })
  assert.equal(relayTransportReadyAt(true, [status(1000, 0)], 1000), undefined)
  assert.equal(relayTransportReadyAt(true, [status(999, 1)], 1000), undefined)
  assert.equal(relayTransportReadyAt(false, [status(1100, 1)], 1000), undefined)
  assert.equal(relayTransportReadyAt(true, [status(1100, 1), status(1200, 0)], 1000), undefined)
  assert.equal(relayTransportReadyAt(true, [status(1000, 0), status(1100, 1)], 1000), 1100)
})

test('a recovery measurement records null only for the expected deadline failure', async () => {
  assert.equal(await measureRecoveryMs(async () => {}, 100, () => 350), 250)
  assert.equal(await measureRecoveryMs(async () => { throw new Error(ONLINE_DEADLINE_MESSAGE) }, 0), null)
})

test('a recovery measurement propagates every other failure instead of recording null', async () => {
  for (const message of ['test host is not running', 'test host exited before replying', 'validation interrupted']) {
    await assert.rejects(measureRecoveryMs(async () => { throw new Error(message) }, 0), { message })
  }
})

test('a relay that already exited is reported at once instead of waiting for an exit event that never comes', async () => {
  const relay = fake('process.exit(3)')
  await once(relay, 'exit')
  const started = Date.now()
  await assert.rejects(terminateRelay(relay), /exited before shutdown \(exit code 3/)
  assert.ok(Date.now() - started < 1000)
})

test('a relay that shuts down on SIGINT is stopped cleanly', async () => {
  const relay = fake("process.on('SIGINT',()=>process.exit(0));console.log('ready');setInterval(()=>{},1000)")
  await ready(relay)
  await terminateRelay(relay)
  assert.equal(relay.exitCode, 0)
})

test('a relay that ignores SIGINT is killed after the bounded wait and reported', async () => {
  const relay = fake("process.on('SIGINT',()=>{});console.log('ready');setInterval(()=>{},1000)")
  await ready(relay)
  await assert.rejects(terminateRelay(relay, { timeoutMs: 300 }), /did not shut down within 300 ms/)
  assert.equal(relay.signalCode, 'SIGKILL')
})
