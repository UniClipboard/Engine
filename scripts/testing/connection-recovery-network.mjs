import assert from 'node:assert/strict'
import { spawn, execFileSync } from 'node:child_process'
import { once } from 'node:events'
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readdirSync, readFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { createInterface } from 'node:readline'
import { setTimeout as delay } from 'node:timers/promises'
import { createHash } from 'node:crypto'

const options = new Map()
for (let i = 2; i < process.argv.length; i += 2) options.set(process.argv[i], process.argv[i + 1])
const binary = resolve(options.get('--host') ?? 'target/debug/uc-connectivity-host')
const only = options.get('--case')
const mode = options.get('--mode') ?? 'direct'
assert(['direct', 'known-peer', 'legacy', 'relay'].includes(mode))
const legacySide = Number(options.get('--legacy-side') ?? 1)
const relayBinary = options.get('--relay')
const legacyBinary = options.get('--legacy-host')
let relay
const repeat = Number(options.get('--repeat') ?? 3)
assert(Number.isInteger(repeat) && repeat > 0)
const evidence = resolve(options.get('--evidence') ?? 'target/connection-recovery-evidence')
mkdirSync(evidence, { recursive: true, mode: 0o700 })
const runId = `ucr${process.pid}`
const nodes = []
const namespaces = []
const bridge = `${runId}br`.slice(0, 15)
const root = mkdtempSync(join(tmpdir(), 'uc-connectivity-'))
const records = []
const faults = []
let server
let interrupted = false

function command(program, args, input) {
  try { return execFileSync(program, args, { input, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'] }) }
  catch { throw new Error(`${program} test environment command failed`) }
}
function ip(...args) { return command('ip', args) }
function net(node, ...args) { return ip('netns', 'exec', node.namespace, ...args) }

function nft(node, ...args) {
  try {
    return execFileSync('ip', ['netns', 'exec', node.namespace, 'nft', ...args], {
      encoding: 'utf8',
      stdio: ['pipe', 'pipe', 'pipe'],
    })
  } catch (error) {
    const detail = typeof error.stderr === 'string'
      ? error.stderr.trim().split('\n')[0].replaceAll(runId, '<run>')
      : ''
    throw new Error(`nft test environment command failed${detail ? `: ${detail}` : ''}`)
  }
}

function processResources(node) {
  const tasks = `/proc/${node.child.pid}/task`
  const names = readdirSync(tasks).flatMap(id => {
    try { return [readFileSync(join(tasks, id, 'comm'), 'utf8').trim()] }
    catch (error) { if (error.code === 'ENOENT') return []; throw error }
  })
  return { system_threads: names.length, blob_store_threads: names.filter(name => name === 'iroh-blob-store').length }
}

class Host {
  constructor(index) {
    this.index = index
    this.label = String.fromCharCode(65 + index)
    this.namespace = `${runId}${this.label}`
    this.root = join(root, this.label)
    this.pending = []
    this.events = []
    this.timeline = []
    this.recoveries = 0
    this.resources = []
    this.secureStorage = undefined
    this.child = undefined
    this.bindPort = mode === 'known-peer' ? 21_000 + index : undefined
    this.commands = []
  }
  async start() {
    const selected = mode === 'legacy' && this.label === String.fromCharCode(65 + legacySide) ? resolve(legacyBinary) : binary
    this.child = spawn('ip', ['netns', 'exec', this.namespace, selected], { stdio: ['pipe', 'pipe', 'pipe'] })
    this.child.stderr.resume()
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
        pending.reject(new Error('test host exited before replying'))
      }
    })
    const ready = await this.raw({ root: this.root, rendezvous: `http://10.233.0.1:${server.address().port}`, secure_storage: this.secureStorage, relay: mode === 'relay', bind_port: this.bindPort })
    assert.equal(ready.ready, true)
    this.version = ready.version
  }
  raw(request) {
    if (interrupted || !this.child || this.child.exitCode !== null || this.child.signalCode !== null) return Promise.reject(new Error('test host is not running'))
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.child.kill('SIGTERM'); reject(new Error('test host command timed out')) }, 130_000)
      this.pending.push({ resolve, reject, timer })
      this.child.stdin.write(`${JSON.stringify(request)}\n`)
    })
  }
  async call(command, fields = {}) {
    this.commands.push(command)
    const result = await this.raw({ command, ...fields })
    assert(!result.error, `${this.label} ${command} failed (code ${result.code ?? 'unavailable'})`)
    return result.ok
  }
  async drain() {
    const events = await this.call('events')
    for (const event of events) {
      assert.notEqual(event.kind, 'lost_events', 'state event evidence was lost')
      assert.notEqual(event.kind, 'host_failed', 'Engine lifecycle failed')
      const sanitized = { at_ms: Math.round(performance.now()), kind: event.kind, peer: nodes.find(node => node.id === event.peer)?.label, state: event.state }
      assert(this.timeline.length < 20000, 'event evidence capacity exceeded')
      this.timeline.push(sanitized)
      this.events.push(sanitized)
      if (event.kind === 'recovery') this.recoveries++
    }
    return events
  }
  async stop() {
    if (!this.child || this.child.exitCode !== null) return
    this.secureStorage = await this.call('secure_storage')
    await this.call('shutdown')
    if (this.child.exitCode === null) await once(this.child, 'exit')
    assert.equal(this.child.exitCode, 0)
  }
  async reset() {
    await this.stop()
    this.secureStorage = undefined
    this.id = undefined
    this.events = []
    this.commands = []
    this.partitionedAt = undefined
    this.bindPort = 21_000 + this.index
    rmSync(this.root, { recursive: true, force: true })
    await this.start()
  }
}

