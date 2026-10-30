import assert from 'node:assert/strict'
import { EventEmitter, once } from 'node:events'
import { PassThrough } from 'node:stream'
import { spawn } from 'node:child_process'
import test from 'node:test'
import { connectHostCommands } from './connection-recovery-host.mjs'

function fixture() {
  return Object.assign(new EventEmitter(), {
    stdin: new PassThrough(), stdout: new PassThrough(),
    exitCode: null, signalCode: null, kill() {},
  })
}

test('EPIPE rejects pending requests and subsequent cleanup without an unhandled error', async () => {
  const child = fixture()
  const channel = connectHostCommands(child)
  const first = channel.raw({ command: 'shutdown' })
  const second = channel.raw({ command: 'flush' })
  const pipeError = Object.assign(new Error('write EPIPE'), { code: 'EPIPE' })
  const check = error => error.message === 'test host input failed' && error.cause === pipeError
  const settled = [assert.rejects(first, check), assert.rejects(second, check)]
  child.stdin.destroy(pipeError)
  await Promise.all(settled)
  assert.equal(channel.pending.length, 0)
  await assert.rejects(channel.raw({ command: 'flush' }), check)
})

test('a signalled process cannot receive cleanup requests before its exit event', async () => {
  const child = fixture()
  const channel = connectHostCommands(child)
  child.signalCode = 'SIGTERM'
  await assert.rejects(channel.raw({ command: 'flush' }), /not running/)
  assert.equal(child.stdin.readableLength, 0)
})

test('spawn failure settles the queued request and clears its timer', async () => {
  const child = fixture()
  const channel = connectHostCommands(child)
  const rejected = assert.rejects(channel.raw({}), /process failed/)
  child.emit('error', new Error('spawn failed'))
  await rejected
  assert.equal(channel.pending.length, 0)
})

test('timeout settles every pending request and stops the host once', async () => {
  const child = fixture()
  let killed = 0
  child.kill = signal => { assert.equal(signal, 'SIGTERM'); killed++ }
  const channel = connectHostCommands(child, 10)
  await Promise.all([assert.rejects(channel.raw({}), /timed out/), assert.rejects(channel.raw({}), /timed out/)])
  assert.equal(channel.pending.length, 0)
  assert.equal(killed, 1)
})

test('a real child preserves reply order and settles an unanswered request on exit', async () => {
  const child = spawn(process.execPath, ['--input-type=module', '-e', `
    import { createInterface } from 'node:readline'
    createInterface({ input: process.stdin }).on('line', line => {
      const request = JSON.parse(line)
      if (request.command === 'exit') process.exit(0)
      process.stdout.write('diagnostic\\n' + JSON.stringify({uc_connectivity:1, ok:request.command}) + '\\n')
    })
  `], { stdio: ['pipe', 'pipe', 'ignore'] })
  const channel = connectHostCommands(child)
  try {
    const replies = await Promise.all([channel.raw({ command: 'one' }), channel.raw({ command: 'two' })])
    assert.deepEqual(replies.map(reply => reply.ok), ['one', 'two'])
    await assert.rejects(channel.raw({ command: 'exit' }), /exited before replying/)
    assert.equal(channel.pending.length, 0)
  } finally {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGKILL')
      await once(child, 'exit')
    }
  }
})
