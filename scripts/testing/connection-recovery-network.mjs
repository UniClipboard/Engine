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
assert(['direct', 'known-peer', 'legacy', 'relay', 'lan-only', 'lan-only-vpn'].includes(mode))
const legacySide = Number(options.get('--legacy-side') ?? 1)
const relayBinary = options.get('--relay')
const legacyBinary = options.get('--legacy-host')
let relay
const repeat = Number(options.get('--repeat') ?? 3)
assert(Number.isInteger(repeat) && repeat > 0)
// 多进程状态读取会跨过截止点少量时间；只给观测过程留余量，不改变 Engine 的二十秒预算。
const offlineObservationGrace = 250
const evidence = resolve(options.get('--evidence') ?? 'target/connection-recovery-evidence')
mkdirSync(evidence, { recursive: true, mode: 0o700 })
const runId = `ucr${process.pid}`
const nodes = []
const namespaces = []
const bridge = `${runId}br`.slice(0, 15)
const vpnBridge = `${runId}bv`.slice(0, 15)
const root = mkdtempSync(join(tmpdir(), 'uc-connectivity-'))
const records = []
const faults = []
const timingStartedAt = performance.now()
let firstScenarioStartedAt
let server
let rendezvousRequests = 0
// 只记录去掉邀请码后的请求形态，用于证明哪一步访问了 rendezvous。
const rendezvousRoutes = []
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
    this.bindPort = mode === 'known-peer' ? 21_000 + this.index : undefined
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

async function paired(group, afterInvite) {
  const created = await group[0].call('create', { name: group[0].label })
  group[0].id = created.device
  for (const node of group.slice(1)) {
    const invitation = await group[0].call('invite')
    if (afterInvite) await afterInvite()
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
  }, budget + offlineObservationGrace, 'silent disconnection exceeded its deadline')
}