async function until(predicate, milliseconds, description) {
  const deadline = performance.now() + milliseconds
  while (true) {
    assert(!interrupted, 'validation interrupted')
    const completed = await predicate()
    assert(performance.now() <= deadline, description)
    if (completed) return
    await delay(50)
  }
}

async function paired(group) {
  const created = await group[0].call('create', { name: group[0].label })
  group[0].id = created.device
  for (const node of group.slice(1)) {
    const invitation = await group[0].call('invite')
    await node.call('join', { invitation: invitation.invitation, name: node.label })
    await until(async () => {
      const response = await node.raw({ command: 'setup' })
      return response.ok?.has_completed && response.ok?.space_id === created.space
    }, 120_000, 'pairing did not finish')
    await until(async () => {
      const peers = await group[0].call('peers')
      node.id = peers.find(peer => peer.device_name === node.label)?.peer_id
      return Boolean(node.id)
    }, 120_000, 'paired identity unavailable')
  }
  await usable(group)
  await online(group, 20_000)
  return created
}

async function usable(group) {
  for (const node of group) {
    await until(async () => {
      const response = await node.raw({ command: 'eligibility' })
      if (response.error && response.code === 1211) return false
      assert(!response.error, 'communication eligibility query failed')
      const choices = response.ok
      return group.filter(peer => peer !== node).every(peer => choices.device_trust.devices.some(device => device.device_id === peer.id && device.sync_relationship === 'usable'))
    }, 120_000, 'ordinary communication eligibility did not converge')
  }
}

async function offline(isolated, budget) {
  await until(async () => {
    const results = await Promise.all(nodes.map(async node => {
      const peers = await node.call('peers')
      await node.drain()
      const expected = node === isolated ? nodes.filter(peer => peer !== isolated) : [isolated]
      return expected.every(peer => peers.some(row => row.peer_id === peer.id && !row.connected))
    }))
    return results.every(Boolean)
  }, budget, 'silent disconnection exceeded its deadline')
}

async function online(group, budget) {
  await until(async () => {
    for (const node of group) {
      const response = await node.raw({ command: 'peers' })
      if (response.error) return false
      const peers = response.ok
      await node.drain()
      if (!group.filter(peer => peer !== node).every(peer => peers.some(row => row.peer_id === peer.id && row.connected))) return false
    }
    return true
  }, budget, 'automatic connection exceeded its deadline')
}

async function transfer(left, right, marker) {
  for (const [sender, receiver] of [[left, right], [right, left]]) {
    const text = `synthetic-${marker}-${sender.label}`
    const result = await sender.call('send', { peer: receiver.id, text })
    const causes = (result.per_target ?? []).map(target => {
      const message = target.outcome?.message ?? ''
      return ['peer rejected', 'stream io', 'internal', 'local policy rejected payload before dispatch', 'peer version is incompatible with confirmed clipboard delivery', 'target device offline or unreachable']
        .find(prefix => message.startsWith(prefix)) ?? target.outcome?.kind ?? 'unknown'
    })
    assert.equal(result.total_accepted, 1, `${sender.label} to ${receiver.label}: content not accepted (offline=${result.total_offline}, failed=${result.total_errored}, pending=${result.total_pending}, duplicate=${result.total_duplicate}, causes=${causes.join(',')})`)
    await until(async () => {
      for (const entry of await receiver.call('history')) {
        if ((await receiver.call('entry', { entry: entry.entry_id })).content === text) return true
      }
      return false
    }, 20_000, 'exact content did not arrive')
  }
}

