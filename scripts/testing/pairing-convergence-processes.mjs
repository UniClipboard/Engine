import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { mkdirSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { resolve, join } from 'node:path'
import { createInterface } from 'node:readline'
import { setTimeout as delay } from 'node:timers/promises'

const options = new Map()
for (let index = 2; index < process.argv.length; index += 2) {
  options.set(process.argv[index], process.argv[index + 1])
}
const binary = resolve(options.get('--host') ?? 'target/debug/uc-connectivity-host')
const evidence = resolve(options.get('--evidence') ?? 'target/pairing-convergence-processes')
mkdirSync(evidence, { recursive: true, mode: 0o700 })

let rendezvous
let failed = false
const results = []

async function handleRendezvous(request, response) {
  let body = ''
  for await (const chunk of request) {
    body += chunk
    if (body.length > 1_048_576) {
      response.writeHead(413).end()
      return
    }
  }
  const value = body ? JSON.parse(body) : {}
  const result = request.url === '/v1/pairings'
    ? { code: value.sponsorTicket, expiresAtMs: 2_000_000_000_000 }
    : { sponsorTicket: value.code, sponsorEndpointId: 'local-test', expiresAtMs: 2_000_000_000_000 }
  response.writeHead(request.url?.endsWith('/consume') ? 204 : 200, { 'content-type': 'application/json' })
  response.end(JSON.stringify(result))
}

class Host {
  constructor(name, root, rendezvousUrl) {
    this.name = name
    this.root = root
    this.rendezvousUrl = rendezvousUrl
    this.pending = []
    this.secureStorage = undefined
    this.child = undefined
    mkdirSync(root, { recursive: true, mode: 0o700 })
  }

  async start() {
    assert(!this.child, `${this.name} is already running`)
    this.child = spawn(binary, { stdio: ['pipe', 'pipe', 'pipe'] })
    this.child.stderr.pipe(process.stderr)
    createInterface({ input: this.child.stdout }).on('line', line => {
      let response
      try { response = JSON.parse(line) } catch { return }
      if (response.uc_connectivity !== 1) return
      const pending = this.pending.shift()
      if (!pending) return
      clearTimeout(pending.timer)
      pending.resolve(response)
    })
    this.child.on('exit', () => {
      for (const pending of this.pending.splice(0)) {
        clearTimeout(pending.timer)
        pending.reject(new Error(`${this.name} exited before replying`))
      }
    })
    const ready = await this.raw({
      root: this.root,
      rendezvous: this.rendezvousUrl,
      secure_storage: this.secureStorage,
    })
    assert.equal(ready.ready, true)
  }

  raw(request) {
    assert(this.child?.exitCode === null && this.child.signalCode === null, `${this.name} is not running`)
    return new Promise((resolveRequest, reject) => {
      const timer = setTimeout(() => reject(new Error(`${this.name} command timed out`)), 130_000)
      this.pending.push({ resolve: resolveRequest, reject, timer })
      this.child.stdin.write(`${JSON.stringify(request)}\n`)
    })
  }

  async call(command, fields = {}) {
    const response = await this.raw({ command, ...fields })
    assert(!response.error, `${this.name} ${command} failed (${response.code ?? 'no code'})`)
    return response.ok
  }

  async crash() {
    this.secureStorage = await this.call('secure_storage')
    const child = this.child
    child.kill('SIGKILL')
    await once(child, 'exit')
    this.child = undefined
  }

  async stop() {
    if (!this.child) return
    this.secureStorage = await this.call('secure_storage')
    const child = this.child
    await this.call('shutdown')
    if (child.exitCode === null) await once(child, 'exit')
    assert.equal(child.exitCode, 0)
    this.child = undefined
  }
}

async function until(predicate, description) {
  const deadline = performance.now() + 120_000
  while (true) {
    if (await predicate()) return
    assert(performance.now() <= deadline, description)
    await delay(50)
  }
}

async function completed(host, space) {
  const response = await host.raw({ command: 'setup' })
  return response.ok?.has_completed && response.ok?.space_id === space
}

async function scenario(name, action) {
  const startedAt = new Date().toISOString()
  const started = performance.now()
  const record = { name, started_at: startedAt, outcome: 'failed' }
  results.push(record)
  try {
    await action()
    record.outcome = 'passed'
  } finally {
    record.elapsed_ms = Math.round(performance.now() - started)
    process.stdout.write(`${name}: ${record.outcome} (${record.elapsed_ms} ms)\n`)
  }
}

async function freshPair(name) {
  const root = join(evidence, name)
  const url = `http://127.0.0.1:${rendezvous.address().port}`
  const sponsor = new Host(`${name}-sponsor`, join(root, 'sponsor'), url)
  const joiner = new Host(`${name}-joiner`, join(root, 'joiner'), url)
  await sponsor.start()
  await joiner.start()
  return { sponsor, joiner }
}

async function main() {
  rendezvous = createServer((request, response) => {
    handleRendezvous(request, response).catch(error => {
      failed = true
      process.stderr.write(`rendezvous failed: ${error.message}\n`)
      response.destroy()
    })
  })
  rendezvous.listen(0, '127.0.0.1')
  await once(rendezvous, 'listening')

  await scenario('joiner_restarts_before_sponsor_is_reachable', async () => {
    const { sponsor, joiner } = await freshPair('joiner-early-exit')
    try {
      const created = await sponsor.call('create', { name: 'Sponsor' })
      const invitation = await sponsor.call('invite')
      await sponsor.stop()
      const pending = await joiner.call('join', { invitation: invitation.invitation, name: 'Joiner' })
      assert.equal(pending.status, 'pending')
      await joiner.crash()
      await sponsor.start()
      await joiner.start()
      await until(() => completed(joiner, created.space), 'joiner did not resume the saved attempt')
    } finally {
      await sponsor.stop()
      await joiner.stop()
    }
  })

  await scenario('sponsor_restarts_after_final_confirmation_connection_failure', async () => {
    const { sponsor, joiner } = await freshPair('sponsor-final-exit')
    try {
      const created = await sponsor.call('create', { name: 'Sponsor' })
      const invitation = await sponsor.call('invite')
      const armed = await joiner.call('arm_complete_ack_failure')
      const pending = await joiner.call('join', { invitation: invitation.invitation, name: 'Joiner' })
      assert.equal(pending.status, 'pending')
      await joiner.call('wait_space_work_event', {
        after_sequence: armed.after_sequence,
        kind: 'final_confirmation_connection_failed',
      })
      const beforeRestart = await joiner.call('space_work_events')
      assert(!beforeRestart.some(event =>
        ['ordinary_member_update_started', 'membership_history_sync_started'].includes(event.kind)
          && event.sequence > armed.after_sequence))
      await sponsor.crash()
      await sponsor.start()
      await until(() => completed(joiner, created.space), 'pairing did not converge after sponsor restart')
    } finally {
      await sponsor.stop()
      await joiner.stop()
    }
  })

  await scenario('joiner_restarts_after_final_confirmation_connection_failure', async () => {
    const { sponsor, joiner } = await freshPair('joiner-final-exit')
    try {
      const created = await sponsor.call('create', { name: 'Sponsor' })
      const invitation = await sponsor.call('invite')
      const armed = await joiner.call('arm_complete_ack_failure')
      const pending = await joiner.call('join', { invitation: invitation.invitation, name: 'Joiner' })
      assert.equal(pending.status, 'pending')
      await joiner.call('wait_space_work_event', {
        after_sequence: armed.after_sequence,
        kind: 'final_confirmation_connection_failed',
      })
      await joiner.crash()
      await joiner.start()
      await until(() => completed(joiner, created.space), 'saved final confirmation did not resume')
    } finally {
      await sponsor.stop()
      await joiner.stop()
    }
  })
}

try {
  await main()
} catch (error) {
  failed = true
  process.stderr.write(`pairing convergence validation failed: ${error.message}\n`)
} finally {
  rendezvous?.close()
  writeFileSync(join(evidence, 'result.json'), JSON.stringify({ failed, results }, null, 2), { mode: 0o600 })
}

if (failed) process.exitCode = 1