async function online(group, budget, description = 'automatic connection exceeded its deadline') {
  await until(async () => {
    for (const node of group) {
      const response = await node.raw({ command: 'peers' })
      if (response.error) return false
      const peers = response.ok
      await node.drain()
      if (!group.filter(peer => peer !== node).every(peer => peers.some(row => row.peer_id === peer.id && row.connected))) return false
    }
    return true
  }, budget, description)
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

async function transferFile(left, right, marker, extraBytes = 0) {
  for (const [sender, receiver] of [[left, right], [right, left]]) {
    const handle = `managed-${marker}-${sender.label}`
    const displayName = `evidence-${sender.label}.bin`
    const content = `managed-file-payload-${marker}-${sender.label}`.padEnd(extraBytes, 'x')
    const result = await sender.call('send_file', {
      peer: receiver.id,
      handle,
      display_name: displayName,
      mime_type: 'application/octet-stream',
      content,
    })
    assert.equal(result.total_accepted, 1, `${sender.label} to ${receiver.label}: file not accepted`)
    await until(async () => {
      for (const entry of await receiver.call('history')) {
        if (entry.content_type !== 'file') continue
        const response = await receiver.raw({ command: 'read_file', entry: entry.entry_id })
        if (!response.ok || response.ok.file_name !== displayName) continue
        if (Buffer.from(response.ok.bytes).equals(Buffer.from(content))) return true
      }
      return false
    }, 60_000, 'exact file bytes did not arrive')
  }
}

async function pairingProof(group, created) {
  const setups = await Promise.all(group.map(node => node.call('setup')))
  const peerCounts = await Promise.all(group.map(async node => (await node.call('peers')).length))
  return {
    node_count: group.length,
    setup_completed: setups.every(setup => setup.has_completed),
    same_space: setups.every(setup => setup.space_id === created.space),
    peer_counts: peerCounts,
    communication_usable: true,
    connections_online: true,
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

// 宿主监听的 UDP 端口（不含 mDNS 5353）；用于证明重启后随机端口确实改变。
function listeningUdpPorts(node) {
  return new Set(net(node, 'ss', '-H', '-l', '-u', '-n').trim().split('\n').filter(Boolean)
    .map(line => Number(line.trim().split(/\s+/)[3].split(':').at(-1)))
    .filter(port => port !== 5353))
}

// 给节点一条经测试网桥的默认路由，并统计发往测试网段之外（含 DNS）的全部出站包。网桥不转发，
// 这些包到不了任何外部服务；计数只用于证明仅局域网模式从未尝试访问 relay、公共发现或 DNS。
function watchEgress(node, gateway = '10.233.0.1') {
  net(node, 'ip', 'route', 'add', 'default', 'via', gateway)
  nft(node, 'add', 'table', 'inet', 'uc_egress')
  nft(node, 'add', 'chain', 'inet', 'uc_egress', 'output', '{ type filter hook output priority -70; policy accept; }')
  nft(node, 'add', 'rule', 'inet', 'uc_egress', 'output', 'udp', 'dport', '53', 'counter', 'drop')
  nft(node, 'add', 'rule', 'inet', 'uc_egress', 'output', 'tcp', 'dport', '53', 'counter', 'drop')
  nft(node, 'add', 'rule', 'inet', 'uc_egress', 'output', 'ip', 'daddr', '!=', '{ 10.233.0.0/24, 100.64.0.0/24, 10.44.0.0/24, 127.0.0.0/8, 224.0.0.0/4 }', 'counter', 'drop')
  nft(node, 'add', 'rule', 'inet', 'uc_egress', 'output', 'ip6', 'daddr', '!=', '{ ::1, fe80::/10, ff00::/8 }', 'counter', 'drop')
  // 独立探测证明计数有效：一次公共 DNS 形态的发送必须被记录。
  try { net(node, 'node', '-e', "const s=require('dgram').createSocket('udp4');s.send('probe',53,'192.0.2.1',()=>s.close())") } catch {}
  const baseline = egressPackets(node)
  assert(baseline > 0, 'egress counters were not exercised')
  faults.push({ node: node.label, action: 'egress_watched', at_ms: Math.round(performance.now()), verified_counted_packets: baseline })
  return baseline
}

function egressPackets(node) {
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'uc_egress'))
  return rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
}

// 地址仓储写入成功的诊断次数（不含地址内容）；用于等待入站写回完成。
async function savedAddressRecords(node) {
  await node.call('flush')
  let count = 0
  for (const name of readdirSync(join(node.root, 'logs')).filter(name => name.includes('.json'))) {
    for (const line of readFileSync(join(node.root, 'logs', name), 'utf8').split('\n')) {
      try { if (JSON.parse(line).fields?.['event.name'] === 'address.record.saved') count++ } catch {}
    }
  }
  return count
}

async function disconnected(observer, peer, budget, description) {
  await until(async () => (await observer.call('peers')).some(row => row.peer_id === peer.id && !row.connected), budget, description)
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

// 节点结构化日志中，给定时刻之后出现的第一条新连接的方向（inbound / outbound）；没有则为 undefined。
function firstConnectionDirectionSince(node, since) {
  let first
  for (const name of readdirSync(join(node.root, 'logs')).filter(name => name.includes('.json'))) {
    for (const line of readFileSync(join(node.root, 'logs', name), 'utf8').split('\n')) {
      try {
        const row = JSON.parse(line)
        if (row.fields?.['event.name'] !== 'connection.established' || row.timestamp < since) continue
        if (!first || row.timestamp < first.timestamp) first = { timestamp: row.timestamp, direction: row.fields.direction }
      } catch {}
    }
  }
  return first?.direction
}

async function scenario(id, action) {
  if (only && !id.startsWith(only)) return
  firstScenarioStartedAt ??= performance.now()
  const started = new Date().toISOString()
  const clock = performance.now()
  const record = { id, started, outcome: 'failed' }
  records.push(record)
  try {
    const proof = await action()
    if (proof !== undefined) record.proof = proof
    record.outcome = 'passed'
  }
  finally { record.elapsed_ms = Math.round(performance.now() - clock); record.completed = new Date().toISOString() }
  process.stdout.write(`${id}: passed (${record.elapsed_ms} ms)\n`)
  return record
}

async function requiredScenario(id, action) {
  if (!only || id.startsWith(only)) return scenario(id, action)
  return action()
}

async function handleRendezvousRequest(request, response) {
  rendezvousRequests++
  rendezvousRoutes.push(`${request.method} ${(request.url ?? '').replace(/^\/v1\/pairings\/[^/]+/, '/v1/pairings/<code>')}`)
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
      const contactStartedAt = new Date().toISOString()
      const deadline = contactStarted + 20_000
      await scenario(`E13-known-peer-contact-${iteration}`, async () => {
        await c.start()
        assert(udpPortBound(c, c.bindPort), 'the replacement fixed UDP port is not listening')
        assert(!udpPortBound(c, oldPort), 'the previous fixed UDP port became reachable again')
        const remaining = deadline - performance.now()
        assert(remaining > 0, 'host restart exhausted the automatic recovery budget')
        await online([c, a], remaining)
        const onlineAt = performance.now()
        assert(onlineAt - contactStarted <= 20_000, 'automatic known-peer recovery exceeded 20 seconds')
        // 防火墙只能拦住 a 的“新建”出站；c 的流量让同 5 元组进入应答方向后，a 按成员恢复逻辑回拨也会放行，
        // 所以最终在线连接的方向取决于竞速，不作断言。可证明的是：c 先联系了 a（a 收到的第一条新连接是入站）。
        await a.call('flush')
        const first = firstConnectionDirectionSince(a, contactStartedAt)
        assert(first === 'inbound', `the first connection the peer saw after the restart was ${first ?? 'absent'}, not the restarted device contacting it`)
        const forbidden = new Set(['opportunity', 'recover', 'send', 'suspend', 'resume'])
        for (const node of [c, a]) {
          assert(!node.commands.slice(commandBaselines.get(node)).some(command => forbidden.has(command)), 'the scenario used a forbidden recovery trigger before Online')
        }
        records.at(-1).proof = {
          old_port_closed: true,
          replacement_port_bound: true,
          discovery_blocked: true,
          public_discovery_disabled: true,
          restarted_device_contacted_first: true,
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

// 仅局域网：宿主以 `relay: false` 启动，即 `allow_relay_fallback = false` 的生产路径（无 relay、只用 mDNS）。
// 全程屏蔽组播，只靠完整邀请、已保存地址与固定端口建立和恢复连接。
async function lanOnlyScenarios(a, b) {
  const fixedPort = 23_000
  blockDiscovery(a)
  blockDiscovery(b)
  const rendezvousBaseline = rendezvousRequests

  await scenario('L01-full-invitation-pairing-without-multicast', async () => {
    // 前置检查：证明宿主运行的是生产的进程级 LAN-only 策略。该策略被空操作化（例如宿主与
    // in-process-multi-node 特性一起构建）时，签发邀请仍会访问 rendezvous；此时环境无效，
    // 后续任何结果都不能作为仅局域网的证据，所以在配对之前直接终止。
    const created = await paired([a, b], () => {
      assert.equal(rendezvousRequests, rendezvousBaseline, `ENVIRONMENT INVALID: the host is not running the production LAN-only policy (rendezvous contacted while issuing an invitation: ${rendezvousRoutes.join(', ')})`)
    })
    await transfer(a, b, 'lan-only-paired')
    const proof = await pairingProof([a, b], created)
    assert.equal(rendezvousRequests, rendezvousBaseline, `LAN-only pairing contacted the rendezvous service: ${rendezvousRoutes.join(', ')}`)
    return { ...proof, discovery_blocked: true, rendezvous_requests: 0, bidirectional_transfer: true }
  })

  await scenario('L02-settings-fixed-port', async () => {
    const learnedBefore = await savedAddressRecords(b)
    assert('Updated' in await a.call('listen_port', { port: fixedPort }), 'the fixed port setting was rejected')
    await a.stop()
    await disconnected(b, a, 20_000, 'fixed-port restart was not observed')
    await a.start()
    assert(udpPortBound(a, fixedPort), 'the fixed port from settings is not listening')
    await online([a, b], 20_000, 'the fixed-port restart did not reconnect')
    // 未重启的一方在连接建立后学到对方新的固定端口地址；这是 L03 中它重启后能找到对方的前提。
    await until(async () => (await savedAddressRecords(b)) > learnedBefore, 20_000, 'the peer that stayed up did not save the restarted fixed-port address')
    await transfer(a, b, 'lan-only-fixed-port')
    return { fixed_port_from_settings: true, peer_address_saved_after_reconnect: true, discovery_blocked: true, bidirectional_transfer: true }
  })

  await scenario('L03-random-port-restart-recovers', async () => {
    const previousPorts = listeningUdpPorts(b)
    const learnedBefore = await savedAddressRecords(a)
    await b.stop()
    await disconnected(a, b, 20_000, 'random-port restart was not observed')
    const restartedAt = performance.now()
    await b.start()
    const currentPorts = listeningUdpPorts(b)
    assert(currentPorts.size > 0 && [...currentPorts].every(port => !previousPorts.has(port)), 'the random port did not change')
    // B 用保存的 A 固定端口地址主动连接；没有组播、没有中转。
    await online([a, b], 20_000, 'the random-port peer did not reconnect to the fixed port')
    const recoveredMs = Math.round(performance.now() - restartedAt)
    // 固定端口一方在入站连接通过准入后保存对方的新地址，之后它重启也能找到对方。
    await until(async () => (await savedAddressRecords(a)) > learnedBefore, 20_000, 'the fixed-port side did not save the restarted peer address')
    await transfer(a, b, 'lan-only-random-port-restart')
    return { random_port_changed: true, recovered_within_ms: recoveredMs, inbound_address_saved: true, discovery_blocked: true, bidirectional_transfer: true }
  })

  await scenario('L04-both-random-restart-needs-discovery', async () => {
    assert('Updated' in await a.call('listen_port', { port: 0 }), 'clearing the fixed port was rejected')
    const previous = [listeningUdpPorts(a), listeningUdpPorts(b)]
    await a.stop()
    await b.stop()
    await a.start()
    await b.start()
    for (const [index, node] of [a, b].entries()) {
      assert([...listeningUdpPorts(node)].every(port => !previous[index].has(port)), 'a random port did not change')
    }
    // 设计上的已知限制：两端同时换随机端口且没有组播时不能自动恢复。
    await delay(20_000)
    for (const [node, peer] of [[a, b], [b, a]]) {
      assert(!(await node.call('peers')).some(row => row.peer_id === peer.id && row.connected), 'peers recovered without any valid address')
    }
    unblockDiscovery(a)
    unblockDiscovery(b)
    const restoredAt = performance.now()
    await online([a, b], 20_000)
    const restoredMs = Math.round(performance.now() - restoredAt)
    await transfer(a, b, 'lan-only-discovery-restored')
    return { both_ports_changed: true, recovered_without_discovery: false, recovered_after_discovery_within_ms: restoredMs }
  })

  await scenario('L05-no-public-infrastructure', async () => {
    const egress = nodes.map(node => ({ node: node.label, packets: egressPackets(node) - node.egressBaseline }))
    assert(egress.every(row => row.packets === 0), 'LAN-only sent packets outside the test network')
    assert.equal(rendezvousRequests, rendezvousBaseline, `LAN-only contacted the rendezvous service: ${rendezvousRoutes.join(', ')}`)
    return { egress_packets_outside_test_network: egress, rendezvous_requests: 0 }
  })
}

// VPN 形态的仅局域网验收：节点之间只有单播路径且没有组播。V1–V2 用 Tailscale 形态的 100.64/10 地址，
// V3 用真实 WireGuard 隧道承载 10.x 地址。这里验证的是地址过滤、可信网段、完整邀请、固定端口与地址写回在这类链路上
// 的行为，不验证 Tailscale 产品本身（登录、NAT 穿透、DERP）。
async function lanOnlyVpnScenarios(a, b) {
  blockDiscovery(a)
  blockDiscovery(b)
  const rendezvousBaseline = rendezvousRequests
  // 环境前置检查：VPN 网段可达，且没有经 eth0 的备用路径，否则后续结果不能说明 VPN 场景。
  const ping = (node, address) => { try { net(node, 'ping', '-c', '1', '-W', '1', address); return true } catch { return false } }
  assert(ping(a, '100.64.0.12') && ping(b, '100.64.0.11'), 'ENVIRONMENT INVALID: the VPN segment is not reachable')
  assert(!ping(a, '10.233.0.12') && !ping(b, '10.233.0.11'), 'ENVIRONMENT INVALID: a non-VPN path exists between the nodes')

  await scenario('V01-cgnat-addresses-refused-until-trusted', async () => {
    const created = await a.call('create', { name: a.label })
    a.id = created.device
    const issued = await a.raw({ command: 'invite' })
    if (issued.error) {
      assert.notEqual(issued.code, undefined, 'the refusal has no stable error code')
      return { refused_at: 'invitation', error_code: issued.code, trusted_networks: 0 }
    }
    // 邀请可以签发（例如只含回环地址）时，加入方必须无法完成配对。
    const joined = await b.raw({ command: 'join', invitation: issued.ok.invitation, name: b.label })
    if (joined.error) {
      assert.notEqual(joined.code, undefined, 'the refusal has no stable error code')
      return { refused_at: 'join_request', error_code: joined.code, trusted_networks: 0 }
    }
    await delay(15_000)
    const setup = await b.call('setup')
    assert.equal(setup.has_completed, false, 'pairing completed over addresses outside the trusted networks')
    const current = (await b.call('eligibility')).device_trust.current_join
    return { refused_at: 'join', join_status: current?.status ?? 'none', join_reason: current?.reason ?? 'none', trusted_networks: 0 }
  })

  await scenario('V02-trusted-cgnat-pairing-and-recovery', async () => {
    for (const node of [a, b]) await node.reset()
    for (const node of [a, b]) {
      assert('Updated' in await node.call('trusted_networks', { networks: ['100.64.0.0/10'] }), 'the trusted network list was rejected')
      // 可信网段在网络启动时读取，修改后需要重启才生效。
      await node.stop()
      await node.start()
    }
    const created = await paired([a, b])
    const proof = await pairingProof([a, b], created)
    await transfer(a, b, 'vpn-cgnat')
    const learnedBefore = await savedAddressRecords(a)
    await b.stop()
    await disconnected(a, b, 20_000, 'restart was not observed over the VPN')
    await b.start()
    await online([a, b], 20_000, 'the random-port peer did not reconnect over the VPN')
    await until(async () => (await savedAddressRecords(a)) > learnedBefore, 20_000, 'the VPN address of the restarted peer was not saved')
    await transfer(b, a, 'vpn-cgnat-restart')
    return { ...proof, trusted_networks: 1, discovery_blocked: true, non_vpn_path: false, vpn_address_saved_after_reconnect: true, bidirectional_transfer: true }
  })

  // 缺少 wg 工具或内核 WireGuard 支持时直接失败，不静默跳过：否则夜间任务会在没有覆盖真实隧道的情况下显示通过。
  await scenario('V03-wireguard-tunnel-large-transfer', async () => {
    assert(wireguardAvailable(), 'ENVIRONMENT INVALID: WireGuard is unavailable (the wg tool or kernel WireGuard interfaces are missing)')
    return wireguardScenario(a, b)
  })

  await scenario('V04-no-public-infrastructure', async () => {
    const egress = nodes.map(node => ({ node: node.label, packets: egressPackets(node) - node.egressBaseline }))
    assert(egress.every(row => row.packets === 0), 'LAN-only over VPN sent packets outside the test networks')
    assert.equal(rendezvousRequests, rendezvousBaseline, `LAN-only over VPN contacted the rendezvous service: ${rendezvousRoutes.join(', ')}`)
    return { egress_packets_outside_test_networks: egress, rendezvous_requests: 0 }
  })
}

function wireguardAvailable() {
  const probe = `${runId}wgprobe`
  try {
    command('wg', ['--version'])
    ip('netns', 'add', probe)
    ip('netns', 'exec', probe, 'ip', 'link', 'add', 'wgprobe0', 'type', 'wireguard')
    return true
  } catch { return false }
  finally { try { ip('netns', 'del', probe) } catch {} }
}

function wireguardTransfer(node) {
  const [, rx, tx] = net(node, 'wg', 'show', 'wg0', 'transfer').trim().split(/\s+/)
  return { rx: Number(rx), tx: Number(tx) }
}

async function wireguardScenario(a, b) {
  const fileBytes = 512 * 1024
  for (const node of [a, b]) await node.stop()
  const keys = nodes.map(node => {
    const file = join(root, `wg-${node.label}.key`)
    const secret = command('wg', ['genkey']).trim()
    writeFileSync(file, `${secret}\n`, { mode: 0o600 })
    return { file, pub: command('wg', ['pubkey'], `${secret}\n`).trim() }
  })
  for (const [index, node] of nodes.entries()) {
    const peer = 1 - index
    net(node, 'ip', 'link', 'set', 'vpn0', 'down')
    net(node, 'ip', 'link', 'set', 'eth0', 'up')
    net(node, 'ip', 'route', 'replace', 'default', 'via', '10.233.0.1')
    net(node, 'ip', 'link', 'add', 'wg0', 'type', 'wireguard')
    net(node, 'wg', 'set', 'wg0', 'listen-port', '51820', 'private-key', keys[index].file, 'peer', keys[peer].pub, 'allowed-ips', `10.44.0.${peer + 11}/32`, 'endpoint', `10.233.0.${peer + 11}:51820`)
    net(node, 'ip', 'addr', 'add', `10.44.0.${index + 11}/24`, 'dev', 'wg0')
    net(node, 'ip', 'link', 'set', 'wg0', 'mtu', '1420', 'up')
    // 底层网络只放行 WireGuard 的外层 UDP；其余流量（包括 Engine 直接使用底层地址）一律丢弃并计数。
    const peerUnderlay = `10.233.0.${peer + 11}`
    nft(node, 'add', 'table', 'inet', 'uc_underlay')
    nft(node, 'add', 'chain', 'inet', 'uc_underlay', 'output', '{ type filter hook output priority -60; policy accept; }')
    nft(node, 'add', 'rule', 'inet', 'uc_underlay', 'output', 'ip', 'daddr', peerUnderlay, 'udp', 'dport', '51820', 'accept')
    nft(node, 'add', 'rule', 'inet', 'uc_underlay', 'output', 'ip', 'daddr', peerUnderlay, 'counter', 'drop')
    nft(node, 'add', 'chain', 'inet', 'uc_underlay', 'input', '{ type filter hook input priority -60; policy accept; }')
    nft(node, 'add', 'rule', 'inet', 'uc_underlay', 'input', 'ip', 'saddr', peerUnderlay, 'udp', 'sport', '51820', 'accept')
    nft(node, 'add', 'rule', 'inet', 'uc_underlay', 'input', 'ip', 'saddr', peerUnderlay, 'counter', 'drop')
  }
  const pingOnce = (node, address) => { try { net(node, 'ping', '-c', '1', '-W', '1', address); return true } catch { return false } }
  await until(async () => pingOnce(a, '10.44.0.12') && pingOnce(b, '10.44.0.11'), 15_000, 'ENVIRONMENT INVALID: the WireGuard tunnel did not come up')
  assert(!pingOnce(a, '10.233.0.12') && !pingOnce(b, '10.233.0.11'), 'ENVIRONMENT INVALID: the underlay is reachable outside the tunnel')
  for (const node of [a, b]) await node.reset()
  const created = await paired([a, b])
  const proof = await pairingProof([a, b], created)
  const before = [wireguardTransfer(a), wireguardTransfer(b)]
  await transferFile(a, b, 'wireguard', fileBytes)
  const after = [wireguardTransfer(a), wireguardTransfer(b)]
  // 每个方向都发送了 fileBytes 字节，隧道计数必须至少增长这么多，证明数据经过了隧道而不是底层网络。
  const sent = after.map((row, index) => row.tx - before[index].tx)
  assert(sent.every(bytes => bytes >= fileBytes), `the tunnel carried less than the transferred payload (${sent.join(', ')} bytes)`)
  return { ...proof, tunnel_mtu: 1420, file_bytes_each_direction: fileBytes, tunnel_tx_bytes_growth: sent, underlay_isolated: true, trusted_networks: 0, bidirectional_transfer: true }
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
  if (mode === 'lan-only-vpn') {
    // 第二张网桥模拟 VPN 网段：只有单播，网桥本机地址充当网关；节点的 eth0 保持关闭，VPN 是唯一路径。
    ip('link', 'add', vpnBridge, 'type', 'bridge')
    ip('addr', 'add', '100.64.0.1/24', 'dev', vpnBridge)
    ip('link', 'set', vpnBridge, 'up')
  }
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
    if (mode === 'lan-only') node.egressBaseline = watchEgress(node)
    if (mode === 'lan-only-vpn') {
      const vpn = `${runId}w${index}`.slice(0, 15)
      ip('link', 'add', vpn, 'type', 'veth', 'peer', 'name', 'vpn0', 'netns', node.namespace)
      ip('link', 'set', vpn, 'master', vpnBridge)
      ip('link', 'set', vpn, 'up')
      net(node, 'ip', 'addr', 'add', `100.64.0.${index + 11}/24`, 'dev', 'vpn0')
      net(node, 'ip', 'link', 'set', 'vpn0', 'up')
      net(node, 'ip', 'link', 'set', 'eth0', 'down')
      node.egressBaseline = watchEgress(node, '100.64.0.1')
    }
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
  if (mode === 'lan-only') {
    const [a, b] = nodes
    await lanOnlyScenarios(a, b)
    for (const node of nodes) await node.stop()
    return
  }
  if (mode === 'lan-only-vpn') {
    const [a, b] = nodes
    await lanOnlyVpnScenarios(a, b)
    for (const node of nodes) await node.stop()
    return
  }
  if (mode === 'relay') await until(async () => nodes.every(node => net(node, 'ss', '-Hnt', 'state', 'established').includes(':19090')), 20_000, 'test hosts did not connect to the configured relay')
  if (mode === 'legacy') {
    await legacyPairingIsRejected(nodes[0], nodes[1])
    for (const node of nodes) await node.stop()
    return
  }
  const [a, b, c] = nodes
  await requiredScenario('E01-complete-pairing', async () => {
    const created = await paired(nodes)
    return pairingProof(nodes, created)
  })
  if (!only || !only.startsWith('E01')) {
    await requiredScenario('E02-text-transfer', async () => {
      await transfer(a, b, 'baseline')
      if (c) await transfer(a, c, 'baseline')
      return { exact_text_verified: true, direction_count: c ? 4 : 2 }
    })
    await requiredScenario('E02-file-transfer', async () => {
      await transferFile(a, b, 'baseline')
      return { exact_bytes_verified: true, direction_count: 2 }
    })
  }
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

async function legacyPairingIsRejected(sponsor, joiner) {
  assert.equal(legacySide, 0, 'legacy incompatibility validation requires the legacy sponsor')
  const created = await sponsor.call('create', { name: sponsor.label })
  sponsor.id = created.device
  const invitation = await sponsor.call('invite')
  const initial = await joiner.call('join', { invitation: invitation.invitation, name: joiner.label })
  assert.equal(initial.status, 'pending')
  await scenario('E09-legacy-pairing-rejected', async () => {
    await until(async () => {
      const choices = await joiner.call('eligibility')
      const current = choices.device_trust.current_join
      if (!current || current.status === 'pending') return false
      assert.equal(current.status, 'rejected')
      assert(
        ['authentication_rejected', 'peer_upgrade_required'].includes(current.reason),
        'legacy pairing ended with an unexpected result',
      )
      return true
    }, 20_000, 'legacy pairing did not reach a stable rejection')
  })
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
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).reduce((total, expr) => total + (expr.counter?.packets ?? 0), 0)
  assert(dropped > 0, 'direct-path drop rule was not exercised')
  faults.push({ node: node.label, action: 'direct_paths_blocked', at_ms: Math.round(performance.now()), verified_dropped_packets: dropped })
  return dropped
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
  const directDropPackets = nodes.reduce((total, node) => total + blockDirect(node), 0)
  for (const node of nodes) await node.start()
  await until(async () => nodes.every(node => net(node, 'ss', '-Hnt', 'state', 'established').includes(':19090')), 20_000, 'test hosts did not connect to the local relay')
  await online(nodes, 20_000)
  await transfer(a, b, 'relay-only-baseline')
  for (let iteration = 0; iteration < repeat; iteration++) {
    for (const node of nodes) { await node.drain(); node.events = [] }
    await scenario(`E10-relay-only-${iteration}`, async () => {
      const recoveryBaseline = nodes.reduce((total, node) => total + node.recoveries, 0)
      const proof = records.at(-1).proof = {
        direct_paths_blocked: directDropPackets > 0,
        direct_drop_packets: directDropPackets,
        both_relay_transports_ready: false,
        bidirectional_transfer: false,
      }
      await stopRelay()
      await offline(b, 20_000)
      await startRelay()
      await until(async () => nodes.every(node => net(node, 'ss', '-Hnt', 'state', 'established').includes(':19090')), 20_000, 'test hosts did not reconnect to the local relay')
      const relayReadyAt = performance.now()
      proof.both_relay_transports_ready = true
      const deadline = relayReadyAt + 3000
      const remaining = deadline - performance.now()
      assert(remaining > 0, 'relay transport observation exhausted the three-second recovery budget')
      await online(nodes, remaining)
      const onlineAt = performance.now()
      proof.relay_transport_ready_to_online_ms = Math.round(onlineAt - relayReadyAt)
      await transfer(a, b, `relay-only-healed-${iteration}`)
      const transferAt = performance.now()
      proof.relay_transport_ready_to_bidirectional_transfer_ms = Math.round(transferAt - relayReadyAt)
      proof.bidirectional_transfer = true
      for (const node of nodes) {
        await node.drain()
        assert(!node.events.some(event => event.kind === 'recovery'), 'relay loss caused whole-session recovery')
      }
      proof.whole_session_recovery_count = nodes.reduce((total, node) => total + node.recoveries, 0) - recoveryBaseline
      assert.equal(proof.whole_session_recovery_count, 0, 'relay loss caused whole-session recovery')
      assert(transferAt <= deadline, 'relay-only bidirectional recovery exceeded three seconds')
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
  const cleanupStartedAt = performance.now()
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
            const historySync = fields['event.name'] === 'uc.operation.completed' && fields['uc.operation'] === 'membership_history_sync'
            if (historySync || ['relay.status.observed', 'address.loaded', 'address.used', 'address.record.saved', 'address.record.save_failed'].includes(fields['event.name'])) {
              node.networkFacts.push({ timestamp: row.timestamp, ...Object.fromEntries(Object.entries(fields).filter(([key]) => ['event.name', 'connected_count', 'known_count', 'direct_count', 'relay_count', 'other_count', 'uc.role', 'uc.outcome'].includes(key))) })
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
  if (mode === 'lan-only-vpn') { try { ip('link', 'del', vpnBridge) } catch { cleaned = false } }
  rmSync(root, { recursive: true, force: true })
  const binaries = [binary, legacyBinary, relayBinary].filter(Boolean).map(path => ({ sha256: createHash('sha256').update(readFileSync(path)).digest('hex') }))
  if (!cleaned) failed = true
  const reproduction = `bash scripts/testing/run-connection-recovery-e2e.sh --suite network --repeat ${repeat} --mode ${mode}${only ? ` --case ${only}` : ''}`
  const timingCompletedAt = performance.now()
  const scenarioStartedAt = firstScenarioStartedAt ?? cleanupStartedAt
  const timings = {
    prepare_ms: Math.round(scenarioStartedAt - timingStartedAt),
    scenario_ms: Math.round(cleanupStartedAt - scenarioStartedAt),
    cleanup_ms: Math.round(timingCompletedAt - cleanupStartedAt),
    total_ms: Math.round(timingCompletedAt - timingStartedAt),
  }
  writeFileSync(join(evidence, `${mode}${mode === 'legacy' ? `-${legacySide}` : ''}.json`), JSON.stringify({ sequence: 'fixed-short-long-heal-v2', reproduction, binaries, faults, records, timings, cleaned, plaintext_clean: plaintextClean, failed, nodes: nodes.map(node => ({ label: node.label, version: node.version, events: node.timeline, resources: node.resources, failure_reasons: node.failureReasons, network_facts: node.networkFacts })) }, null, 2), { mode: 0o600 })
}
process.exitCode = failed ? 1 : 0