function partition(node, blocked) {
  if (blocked && node.partitionedAt) return node.partitionedAt
  if (!blocked) {
    net(node, 'nft', 'delete', 'table', 'inet', 'uc_liveness')
    node.partitionedAt = undefined
    const at = performance.now()
    faults.push({ node: node.label, action: 'heal', at_ms: Math.round(at) })
    return at
  }
  nft(node, 'add', 'table', 'inet', 'uc_liveness')
  nft(node, 'add', 'chain', 'inet', 'uc_liveness', 'input', '{ type filter hook input priority -100; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_liveness', 'input', 'counter', 'drop')
  nft(node, 'add', 'chain', 'inet', 'uc_liveness', 'output', '{ type filter hook output priority -100; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_liveness', 'output', 'counter', 'drop')
  const activated = performance.now()
  node.partitionedAt = activated
  // An independent probe and packet counters prove the fault, independently of Engine state.
  try { execFileSync('ip', ['netns', 'exec', node.namespace, 'ping', '-c', '1', '-W', '1', '10.233.0.1'], { stdio: 'pipe' }); assert.fail('drop rule did not block probe') }
  catch (error) { if (error.code === 'ERR_ASSERTION') throw error }
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'uc_liveness'))
  assert(rules.nftables.some(row => row.rule?.expr?.some(expr => expr.counter?.packets > 0)), 'drop counters remained empty')
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
  faults.push({ node: node.label, action: 'drop', at_ms: Math.round(activated), verified_dropped_packets: dropped })
  return activated
}

function blockDiscovery(node) {
  nft(node, 'add', 'table', 'inet', 'uc_discovery')
  nft(node, 'add', 'chain', 'inet', 'uc_discovery', 'input', '{ type filter hook input priority -75; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_discovery', 'input', 'udp', 'dport', '5353', 'counter', 'drop')
  nft(node, 'add', 'chain', 'inet', 'uc_discovery', 'output', '{ type filter hook output priority -75; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_discovery', 'output', 'udp', 'dport', '5353', 'counter', 'drop')
  try {
    net(node, 'node', '-e', "const d=require('dgram');const s=d.createSocket('udp4');s.send('probe',5353,'10.233.0.1',()=>s.close())")
  } catch {}
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'uc_discovery'))
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
  assert(dropped > 0, 'discovery drop rules were not exercised')
  faults.push({ node: node.label, action: 'discovery_blocked', at_ms: Math.round(performance.now()), verified_dropped_packets: dropped, public_discovery_disabled: true })
}

function unblockDiscovery(node) {
  net(node, 'nft', 'delete', 'table', 'inet', 'uc_discovery')
  faults.push({ node: node.label, action: 'discovery_restored', at_ms: Math.round(performance.now()) })
}

function udpPortBound(node, port) {
  return net(node, 'ss', '-H', '-l', '-u', '-n', `sport = :${port}`).trim().length > 0
}

function blockPeerPair(left, right) {
  for (const [node, peer] of [[left, right], [right, left]]) {
    const peerIp = `10.233.0.${peer.index + 11}`
    nft(node, 'add', 'table', 'inet', 'uc_pair')
    nft(node, 'add', 'chain', 'inet', 'uc_pair', 'input', '{ type filter hook input priority -80; policy accept; }')
    nft(node, 'add', 'rule', 'inet', 'uc_pair', 'input', 'ip', 'saddr', peerIp, 'counter', 'drop')
    nft(node, 'add', 'chain', 'inet', 'uc_pair', 'output', '{ type filter hook output priority -80; policy accept; }')
    nft(node, 'add', 'rule', 'inet', 'uc_pair', 'output', 'ip', 'daddr', peerIp, 'counter', 'drop')
  }
  try { net(left, 'ping', '-c', '1', '-W', '1', `10.233.0.${right.index + 11}`); assert.fail('peer isolation did not block the probe') }
  catch (error) { if (error.code === 'ERR_ASSERTION') throw error }
  const rules = JSON.parse(net(left, 'nft', '-j', 'list', 'table', 'inet', 'uc_pair'))
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
  assert(dropped > 0, 'peer isolation counters remained empty')
  faults.push({ node: `${left.label}-${right.label}`, action: 'peer_pair_blocked', at_ms: Math.round(performance.now()), verified_dropped_packets: dropped })
}

function unblockPeerPair(left, right) {
  for (const node of [left, right]) net(node, 'nft', 'delete', 'table', 'inet', 'uc_pair')
  faults.push({ node: `${left.label}-${right.label}`, action: 'peer_pair_restored', at_ms: Math.round(performance.now()) })
}

function blockOutboundInitiation(node, peer) {
  const peerIp = `10.233.0.${peer.index + 11}`
  nft(node, 'add', 'table', 'inet', 'uc_initiator')
  nft(node, 'add', 'chain', 'inet', 'uc_initiator', 'output', '{ type filter hook output priority -90; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_initiator', 'output', 'ip', 'daddr', peerIp, 'ct', 'state', 'new', 'counter', 'drop')
  try { net(node, 'ping', '-c', '1', '-W', '1', peerIp); assert.fail('outbound initiation rule did not block the probe') }
  catch (error) { if (error.code === 'ERR_ASSERTION') throw error }
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'uc_initiator'))
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
  assert(dropped > 0, 'outbound initiation counters remained empty')
  faults.push({ node: `${node.label}->${peer.label}`, action: 'outbound_initiation_blocked', at_ms: Math.round(performance.now()), verified_dropped_packets: dropped })
}

function unblockOutboundInitiation(node, peer) {
  net(node, 'nft', 'delete', 'table', 'inet', 'uc_initiator')
  faults.push({ node: `${node.label}->${peer.label}`, action: 'outbound_initiation_restored', at_ms: Math.round(performance.now()) })
}

async function scenario(id, action) {
  if (only && !id.startsWith(only)) return
  const started = new Date().toISOString()
  const clock = performance.now()
  const record = { id, started, outcome: 'failed' }
  records.push(record)
  try { await action(); record.outcome = 'passed' }
  finally { record.elapsed_ms = Math.round(performance.now() - clock); record.completed = new Date().toISOString() }
  process.stdout.write(`${id}: passed (${record.elapsed_ms} ms)\n`)
}

async function handleRendezvousRequest(request, response) {
  let body = ''
  for await (const chunk of request) {
    body += chunk
    if (body.length > 1_048_576) { response.writeHead(413).end(); return }
  }
  const value = body ? JSON.parse(body) : {}
  const result = request.url === '/v1/pairings' ? { code: value.sponsorTicket, expiresAtMs: 2_000_000_000_000 }
    : { sponsorTicket: value.code, sponsorEndpointId: 'local-test', expiresAtMs: 2_000_000_000_000 }
  response.writeHead(request.url?.endsWith('/consume') ? 204 : 200, { 'content-type': 'application/json' })
  response.end(JSON.stringify(result))
}

async function knownPeerRecoveryScenarios(a, b, c) {
  for (let iteration = 0; iteration < repeat; iteration++) {
    if (iteration > 0) {
      for (const node of nodes) await node.reset()
    }
    const created = await paired([a, b])
    await transfer(a, b, `known-peer-baseline-${iteration}`)

    blockPeerPair(a, c)
    const invitation = await b.call('invite')
    await c.call('join', { invitation: invitation.invitation, name: c.label })
    await until(async () => {
      const response = await c.raw({ command: 'setup' })
      return response.ok?.has_completed && response.ok?.space_id === created.space
    }, 120_000, 'third member pairing did not finish')
    await until(async () => {
      const peers = await b.call('peers')
      c.id = peers.find(peer => peer.device_name === c.label)?.peer_id
      return Boolean(c.id)
    }, 120_000, 'third member identity unavailable')
    await usable([b, c])
    await until(async () => {
      const response = await a.raw({ command: 'eligibility' })
      if (response.error && response.code === 1211) return false
      assert(!response.error, 'membership lag precondition query failed')
      const target = response.ok.device_trust.devices.find(device => device.device_id === c.id)
      return Boolean(target && target.sync_relationship !== 'usable')
    }, 120_000, 'isolated new member did not become a known pending peer')

    const oldPort = c.bindPort
    await c.stop()
    await until(async () => (await b.call('peers')).some(peer => peer.peer_id === c.id && !peer.connected), 20_000, 'third member did not disconnect before the directed recovery')
    blockOutboundInitiation(a, c)
    unblockPeerPair(a, c)

    blockDiscovery(a)
    let discoveryBlocked = true
    let outboundInitiationBlocked = true
    try {
      c.bindPort = 22_000 + iteration
      assert(!udpPortBound(c, oldPort), 'the previous fixed UDP port is still listening')
      const commandBaselines = new Map(nodes.map(node => [node, node.commands.length]))
      const contactStarted = performance.now()
      await scenario(`E13-known-peer-contact-${iteration}`, async () => {
        await c.start()
        assert(udpPortBound(c, c.bindPort), 'the replacement fixed UDP port is not listening')
        assert(!udpPortBound(c, oldPort), 'the previous fixed UDP port became reachable again')
        const remaining = 20_000 - (performance.now() - contactStarted)
        assert(remaining > 0, 'host restart exhausted the automatic recovery budget')
        await online([c, a], remaining)
        const onlineAt = performance.now()
        assert(onlineAt - contactStarted <= 20_000, 'automatic known-peer recovery exceeded 20 seconds')
        const [cConnections, aConnections] = await Promise.all([
          c.call('connections'),
          a.call('connections'),
        ])
        assert(cConnections.outgoing > 0, 'the restarted device did not initiate the recovered connection')
        assert(aConnections.incoming > 0, 'the waiting device did not retain the inbound recovered connection')
        const forbidden = new Set(['opportunity', 'recover', 'send', 'suspend', 'resume'])
        for (const node of [c, a]) {
          assert(!node.commands.slice(commandBaselines.get(node)).some(command => forbidden.has(command)), 'the scenario used a forbidden recovery trigger before Online')
        }
        records.at(-1).proof = {
          old_port_closed: true,
          replacement_port_bound: true,
          discovery_blocked: true,
          public_discovery_disabled: true,
          initiator_outbound: true,
          receiver_inbound: true,
          automatic_online_within_ms: Math.round(onlineAt - contactStarted),
          forbidden_triggers_used: false,
        }
        await transfer(c, a, `known-peer-recovered-${iteration}`)
        records.at(-1).proof.bidirectional_transfer = true
      })
    } finally {
      if (discoveryBlocked) {
        unblockDiscovery(a)
        discoveryBlocked = false
      }
      if (outboundInitiationBlocked) {
        unblockOutboundInitiation(a, c)
        outboundInitiationBlocked = false
      }
    }
  }
}

async function run() {
  assert.equal(process.platform, 'linux', 'Linux network namespaces are required')
  command('nft', ['--version'])
  command('ip', ['-Version'])
  command('ping', ['-V'])
  command('file', ['--version'])
  ip('link', 'add', bridge, 'type', 'bridge')
  ip('addr', 'add', '10.233.0.1/24', 'dev', bridge)
  ip('link', 'set', bridge, 'up')
  server = createServer((request, response) => {
    handleRendezvousRequest(request, response).catch(() => {
      failed = true
      response.destroy()
    })
  })
  server.listen(0, '10.233.0.1')
  await once(server, 'listening')
  if (mode === 'relay') await startRelay()
  for (let index = 0; index < (mode === 'direct' || mode === 'known-peer' ? 3 : 2); index++) {
    const node = new Host(index)
    nodes.push(node)
    ip('netns', 'add', node.namespace)
    namespaces.push(node.namespace)
    const external = `${runId}v${index}`.slice(0, 15)
    ip('link', 'add', external, 'type', 'veth', 'peer', 'name', 'eth0', 'netns', node.namespace)
    ip('link', 'set', external, 'master', bridge)
    ip('link', 'set', external, 'up')
    net(node, 'ip', 'link', 'set', 'lo', 'up')
    net(node, 'ip', 'addr', 'add', `10.233.0.${index + 11}/24`, 'dev', 'eth0')
    net(node, 'ip', 'link', 'set', 'eth0', 'up')
    await node.start()
    if (mode === 'relay') {
      await node.call('relay_config', { url: 'http://10.233.0.1:19090' })
      await node.stop()
      await node.start()
    }
  }
  if (mode === 'known-peer') {
    const [a, b, c] = nodes
    await knownPeerRecoveryScenarios(a, b, c)
    for (const node of nodes) await node.stop()
    return
  }
  if (mode === 'relay') await until(async () => nodes.every(node => net(node, 'ss', '-Hnt', 'state', 'established').includes(':19090')), 20_000, 'test hosts did not connect to the configured relay')
  await paired(nodes)
  const [a, b, c] = nodes
  await transfer(a, b, 'baseline')
  if (c) await transfer(a, c, 'baseline')
  if (mode === 'legacy') { await legacyScenarios(a, b); for (const node of nodes) await node.stop(); return }
  if (mode === 'relay') { await relayScenarios(a, b); for (const node of nodes) await node.stop(); return }
  for (let iteration = 0; iteration < repeat; iteration++) {
    for (const node of nodes) { await node.drain(); node.events = [] }
    const recoveryBaseline = nodes.map(node => node.recoveries)
    await scenario(`E03-${iteration}`, async () => {
      const activated = partition(c, true)
      await until(async () => {
        const left = await a.call('peers')
        const right = await c.call('peers')
        return left.some(peer => peer.peer_id === c.id && !peer.connected) && [a, b].every(node => right.some(peer => peer.peer_id === node.id && !peer.connected))
      }, 20_000 - (performance.now() - activated), 'silent disconnection detection exceeded 20 seconds')
    })
    await scenario(`E05-${iteration}`, async () => {
      await online([a, b], 1000)
      await transfer(a, b, `isolated-C-${iteration}`)
      for (const node of [a, b]) {
        await node.drain()
        assert(!node.events.some(event => event.kind === 'peer' && event.peer !== 'C' && event.state !== 'online'), 'healthy pair was interrupted')
      }
    })
    await scenario(`E04-no-opportunity-${iteration}`, async () => {
      const activated = partition(c, true)
      await offline(c, 20_000 - Math.min(performance.now() - activated, 19_000))
      for (const node of nodes) await node.call('suppress_opportunities', { suppressed: true })
      partition(c, false)
      await online(nodes, 92_000)
      for (const node of nodes) await node.call('suppress_opportunities', { suppressed: false })
      await transfer(a, c, `healed-${iteration}`)
    })
    await scenario(`E04-opportunity-${iteration}`, async () => {
      const activated = partition(c, true)
      await offline(c, 20_000 - (performance.now() - activated))
      partition(c, false)
      for (const node of nodes) await node.call('opportunity')
      await online(nodes, 20_000)
      await transfer(a, c, `opportunity-${iteration}`)
    })
    await scenario(`E06-${iteration}`, async () => {
      const taskBaseline = await Promise.all(nodes.map(async node => (await node.call('connections')).registered_tasks))
      const processBaseline = nodes.map(processResources)
      assert(processBaseline.every(counts => counts.blob_store_threads > 0), 'blob store worker measurement is unavailable')
      for (let round = 0; round < 10; round++) {
        for (const node of nodes) { await node.drain(); node.events = [] }
        partition(c, true)
        partition(c, false)
        await delay(5500)
        await online(nodes, 1000)
        for (const node of nodes) {
          await node.drain()
          assert(!node.events.some(event => event.kind === 'peer' && event.state !== 'online'), 'short interruption caused offline')
        }
        const activated = partition(c, true)
        await offline(c, 20_000 - (performance.now() - activated))
        partition(c, false)
        for (const node of nodes) await node.call('opportunity')
        await online(nodes, 20_000)
        await transfer(a, c, `round-${iteration}-${round}`)
        for (const node of nodes) {
          const counts = await node.call('connections')
          assert(counts.incoming + counts.outgoing <= 3 * (nodes.length - 1), 'peer connections grew beyond the admission limit')
          assert(counts.incoming + counts.outgoing >= nodes.length - 1, 'online state has no retained connection')
          assert.equal(counts.registered_tasks, taskBaseline[nodes.indexOf(node)], 'registered Engine tasks grew during repeated recovery')
          const processCounts = processResources(node)
          assert.equal(processCounts.blob_store_threads, processBaseline[nodes.indexOf(node)].blob_store_threads, 'blob store runtimes grew during ordinary peer recovery')
          node.resources.push({ iteration, round, ...counts, ...processCounts })
        }
      }
    })
    await scenario(`E11-${iteration}`, async () => {
      await b.stop()
      await c.stop()
      await until(async () => (await a.call('peers')).every(peer => !peer.connected), 20_000, 'remote exit was not observed')
      await delay(1000)
      await b.start()
      await online([a, b], 92_000)
      await transfer(a, b, `restart-${iteration}`)
      await c.start()
      await online(nodes, 92_000)
    })
    for (const [index, node] of nodes.entries()) { await node.drain(); assert.equal(node.recoveries, recoveryBaseline[index], 'remote failure caused whole-session recovery') }
    await scenario(`E12-${iteration}`, async () => {
      await a.call('recover')
      await online(nodes, 20_000)
      await transfer(a, b, `explicit-${iteration}`)
      await a.call('suspend')
      await a.call('resume')
      await online(nodes, 20_000)
      await transfer(a, b, `resume-${iteration}`)
    })
  }
  for (const node of nodes) await node.stop()
}

async function legacyScenarios(a, b) {
  for (let iteration = 0; iteration < repeat; iteration++) {
    for (const node of nodes) { await node.drain(); node.events = [] }
    await scenario(`E09-side-${legacySide}-${iteration}`, async () => {
      const deadline = performance.now() + 22_000
      while (performance.now() < deadline) {
        await online(nodes, 1000)
        await delay(100)
      }
      await transfer(a, b, `legacy-healthy-${iteration}`)
      const activated = partition(b, true)
      await offline(b, 70_000 - (performance.now() - activated))
      partition(b, false)
      await online(nodes, 92_000)
      await transfer(a, b, `legacy-healed-${iteration}`)
    })
  }
}

async function startRelay() {
  assert(relayBinary, 'a locally built relay is required')
  relay = spawn(resolve(relayBinary), ['10.233.0.1:19090'], { stdio: ['ignore', 'pipe', 'pipe'] })
  relay.stderr.resume()
  await Promise.race([
    once(createInterface({ input: relay.stdout }), 'line').then(([line]) => assert.equal(line, 'ready')),
    once(relay, 'exit').then(() => { throw new Error('local relay failed to start') }),
    delay(5000).then(() => { throw new Error('relay startup deadline exceeded') }),
  ])
}

async function stopRelay() {
  relay.kill('SIGINT')
  await once(relay, 'exit')
  assert.equal(relay.exitCode, 0, 'local relay failed to shut down')
}

function blockDirect(node) {
  nft(node, 'add', 'table', 'inet', 'uc_direct')
  nft(node, 'add', 'chain', 'inet', 'uc_direct', 'output', '{ type filter hook output priority -50; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_direct', 'output', 'meta', 'l4proto', 'udp', 'counter', 'drop')
  net(node, 'node', '-e', "const s=require('dgram').createSocket('udp4');s.send('probe',19091,'10.233.0.1',()=>s.close())")
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'uc_direct'))
  assert(rules.nftables.some(row => row.rule?.expr?.some(expr => expr.counter?.packets > 0)), 'direct-path drop rule was not exercised')
}

async function relayScenarios(a, b) {
  for (let iteration = 0; iteration < repeat; iteration++) {
    for (const node of nodes) { await node.drain(); node.events = [] }
    await scenario(`E10-direct-${iteration}`, async () => {
      await stopRelay()
      await transfer(a, b, `direct-with-relay-down-${iteration}`)
      await delay(1000)
      await startRelay()
      const deadline = performance.now() + 22_000
      while (performance.now() < deadline) { await online(nodes, 1000); await delay(100) }
      for (const node of nodes) {
        await node.drain()
        assert(!node.events.some(event => event.kind === 'recovery' || (event.kind === 'peer' && event.state !== 'online')), 'relay interruption damaged direct connectivity')
      }
    })
  }
  for (const node of nodes) await node.stop()
  for (const node of nodes) blockDirect(node)
  for (const node of nodes) await node.start()
  await until(async () => nodes.every(node => net(node, 'ss', '-Hnt', 'state', 'established').includes(':19090')), 20_000, 'test hosts did not connect to the local relay')
  await online(nodes, 20_000)
  await transfer(a, b, 'relay-only-baseline')
  for (let iteration = 0; iteration < repeat; iteration++) {
    for (const node of nodes) { await node.drain(); node.events = [] }
    await scenario(`E10-relay-only-${iteration}`, async () => {
      await stopRelay()
      await offline(b, 20_000)
      await startRelay()
      await online(nodes, 20_000)
      await transfer(a, b, `relay-only-healed-${iteration}`)
      for (const node of nodes) {
        await node.drain()
        assert(!node.events.some(event => event.kind === 'recovery'), 'relay loss caused whole-session recovery')
      }
    })
  }
}

let failed = false
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => {
  interrupted = true
  failed = true
  for (const node of nodes) node.child?.kill('SIGTERM')
})
try { await run() }
catch (error) { failed = true; process.stderr.write(`connection recovery validation failed: ${error.message}\n`) }
finally {
  for (const node of nodes) {
    if (node.child?.exitCode === null && node.pending.length === 0) {
      try { await node.call('flush') } catch {}
    }
  }
  const allowedReasons = new Set(['no_consumer', 'receipt_dropped', 'application_timeout', 'rejected', 'sync_disabled', 'settings_unavailable', 'membership_scope_blocked', 'membership_scope_unavailable', 'member_missing', 'member_lookup_failed', 'receive_disabled', 'content_type_disabled', 'session_locked', 'content_key_missing', 'content_key_epoch_mismatch', 'invalid_transfer_format', 'decryption_failed', 'cipher_unavailable', 'invalid_content', 'apply_failed', 'permission_denied', 'storage_full', 'read_only_filesystem', 'io_failed'])
  for (const node of nodes) {
    node.failureReasons = []
    node.networkFacts = []
    try {
      for (const name of readdirSync(join(node.root, 'logs')).filter(name => name.includes('.json'))) {
        for (const line of readFileSync(join(node.root, 'logs', name), 'utf8').split('\n')) {
          try {
            const row = JSON.parse(line)
            const reason = row.fields?.['error.reason']
            if (allowedReasons.has(reason)) node.failureReasons.push({ timestamp: row.timestamp, reason })
            const fields = row.fields ?? {}
            if (['relay.status.observed', 'address.loaded', 'address.used'].includes(fields['event.name'])) {
              node.networkFacts.push(Object.fromEntries(Object.entries(fields).filter(([key]) => ['event.name', 'connected_count', 'known_count', 'direct_count', 'relay_count', 'other_count'].includes(key))))
            }
          } catch {}
        }
      }
    } catch {}
    if (failed && node.failureReasons.length) process.stderr.write(`${node.label}: ${[...new Set(node.failureReasons.map(row => row.reason))].join(', ')}\n`)
  }
  for (const node of nodes) {
    if (node.child?.exitCode === null) {
      node.child.kill('SIGTERM')
      await Promise.race([once(node.child, 'exit'), delay(2000)])
      if (node.child.exitCode === null && node.child.signalCode === null) { node.child.kill('SIGKILL'); await once(node.child, 'exit') }
    }
  }
  if (relay?.exitCode === null && relay.signalCode === null) { relay.kill('SIGINT'); await once(relay, 'exit') }
  server?.close()
  let plaintextClean = false
  if (nodes.length) {
    try {
      const probe = join(root, 'plaintext-probe')
      writeFileSync(probe, 'synthetic-', { mode: 0o600 })
      command('bash', [resolve('scripts/security/scan-plaintext-probe.sh'), probe, ...nodes.map(node => node.root)])
      plaintextClean = true
    } catch { failed = true; process.stderr.write('test content plaintext scan failed\n') }
  }
  let cleaned = true
  for (const namespace of namespaces.reverse()) { try { ip('netns', 'del', namespace) } catch { cleaned = false } }
  try { ip('link', 'del', bridge) } catch { cleaned = false }
  rmSync(root, { recursive: true, force: true })
  const binaries = [binary, legacyBinary, relayBinary].filter(Boolean).map(path => ({ sha256: createHash('sha256').update(readFileSync(path)).digest('hex') }))
  if (!cleaned) failed = true
  writeFileSync(join(evidence, `${mode}${mode === 'legacy' ? `-${legacySide}` : ''}.json`), JSON.stringify({ sequence: 'fixed-short-long-heal-v1', binaries, faults, records, cleaned, plaintext_clean: plaintextClean, failed, nodes: nodes.map(node => ({ label: node.label, version: node.version, events: node.timeline, resources: node.resources, failure_reasons: node.failureReasons, network_facts: node.networkFacts })) }, null, 2), { mode: 0o600 })
}
process.exitCode = failed ? 1 : 0
