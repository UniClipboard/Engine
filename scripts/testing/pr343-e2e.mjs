import { spawn, execFileSync } from 'node:child_process'
import { createInterface } from 'node:readline'
import { once } from 'node:events'
import { createHash, randomUUID, randomBytes } from 'node:crypto'
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, readdirSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { tmpdir } from 'node:os'
import { setTimeout as delay } from 'node:timers/promises'
import { ensureSessionReady } from './connection-recovery-session.mjs'

// 独立进程验收：只调用公开 Engine 操作，不在运行器中实现业务重试。
const args = new Map()
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1])
const binary = resolve(args.get('--host') ?? 'target/debug/uc-connectivity-host')
const evidence = resolve(args.get('--evidence') ?? 'target/connection-recovery-evidence/engine-343')
const selected = args.get('--case')
const addressProbeEnabled = args.get('--address-probe') !== 'disabled'
const baselineBinary = args.get('--baseline-host') ? resolve(args.get('--baseline-host')) : undefined
const invitationFixture = args.get('--invitation-fixture') ? resolve(args.get('--invitation-fixture')) : undefined
const regressionHost = args.get('--regression-host') ? resolve(args.get('--regression-host')) : undefined
const relayBinary = args.get('--relay') ? resolve(args.get('--relay')) : undefined
const legacyBinary = args.get('--legacy-host') ? resolve(args.get('--legacy-host')) : undefined
const repeat = Number(args.get('--repeat') ?? 1)
if (!Number.isInteger(repeat) || repeat < 1) throw new Error('invalid repeat')
mkdirSync(evidence, { recursive: true, mode: 0o700 })

class Failure extends Error {
  constructor(category, code) { super(code); this.category = category }
}
function requireFact(value, code) { if (!value) throw new Failure('product', code) }
function environment(value, code) { if (!value) throw new Failure('environment', code) }
function command(program, argv, input) {
  try { return execFileSync(program, argv, { input, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'], timeout: 15_000 }) }
  catch (error) {
    const failure = new Failure('environment', `${program}_command_failed`)
    // 只保存固定错误类别；不导出命令参数、密钥或地址。
    const stderr = String(error.stderr ?? '')
    failure.detail = ['Operation not permitted', 'Invalid argument', 'File exists', 'Cannot find device', 'No such file', 'syntax error', 'Required key not available'].find(text => stderr.includes(text)) ?? 'other'
    if (failure.detail === 'syntax error') failure.parser_detail = stderr.split('\n').find(line => line.includes('Error:'))?.split('Error:')[1]?.trim()
    failure.command_shape = argv.filter(arg => /^[a-z][a-z_-]*$/.test(arg)).join(' ')
    throw failure
  }
}
function ip(...argv) { return command('ip', argv) }
function net(node, ...argv) { return ip('netns', 'exec', node.namespace, ...argv) }
function listenerPort(node) {
  const ports = new Set(net(node, 'ss', '-4', '-Hlunp').split('\n')
    .filter(line => line.includes(`pid=${node.child.pid},`))
    .map(line => Number(line.trim().split(/\s+/)[3]?.match(/:(\d+)$/)?.[1]))
    .filter(port => Number.isInteger(port) && port > 0 && port !== 5353))
  node.run.facts.listener_observations ??= []
  node.run.facts.listener_observations.push({ node: node.alias, family: 'ipv4', matching_ports: ports.size })
  environment(ports.size === 1, 'engine_ipv4_udp_listener_not_unique')
  return [...ports][0]
}
function blockNewPeerDials(run, node) {
  command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], 'table inet directionproof { chain output { type filter hook output priority -60; policy accept; oifname "wg0" meta l4proto udp ct state new counter drop; }; }\n')
  net(node, 'conntrack', '-F')
  try { net(node, 'python3', '-c', 'import socket; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.sendto(b"fault-preflight",("10.44.0.11",23110))') }
  catch (error) { if (!(error instanceof Failure)) throw error }
  const rules = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'directionproof'))
  const dropped = rules.nftables.flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).reduce((sum, expr) => sum + expr.counter.packets, 0)
  run.facts.outbound_guard_probe_drop_packets = dropped
  environment(dropped > 0, 'new_outbound_block_not_hit')
}
async function until(predicate, budget, code) {
  const start = performance.now()
  while (performance.now() - start < budget) {
    const matched = await predicate()
    const elapsed = performance.now() - start
    if (elapsed >= budget) break
    if (matched) return Math.round(elapsed)
    await delay(Math.min(100, budget - elapsed))
  }
  throw new Failure('product', code)
}
async function holdUdp(run, node, port) {
  const child = spawn('ip', ['netns', 'exec', node.namespace, 'python3', '-u', '-c', 'import socket,time; s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM); s.bind(("0.0.0.0",int(__import__("sys").argv[1]))); print("ready",flush=True); time.sleep(600)', String(port)], { stdio: ['ignore', 'pipe', 'ignore'] })
  run.helpers ??= []
  run.helpers.push(child)
  await Promise.race([
    once(child.stdout, 'data').then(data => environment(data.toString().trim() === 'ready', 'udp_holder_not_ready')),
    once(child, 'exit').then(() => { throw new Failure('environment', 'udp_holder_failed') }),
    delay(5_000).then(() => { throw new Failure('environment', 'udp_holder_timeout') }),
  ])
  return child
}
const hash = value => createHash('sha256').update(value).digest('hex')
const binarySha256 = hash(readFileSync(binary))
const driverSha256 = hash(readFileSync(new URL(import.meta.url)))

// 等到进程与两个读取器全部关闭后，才冻结计数和最终结果。
function captureCompletion(child, state) {
  const stdout = createInterface({ input: child.stdout })
  const stderr = createInterface({ input: child.stderr })
  const readers = [stdout, stderr].map(reader => new Promise(resolve => reader.once('close', resolve)))
  for (const stream of [child, child.stdin, child.stdout, child.stderr].filter(Boolean)) stream.on('error', () => { state.stream_error = true })
  const closed = new Promise(resolve => child.once('close', (code, signal) => resolve([code, signal])))
  return { stdout, stderr, finished: Promise.all([closed, ...readers]).then(([result]) => result) }
}

async function stopCaptureProcess(capture, signal = 'SIGINT') {
  const { child, finished, state } = capture
  state.unexpected_early_exit = child.exitCode !== null || child.signalCode !== null
  const timer = setTimeout(() => { state.stop_timeout = true; child.kill('SIGKILL') }, 5_000)
  try {
    if (!state.unexpected_early_exit) {
      if (signal) child.kill(signal)
      else child.stdin.end('\n')
    }
    const [code, exitSignal] = await finished
    state.exit_code = code
    state.exit_signal = exitSignal
  } finally { clearTimeout(timer) }
}

async function startCaptureClockGuard(run) {
  const path = new URL('./pr343-clock-guard.py', import.meta.url)
  const state = { source_sha256: hash(readFileSync(path)), samples: [], mechanism: 'linux_timerfd_realtime_absolute_cancel_on_set', ready: false }
  const child = spawn('python3', [path.pathname], { stdio: ['pipe', 'pipe', 'pipe'] })
  const streams = captureCompletion(child, state)
  streams.stdout.on('line', line => {
    try {
      const result = JSON.parse(line)
      state.samples.push(result.sample)
      if (result.ready) state.ready = true
      if (result.outcome) state.outcome = result.outcome
    } catch { state.stream_error = true }
  })
  streams.stderr.on('line', () => { state.stream_error = true })
  run.captureClockGuard = { child, state, finished: streams.finished }
  run.facts.capture_clock_guard = state
  await until(() => Promise.resolve(state.ready && child.exitCode === null), 5_000, 'capture_clock_guard_not_ready')
}

// NFLOG 与累积计数位于同一条 OUTPUT 规则。原始数据只经过内存。
async function startSubnetCapture(run, node) {
  if (!run.captureClockGuard) await startCaptureClockGuard(run)
  const state = { node: node.alias, predicate: 'outbound_peer_global_udp', records: [], parser_errors: 0, overflow: 0, ready: false, statistics: {}, started_unix_micros: Date.now() * 1000, timestamp_source: 'libpcap_1_10_3_userspace_gettimeofday_microseconds', observation_point: 'nft_output_priority_minus_60_nflog', timestamp_quantization_micros: 1 }
  const controllerClock = readFileSync('/proc/self/timens_offsets', 'utf8')
  const nodeClock = net(node, 'cat', '/proc/self/timens_offsets')
  environment(controllerClock === nodeClock, 'capture_time_namespace_mismatch')
  state.time_namespace_offsets_match = true
  const child = spawn('ip', ['netns', 'exec', node.namespace, 'tcpdump', '-i', 'nflog:343', '-tt', '-nn', '-l', '-q', '-s', '96'], { stdio: ['ignore', 'pipe', 'pipe'] })
  const streams = captureCompletion(child, state)
  streams.stdout.on('line', line => {
    const match = line.match(/^(\d+)\.(\d{6})\s/)
    if (!match) { if (line.trim() && !/^\s/.test(line)) state.parser_errors++; return }
    if (state.records.length >= 100_000) { state.overflow++; return }
    const unix = Number(match[1]) * 1_000_000 + Number(match[2])
    if (!Number.isSafeInteger(unix) || unix <= 0 || state.records.at(-1)?.unix_micros > unix) state.parser_errors++
    state.records.push({ sequence: state.records.length + 1, unix_micros: unix, node: node.alias, direction: 'outbound', predicate: state.predicate })
  })
  streams.stderr.on('line', line => {
    if (/^listening on nflog:343,/.test(line)) state.ready = true
    const count = line.match(/^(\d+) packets? (captured|received by filter|dropped by kernel)$/)
    if (count) state.statistics[count[2].replaceAll(' ', '_')] = Number(count[1])
    else if (!/^tcpdump: verbose output suppressed|^listening on |^$/.test(line)) state.parser_errors++
  })
  run.subnetCaptures ??= []
  run.subnetCaptures.push({ node, child, finished: streams.finished, state })
  run.facts.subnet_packet_capture = run.subnetCaptures.map(capture => capture.state)
  await until(() => Promise.resolve(state.ready && child.exitCode === null), 5_000, 'subnet_capture_not_ready')
}

async function finishSubnetCaptures(run) {
  for (const capture of run.subnetCaptures ?? []) {
    const { node, state } = capture
    try { state.final_counters = subnetPackets(node) }
    catch { state.counter_error = 'final_counter_unavailable' }
    finally {
      state.stop_requested_unix_micros = Date.now() * 1000
      await stopCaptureProcess(capture)
    }
    state.accounting_matches = state.records.length === state.final_counters?.global && state.statistics.captured === state.records.length && state.statistics.received_by_filter === state.records.length
    state.valid = state.ready && !state.unexpected_early_exit && !state.stop_timeout && !state.stream_error && !run.facts.capture_producer_stop_failed && state.exit_code === 0 && state.statistics.dropped_by_kernel === 0 && state.accounting_matches && !state.parser_errors && !state.overflow
  }
  const guard = run.captureClockGuard
  if (guard) {
    await stopCaptureProcess(guard, null)
    try {
      const ranges = guard.state.samples.map(sample => [BigInt(sample.realtime_before_ns) - BigInt(sample.monotonic_ns), BigInt(sample.realtime_after_ns) - BigInt(sample.monotonic_ns)])
      const lower = ranges.reduce((value, range) => range[0] > value ? range[0] : value, ranges[0][0])
      const upper = ranges.reduce((value, range) => range[1] < value ? range[1] : value, ranges[0][1])
      guard.state.offset_intersection_ns = { lower: String(lower), upper: String(upper) }
      guard.state.valid = guard.state.samples.length === 2 && lower <= upper && guard.state.outcome === 'no_discontinuous_change' && guard.state.exit_code === 0 && !guard.state.stream_error && !guard.state.stop_timeout && !guard.state.unexpected_early_exit
    } catch { guard.state.valid = false }
  }
  if (!run.subnetCaptures?.length) return
  const a = run.subnetCaptures.find(capture => capture.node.alias === 'A')?.state
  const interval = run.facts.subnet_revocation_interval
  const snapshots = (run.facts.subnet_probe_snapshots ?? []).filter(row => row.node === 'A').map(row => row.snapshot)
  const journal = snapshots.at(-1)?.events ?? run.facts.subnet_latest?.events ?? []
  run.facts.subnet_packet_clock_journal = journal
  const integers = journal.length > 0 && journal.every(event => Number.isSafeInteger(event.unix_micros) && event.unix_micros > 0 && Number.isSafeInteger(event.elapsed_micros))
  const ordered = integers && journal.every((event, index) => index === 0 || (event.unix_micros >= journal[index - 1].unix_micros && event.elapsed_micros >= journal[index - 1].elapsed_micros))
  const offsets = integers ? journal.map(event => event.unix_micros - event.elapsed_micros) : []
  const spread = integers ? Math.max(...offsets) - Math.min(...offsets) : null
  const uncertainty = spread === null ? null : spread + 2
  const intervalValid = Boolean(interval) && [interval.invalidated_unix_micros, interval.published_false_unix_micros].every(value => Number.isSafeInteger(value) && value > 0) && interval.invalidated_unix_micros <= interval.published_false_unix_micros
  const lease = run.facts.subnet_lease_denial_interval
  const leaseValid = Boolean(lease) && Number.isSafeInteger(lease.expired_unix_micros) && lease.expired_unix_micros > 0
  const complete = intervalValid ? Number.isSafeInteger(interval.restore_begin_unix_micros) && interval.published_false_unix_micros < interval.restore_begin_unix_micros : leaseValid && Number.isSafeInteger(lease.read_released_unix_micros) && lease.expired_unix_micros < lease.read_released_unix_micros
  const start = intervalValid ? interval.invalidated_unix_micros : lease?.expired_unix_micros
  const end = complete ? intervalValid ? interval.restore_begin_unix_micros : lease.read_released_unix_micros : a?.stop_requested_unix_micros
  const records = (intervalValid || leaseValid) && uncertainty !== null && end > start ? a.records.filter(record => record.unix_micros + uncertainty >= start && record.unix_micros - uncertainty <= end) : null
  const capturesValid = run.subnetCaptures.length === 2 && ['A', 'B'].every(alias => run.subnetCaptures.some(capture => capture.node.alias === alias && capture.state.valid && capture.state.records.length > 0))
  run.facts.subnet_packet_acceptance = { complete_interval: complete, interval_kind: leaseValid ? 'real_lease_expiry_until_release' : 'native_invalidation_until_restore', diagnostic_open_interval: (intervalValid || leaseValid) && !complete, all_captures_valid: capturesValid, clock_valid: Boolean(guard?.state.valid) && ordered, journal_wall_monotonic_offset_spread_micros: spread, conservative_boundary_expansion_micros: uncertainty, denied_interval_observations: records?.length ?? null, post_invalidation_observations: intervalValid ? records?.length ?? null : null, scope: 'external_observation_envelope_not_lookup_causality', passed: complete && capturesValid && Boolean(guard?.state.valid) && ordered && records?.length === 0 }
}

class Host {
  constructor(run, index) {
    this.run = run
    this.alias = String.fromCharCode(65 + index)
    this.namespace = `${run.prefix}${index}`
    this.root = join(run.root, this.alias)
    this.pending = []
    this.index = index
    this.timeline = []
  }
  async start() {
    this.processWindows ??= []
    const processWindow = { process_alias: `${this.alias}-${this.processWindows.length + 1}`, requested_at_utc: new Date().toISOString() }
    this.processWindows.push(processWindow)
    const selectedBinary = this.binaryOverride ?? binary
    const program = this.run.topology === 'local' ? selectedBinary : 'ip'
    const argv = this.run.topology === 'local' ? [] : ['netns', 'exec', this.namespace, selectedBinary]
    this.child = spawn(program, argv, { stdio: ['pipe', 'pipe', 'pipe'], env: { ...process.env, UC_IROH_BIND_PORT: '', UC_IROH_PUBLIC_ADDR: '' } })
    const child = this.child
    this.child.stderr.resume()
    createInterface({ input: this.child.stdout }).on('line', line => {
      let response
      try { response = JSON.parse(line) } catch { return }
      if (response.uc_connectivity !== 1) return
      const pending = this.pending.shift()
      if (pending) { clearTimeout(pending.timer); pending.resolve(response) }
    })
    this.child.on('error', () => { if (this.child === child) this.rejectPending('host_spawn_failed') })
    this.child.stdin.on('error', () => { if (this.child === child) this.rejectPending('host_pipe_failed') })
    this.child.on('exit', () => { if (this.child === child) this.rejectPending('host_exited') })
    const ready = await this.raw({ root: this.root, secure_storage: this.storage, address_probe: this.addressProbe, subnet_probe: this.subnetProbe, reset_active_pull: this.resetActivePull })
    this.ready = Boolean(ready.ready)
    processWindow.ready_at_utc = new Date().toISOString()
    if (this.resetActivePull !== undefined) this.receiveObserved = true
    this.storage = ready.secure_storage ?? this.storage
    if (!ready.ready) return ready
    environment(ready.host?.capabilities?.includes('production-network-policy'), 'production_host_required')
    this.run.facts.host_version = ready.version
    this.capabilities = ready.host?.capabilities ?? []
    this.run.facts.host_sources ??= []
    this.run.facts.host_sources.push({ node: this.alias, revision: ready.host.engine_rev ?? null, anchor: ready.host.anchor })
    if (ready.host?.capabilities?.includes('detailed-network-capture')) await this.call('start_network_capture')
    return ready
  }
  rejectPending(code) {
    for (const pending of this.pending.splice(0)) {
      clearTimeout(pending.timer)
      pending.reject(new Failure('product', code))
    }
  }
  raw(request) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.child.kill('SIGKILL'); reject(new Failure('product', 'host_command_timeout')) }, 120_000)
      this.pending.push({ resolve, reject, timer })
      this.child.stdin.write(`${JSON.stringify(request)}\n`)
    })
  }
  async call(name, fields = {}) {
    const response = await this.raw({ command: name, ...fields })
    requireFact(!response.error, `${name}_failed_code_${response.code ?? 'unknown'}`)
    return response.ok
  }
  async settings(network) {
    const result = await this.call('settings_update', { patch: { network } })
    requireFact(result.Updated, 'valid_settings_rejected')
    return result.Updated.network
  }
  async drain() {
    for (const event of await this.call('events')) {
      requireFact(!['lost_events', 'host_failed'].includes(event.kind), 'lifecycle_evidence_failed')
      this.timeline.push({ at_ms: Math.round(performance.now() - this.run.start), kind: event.kind, state: typeof event.state === 'string' ? event.state : undefined })
    }
  }
  async stop() {
    if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) return
    if (!this.ready) { await once(this.child, 'exit'); return }
    this.storage = await this.call('secure_storage')
    this.processWindows.at(-1).stop_requested_at_utc = new Date().toISOString()
    const shutdown = await this.call('shutdown')
    if (shutdown?.subnet_probe) {
      this.run.facts.subnet_probe_snapshots ??= []
      this.run.facts.subnet_probe_snapshots.push({ node: this.alias, phase: 'after_shutdown', snapshot: shutdown.subnet_probe })
    }
    if (shutdown?.address_probe) {
      this.run.facts.address_probe_snapshots ??= []
      this.run.facts.address_probe_snapshots.push({ node: this.alias, phase: 'after_shutdown', snapshot: shutdown.address_probe })
    }
    if (this.child.exitCode === null && this.child.signalCode === null) await Promise.race([once(this.child, 'exit'), delay(15_000, undefined, { ref: false }).then(() => { throw new Failure('product', 'shutdown_timeout') })])
    requireFact(this.child.exitCode === 0, 'shutdown_failed')
    this.processWindows.at(-1).exited_at_utc = new Date().toISOString()
  }
}

class Run {
  constructor(id, topology) {
    this.id = id
    this.topology = topology
    this.prefix = `u3${process.pid.toString(36)}${Math.random().toString(36).slice(2, 5)}`
    this.root = mkdtempSync(join(tmpdir(), 'uc343-'))
    this.nodes = [new Host(this, 0), new Host(this, 1)]
    this.namespaces = []
    this.start = performance.now()
    this.facts = {}
    this.readbacks = []
    this.canaries = []
  }
  async prepare() {
    if (this.topology !== 'local') {
      environment(process.platform === 'linux', 'linux_required')
      for (const node of this.nodes) {
        ip('netns', 'add', node.namespace)
        this.namespaces.push(node.namespace)
        net(node, 'ip', 'link', 'set', 'lo', 'up')
      }
      this.bridge = `${this.prefix}b`.slice(0, 15)
      ip('link', 'add', this.bridge, 'type', 'bridge')
      ip('link', 'set', this.bridge, 'up')
      for (const node of this.nodes) {
        const veth = `${this.prefix}${node.index}v`.slice(0, 15)
        ip('link', 'add', veth, 'type', 'veth', 'peer', 'name', 'eth0', 'netns', node.namespace)
        ip('link', 'set', veth, 'master', this.bridge)
        ip('link', 'set', veth, 'up')
        net(node, 'ip', 'addr', 'add', `10.233.0.${11 + node.index}/24`, 'dev', 'eth0')
        net(node, 'ip', 'link', 'set', 'eth0', 'up')
      }
      if (this.topology === 'vpn') this.wireguard()
    }
    if (['M03', 'M03R', 'M04'].includes(this.id)) {
      this.mdnsBlockers = []
      for (const node of this.nodes) {
        command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], 'table inet mdnsfault { chain output { type filter hook output priority -40; policy accept; ip6 daddr ff00::/8 counter drop; }; }\n')
        this.mdnsBlockers.push(await holdUdp(this, node, 5353))
      }
    }
    for (const node of this.nodes) {
      requireFact((await node.start()).ready, 'initial_start_failed')
      await node.settings({ allow_relay_fallback: false, bind_port: 23110 + node.index })
      await node.stop()
      requireFact((await node.start()).ready, 'configured_profile_start_failed')
    }
  }
  wireguard() {
    const keys = this.nodes.map(() => command('wg', ['genkey']).trim())
    const pubs = keys.map(key => command('wg', ['pubkey'], `${key}\n`).trim())
    for (const node of this.nodes) {
      const peer = 1 - node.index
      net(node, 'ip', 'link', 'add', 'wg0', 'type', 'wireguard')
      // Node 的 stdin 是 Unix socket；用进程替换转成 wg 可以打开的匿名管道，密钥不落盘。
      command('ip', ['netns', 'exec', node.namespace, 'bash', '-c', 'read -r key; printf "%s\\n" "$key" | wg set wg0 private-key /dev/stdin "$@"', '--', 'listen-port', '51820', 'peer', pubs[peer], 'allowed-ips', `10.44.0.${11 + peer}/32`, 'endpoint', `10.233.0.${11 + peer}:51820`], `${keys[node.index]}\n`)
      net(node, 'ip', 'addr', 'add', `10.44.0.${11 + node.index}/24`, 'dev', 'wg0')
      net(node, 'ip', 'link', 'set', 'wg0', 'mtu', '1420', 'up')
      command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], `table inet uc343 {
        chain output { type filter hook output priority -50; policy accept;
          ip daddr 224.0.0.0/4 counter drop
          ip6 daddr ff00::/8 counter drop
          oifname "eth0" udp dport 51820 accept
          oifname "eth0" counter drop
        }
        chain input { type filter hook input priority -50; policy accept;
          iifname "eth0" udp dport 51820 accept
          iifname "eth0" counter drop
        }
      }`)
    }
    for (const node of this.nodes) {
      net(node, 'ping', '-c', '1', '-W', '3', `10.44.0.${12 - node.index}`)
      let underlayReachable = true
      try { net(node, 'ping', '-c', '1', '-W', '1', `10.233.0.${12 - node.index}`) } catch { underlayReachable = false }
      environment(!underlayReachable, 'underlay_not_isolated')
    }
    this.facts.wireguard_preflight = { tunnel_reachable: true, underlay_blocked: true, multicast_blocked: true, mtu: 1420 }
  }
  async pair() {
    const [a, b] = this.nodes
    const created = this.created ?? await a.call('create', { name: 'A' })
    a.id = created.device
    let invitation = await a.call('invite')
    requireFact(invitation.invitation?.startsWith('ucspace1_'), 'full_invitation_missing')
    this.canaries.push(invitation.invitation)
    if (this.invitationTransform) {
      environment(invitationFixture, 'invitation_fixture_required')
      const changed = JSON.parse(command(invitationFixture, [], JSON.stringify({ invitation: invitation.invitation, ...this.invitationTransform })))
      requireFact(changed.endpoint_identity_preserved && changed.invitation_identity_preserved && changed.expiry_preserved, 'fixture_changed_invitation_identity')
      requireFact(changed.contains_direct.every(Boolean), 'fixture_candidate_missing')
      this.facts.invitation_fixture = { identity_preserved: true, direct_count: changed.direct_count, relay_count: changed.relay_count }
      invitation = { invitation: changed.invitation }
      this.canaries.push(changed.invitation)
    }
    await b.call('join', { invitation: invitation.invitation, name: 'B' })
    await until(async () => {
      const response = await b.raw({ command: 'setup' })
      if ([1103, 1211].includes(response.code)) return false
      requireFact(!response.error, 'pairing_setup_query_failed')
      return response.ok.has_completed && response.ok.space_id === created.space
    }, 30_000, 'pairing_deadline')
    await until(async () => {
      b.id = (await a.call('peers')).find(peer => peer.device_name === 'B')?.peer_id
      return Boolean(b.id)
    }, 5_000, 'paired_identity_missing')
    await this.online()
    for (const node of this.nodes) {
      const overview = await node.call('relay_overview')
      this.facts.applied_lan_only ??= []
      this.facts.applied_lan_only.push({ node: node.alias, saved: overview.saved_mode, applied: overview.applied_mode, pending: overview.change_pending })
      const expected = node.defaultMode ? overview.saved_mode !== 'Disabled' && overview.applied_mode !== 'Disabled' : overview.saved_mode === 'Disabled' && overview.applied_mode === 'Disabled'
      requireFact(expected && !overview.change_pending, 'network_policy_not_applied')
    }
    for (const node of this.nodes) {
      await until(async () => {
        const response = await node.raw({ command: 'eligibility' })
        if ([1103, 1211].includes(response.code)) return false
        requireFact(!response.error, 'eligibility_query_failed')
        return response.ok.device_trust.devices.some(device => device.device_id === this.nodes[1 - node.index].id && device.sync_relationship === 'usable')
      }, 20_000, 'membership_not_usable')
    }
    this.facts.same_space_and_admitted_online = true
  }
  async online(budget = 20_000) {
    return until(async () => {
      for (const node of this.nodes) {
        const response = await node.raw({ command: 'peers' })
        if ([1103, 1211].includes(response.code)) return false
        requireFact(!response.error, 'peer_status_query_failed')
        if (!response.ok.some(peer => peer.peer_id === this.nodes[1 - node.index].id && peer.connected)) return false
      }
      return true
    }, budget, 'passive_reconnect_deadline')
  }
  async changedPorts() {
    for (const node of this.nodes) {
      await node.settings({ bind_port: 23210 + node.index })
      await node.stop()
    }
    for (const node of this.nodes) requireFact((await node.start()).ready, 'profile_restart_failed')
  }
  diagnostics(alias) {
    const messages = new Map([
      ['local peer discovery unavailable; continuing without mDNS and retrying in the background', 'mdns_initial_failure'],
      ['local peer discovery recovered; mDNS is available', 'mdns_recovered'],
      ['interface installation incomplete; retry scheduled', 'mdns_interface_retry'],
      ['clipboard delivery recovery dispatched', 'recovery_dispatch'],
      ['clipboard delivery recovery round finished', 'recovery_round'],
    ])
    const counts = Object.fromEntries([...messages.values()].map(key => [key, 0]))
    counts.recovery_retry_round = 0
    counts.recovery_online_dispatch = 0
    counts.address_record_saved = 0
    counts.address_record_saved_latest_ms = 0
    counts.address_loaded = 0
    counts.non_local_candidates_dropped = 0
    counts.mdns_multi_address_result = 0
    counts.mdns_candidate_result = 0
    const visit = directory => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name)
        if (entry.isDirectory()) visit(path)
        else if (entry.isFile() && /\.(jsonl|log)$/.test(entry.name)) {
          for (const line of readFileSync(path, 'utf8').split('\n')) {
            let row
            try { row = JSON.parse(line) } catch { continue }
            const message = row.message ?? row.fields?.message
            const key = messages.get(message)
            if (key) counts[key]++
            const event = row.event ?? row.fields?.['event.name'] ?? row['event.name']
            if (event === 'address.loaded') counts.address_loaded++
            const fields = row.f ?? row.fields ?? row
            if (event === 'address.discovered' && fields.source === 'mdns' && fields.direct_count > 0) counts.mdns_candidate_result++
            if (event === 'address.discovered' && fields.source === 'mdns' && fields.direct_count >= 2) counts.mdns_multi_address_result++
            if (message === 'dropped non-local dial candidates in LAN-only mode') counts.non_local_candidates_dropped += Number(row.fields?.dropped_count ?? row.dropped_count ?? 0)
            if (event === 'address.record.saved') {
              counts.address_record_saved++
              counts.address_record_saved_latest_ms = Math.max(counts.address_record_saved_latest_ms, Number(row.f?.observed_at_ms ?? row.fields?.observed_at_ms ?? 0))
            }
            if (key === 'recovery_round' && (row.fields?.trigger ?? row.trigger) === 'retry') counts.recovery_retry_round++
            if (key === 'recovery_round' && (row.fields?.trigger ?? row.trigger) === 'peer_online' && ['delivered', 'dispatched'].includes(row.fields?.outcome ?? row.outcome)) counts.recovery_online_dispatch++
          }
        }
      }
    }
    visit(alias ? join(this.root, alias) : this.root)
    return counts
  }
  counters() {
    return this.nodes.map(node => Number(net(node, 'wg', 'show', 'wg0', 'transfer').trim().split(/\s+/)[2] ?? 0))
  }
  async transfer(files = false) {
    for (const sender of this.nodes) {
      const receiver = this.nodes[1 - sender.index]
      const content = files ? randomBytes(512 * 1024) : `uc343-canary-${randomUUID()}`
      if (!files) this.canaries.push(content)
      const digest = hash(content)
      const attempt = { phase: this.phase, from: sender.alias, to: receiver.alias, kind: files ? 'file' : 'text', bytes: Buffer.byteLength(content), sha256: digest, receiver_matched: false }
      this.facts.transfers ??= []
      this.facts.transfers.push(attempt)
      const observed = new Map()
      const entryAliases = new Map()
      attempt.readback_observations = { polls: 0, errors: {}, values: [] }
      const response = await sender.raw(files
        ? { command: 'send_file', peer: receiver.id, handle: `synthetic-${randomUUID()}`, display_name: 'synthetic.bin', bytes: Array.from(content) }
        : { command: 'send', peer: receiver.id, text: content })
      attempt.send_error_code = response.code
      attempt.send_totals = Object.fromEntries(Object.entries(response.ok ?? {}).filter(([key, value]) => key.startsWith('total_') && typeof value === 'number'))
      requireFact(!response.error, `transfer_send_failed_code_${response.code ?? 'unknown'}`)
      try { await until(async () => {
        attempt.readback_observations.polls++
        for (const entry of await receiver.call('history')) {
          if (files && entry.content_type !== 'file') continue
          const result = await receiver.raw({ command: files ? 'read_file' : 'entry', entry: entry.entry_id })
          if (!result.ok) {
            const code = Number.isInteger(result.code) ? String(result.code) : 'unknown'
            attempt.readback_observations.errors[code] = (attempt.readback_observations.errors[code] ?? 0) + 1
            continue
          }
          const bytes = files ? Buffer.from(result.ok.bytes) : Buffer.from(result.ok.content ?? '')
          const actual = hash(bytes)
          if (!entryAliases.has(entry.entry_id)) entryAliases.set(entry.entry_id, entryAliases.size + 1)
          const key = `${entryAliases.get(entry.entry_id)}:${actual}`
          if (!observed.has(key)) {
            requireFact(observed.size < 256, 'readback_observation_overflow')
            observed.set(key, true)
            attempt.readback_observations.values.push({ entry_alias: entryAliases.get(entry.entry_id), bytes: bytes.length, sha256: actual, matched: actual === digest })
          }
          if (actual === digest) return true
        }
        return false
      }, files ? 60_000 : 20_000, files ? 'file_readback_deadline' : 'text_readback_deadline')
        attempt.receiver_matched = true
      }
      finally {
        const entry = response.ok?.entry_id
        if (!attempt.receiver_matched && entry) {
          const view = await sender.raw({ command: 'delivery', entry })
          attempt.sender_status = view.ok?.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
        }
      }
      requireFact(response.ok?.entry_id, 'transfer_sender_entry_missing')
      await until(async () => {
        const view = await sender.call('delivery', { entry: response.ok.entry_id })
        attempt.sender_status = view.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
        return attempt.sender_status.length === 1 && attempt.sender_status.every(status => ['delivered', 'duplicate'].includes(status.kind))
      }, 10_000, 'transfer_sender_not_settled')
      this.readbacks.push({ from: sender.alias, to: receiver.alias, kind: files ? 'file' : 'text', bytes: Buffer.byteLength(content), sha256: digest, matched: true })
    }
  }
  async cleanup() {
    let cleaned = true
    for (const node of this.nodes) {
      if (node.ready && node.child && node.child.exitCode === null && node.child.signalCode === null) {
        const observations = [
          ...(node.receiveObserved ? ['receive_snapshot'] : []),
          ...(node.recoveryObserved ? ['recovery_snapshot'] : []),
          ...(node.acquisitionObserved ? ['acquisition_snapshot'] : []),
          ...(node.capabilities.includes('clipboard-stream-fault') && (node.receiveObserved || node.recoveryObserved) ? ['snapshot'] : []),
        ]
        for (const action of observations) {
          try {
            const snapshot = await node.call('stream_fault', { action })
            this.facts.cleanup_observations ??= []
            this.facts.cleanup_observations.push({ node: node.alias, action, snapshot })
          } catch (error) {
            cleaned = false
            this.facts.observation_export_errors ??= []
            this.facts.observation_export_errors.push({ node: node.alias, action, code: error instanceof Failure ? error.message : 'unexpected_export_error' })
          }
        }
      }
      try {
        if (node.subnetProbe && node.child && node.child.exitCode === null && node.child.signalCode === null) {
          this.facts.subnet_probe_snapshots ??= []
          this.facts.subnet_probe_snapshots.push({ node: node.alias, phase: 'before_cleanup', snapshot: await node.call('subnet_probe', { action: 'snapshot' }) })
        }
        if (node.addressProbe && node.child && node.child.exitCode === null && node.child.signalCode === null) {
          const snapshot = await node.call('address_probe', { action: 'snapshot' })
          this.facts.address_probe_snapshots ??= []
          this.facts.address_probe_snapshots.push({ node: node.alias, phase: 'before_cleanup', snapshot })
        }
        if (node.ready && node.child && node.child.exitCode === null && node.child.signalCode === null) await node.drain()
        await node.stop()
      } catch {
        cleaned = false
        if (this.subnetCaptures?.length) this.facts.capture_producer_stop_failed = true
        if (node.child && node.child.exitCode === null && node.child.signalCode === null) {
          const closed = new Promise(resolve => node.child.once('close', resolve))
          node.child.kill('SIGKILL')
          await closed
        }
      }
    }
    try {
      await finishSubnetCaptures(this)
    } catch {
      cleaned = false
      this.facts.subnet_capture_finalization_failed = true
    }
    try {
      this.facts.network_diagnostics = this.nodes.map(node => this.exportNetworkEvidence(node))
    } catch (error) {
      cleaned = false
      this.facts.network_diagnostics_export_error = error instanceof Failure ? error.message : 'unexpected_network_export_error'
    }
    for (const helper of this.helpers ?? []) {
      if (helper.exitCode === null && helper.signalCode === null) { helper.kill('SIGKILL'); await once(helper, 'exit') }
    }
    const leaked = []
    let filesScanned = 0
    const probes = this.canaries.flatMap(value => value.length > 128
      ? [value.slice(0, 64), value.slice(Math.floor(value.length / 2), Math.floor(value.length / 2) + 64), value.slice(-64)]
      : [value])
    const scan = directory => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name)
        if (entry.isDirectory()) scan(path)
        else if (entry.isFile()) {
          filesScanned++
          const data = readFileSync(path)
          for (const canary of probes) if (data.includes(Buffer.from(canary))) leaked.push(hash(path))
        }
      }
    }
    try { scan(this.root) } catch { cleaned = false }
    for (const namespace of this.namespaces.reverse()) {
      const remaining = ip('netns', 'pids', namespace).trim().split(/\s+/).filter(Boolean)
      if (remaining.length) {
        cleaned = false
        for (const pid of remaining) { try { process.kill(Number(pid), 'SIGKILL') } catch {} }
      }
      try { ip('netns', 'delete', namespace) } catch { cleaned = false }
    }
    if (this.bridge) { try { ip('link', 'delete', this.bridge) } catch { cleaned = false } }
    rmSync(this.root, { recursive: true, force: true })
    return { cleaned, plaintext_clean: leaked.length === 0, canary_matches: leaked.length, probes: probes.length, files_scanned: filesScanned }
  }

  exportNetworkEvidence(node) {
    const rows = []
    let bytes = 0
    const visit = directory => {
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name)
        if (entry.isDirectory()) { visit(path); continue }
        if (!entry.isFile() || !entry.name.endsWith('.jsonl')) continue
        for (const line of readFileSync(path, 'utf8').split('\n')) {
          if (!line) continue
          let record
          try { record = JSON.parse(line) } catch { continue }
          const decodedLibrary = /^network\.\d{8}\.\d+\.jsonl$/.test(entry.name) && record.v && typeof record.event === 'string' && typeof record.f === 'object'
          const decodedOwner = [1, 2].includes(record.local_schema_version) && ['uc.connectivity', 'uc.telemetry'].includes(record.target) && typeof record.fields === 'object'
          const decodedModule = record.local_schema_version === 2 && record.source === 'engine_module' && /^(uc_application|uc_infra_p2p|uc_engine)(::|$)/.test(record.target ?? '') && typeof record.fields === 'object'
          if (!decodedLibrary && !decodedOwner && !decodedModule) continue
          bytes += Buffer.byteLength(line) + 1
          requireFact(bytes <= 16 * 1024 * 1024 && rows.length < 100_000, 'network_diagnostic_export_overflow')
          rows.push(JSON.stringify({ node: node.alias, kind: decodedLibrary ? 'decoded_library' : decodedModule ? 'decoded_module' : 'decoded_owner', record }))
        }
      }
    }
    visit(node.root)
    const directory = join(evidence, 'network-diagnostics')
    mkdirSync(directory, { recursive: true, mode: 0o700 })
    const filename = `${this.prefix}-${node.alias}.jsonl`
    const content = rows.join('\n') + (rows.length ? '\n' : '')
    writeFileSync(join(directory, filename), content, { mode: 0o600 })
    return { node: node.alias, file: `network-diagnostics/${filename}`, rows: rows.length, bytes: Buffer.byteLength(content), sha256: hash(content), process_windows: node.processWindows }
  }
}

function publicCandidateLab(run) {
  const [a, b] = run.nodes
  net(a, 'ip', 'addr', 'add', '203.0.113.11/32', 'dev', 'wg0')
  net(b, 'ip', 'route', 'add', '203.0.113.11/32', 'dev', 'wg0')
  const key = net(b, 'wg', 'show', 'wg0', 'peers').trim()
  net(b, 'wg', 'set', 'wg0', 'peer', key, 'allowed-ips', '10.44.0.11/32,203.0.113.11/32')
  net(b, 'ping', '-c', '1', '-W', '3', '203.0.113.11')
  command('ip', ['netns', 'exec', b.namespace, 'nft', '-f', '-'], 'table inet publicproof { chain output { type filter hook output priority -60; policy accept; ip daddr 203.0.113.11 udp dport 23110 counter; ip daddr 203.0.113.11 tcp dport 19090 tcp flags & (syn | ack) == syn counter; }; }\n')
  run.facts.public_candidate_reachable = true
}
function publicCandidateCounters(node) {
  return JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'publicproof')).nftables
    .flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).map(expr => expr.counter.packets)
}

async function learnedAddressReconnect(run, configuredControl) {
  await run.pair()
  const [a, b] = run.nodes
  if (addressProbeEnabled && a.capabilities.includes('network-test-probe')) await a.call('address_probe', { action: 'install', peer: b.id })
  const oldPort = listenerPort(b)
  await b.settings({ bind_port: 0 })
  await b.stop()
  requireFact((await b.start()).ready, 'changed_port_restart_failed')
  const currentPort = listenerPort(b)
  if (addressProbeEnabled && a.capabilities.includes('network-test-probe')) {
    const socket = `10.44.0.12:${currentPort}`
    await a.call('address_probe', { action: 'expect', socket })
    a.addressProbe = { peer: b.id, socket }
  }
  run.facts.listener_ports_are_distinct = oldPort !== currentPort
  environment(oldPort !== currentPort, 'random_port_did_not_change')
  await run.online()
  await run.transfer()
  if (configuredControl) await a.settings({ peer_addresses: { [b.id]: { ip: '10.44.0.12', port: currentPort } } })
  await a.call('flush')
  if (a.addressProbe) {
    run.facts.address_probe_snapshots ??= []
    run.facts.address_probe_snapshots.push({ node: 'A', phase: 'before_restart', snapshot: await a.call('address_probe', { action: 'snapshot' }) })
  }
  await a.stop()
  await until(async () => !(await b.call('peers')).some(peer => peer.connected), 25_000, 'peer_stop_not_observed')
  blockNewPeerDials(run, b)
  run.facts.correct_configured_hint_control = configuredControl
  requireFact((await a.start()).ready, 'learned_address_profile_restart_failed')
  await run.online()
  await run.transfer()
  await run.transfer(true)
  run.facts.fresh_process_reconnected_to_current_port = true
}

async function transportRegression(run, mode, legacySide) {
  const label = mode === 'legacy' ? `legacy-${legacySide}` : mode
  const directory = join(evidence, 'regression', run.prefix, label)
  mkdirSync(directory, { recursive: true, mode: 0o700 })
  const argv = ['scripts/testing/connection-recovery-network.mjs', '--host', regressionHost, '--mode', mode, '--repeat', '1', '--evidence', directory]
  if (mode === 'relay') argv.push('--relay', relayBinary)
  if (mode === 'legacy') argv.push('--legacy-host', legacyBinary, '--legacy-side', String(legacySide))
  const child = spawn(process.execPath, argv, { stdio: ['ignore', 'pipe', 'pipe'] })
  run.helpers ??= []
  run.helpers.push(child)
  let output = ''
  let overflow = false
  const capture = chunk => {
    if (Buffer.byteLength(output) + chunk.length <= 256 * 1024) output += chunk.toString()
    else overflow = true
  }
  child.stdout.on('data', capture)
  child.stderr.on('data', capture)
  const deadline = setTimeout(() => child.kill('SIGTERM'), 25 * 60_000)
  let code
  try { [code] = await once(child, 'close') }
  finally { clearTimeout(deadline); writeFileSync(join(directory, 'console.log'), output, { mode: 0o600 }) }
  requireFact(!overflow, 'transport_regression_output_overflow')
  const report = JSON.parse(readFileSync(join(directory, `${label}.json`), 'utf8'))
  run.facts.transport_regressions ??= []
  run.facts.transport_regressions.push({ mode, legacy_side: legacySide, exit_code: code, artifact: `regression/${run.prefix}/${label}/${label}.json`, binary_sha256: report.binaries.map(value => value.sha256), cases: report.records.map(value => ({ id: value.id, outcome: value.outcome })), cleaned: report.cleaned, plaintext_clean: report.plaintext_clean })
  const expected = {
    direct: ['E01-complete-pairing', 'E02-text-transfer', 'E02-file-transfer', 'E14-concurrent-clipboard', 'E03-0', 'E05-0', 'E04-no-opportunity-0', 'E04-opportunity-0', 'E06-0', 'E11-0', 'E12-0'],
    'known-peer': ['E13-known-peer-contact-0'],
    relay: ['E01-complete-pairing', 'E02-text-transfer', 'E02-file-transfer', 'E10-direct-0', 'E02-relay-only-text-transfer', 'E02-relay-only-file-transfer', 'E14-concurrent-clipboard-relay-only', 'E10-relay-only-0', 'E15-pairing-current-address'],
    legacy: [legacySide === 0 ? 'E09-legacy-pairing-rejected' : 'E09-current-invitation-rejected-by-legacy'],
  }[mode]
  requireFact(code === 0 && !report.failed && report.cleaned && report.plaintext_clean, 'existing_transport_regression_failed')
  requireFact(expected.every(id => report.records.some(value => value.id === id && value.outcome === 'passed')) && report.records.every(value => value.outcome === 'passed'), 'transport_regression_case_coverage_missing')
  const required = [regressionHost, ...(mode === 'relay' ? [relayBinary] : []), ...(mode === 'legacy' ? [legacyBinary] : [])]
  requireFact(required.every(path => report.binaries.some(value => value.sha256 === hash(readFileSync(path)))), 'transport_regression_binary_identity_mismatch')
}

const scenarios = [
  { id: 'L01', topology: 'vpn', async run(run) {
    await run.pair()
    await run.transfer()
    const before = run.counters()
    await run.transfer(true)
    const delta = run.counters().map((value, index) => value - before[index])
    run.facts.wireguard_tx_growth = delta
    requireFact(delta.every(value => value >= 512 * 1024), 'wireguard_payload_counter_short')
  } },
  { id: 'L02', topology: 'vpn', async run(run) {
    await run.pair()
    const durations = []
    for (const node of run.nodes) {
      await node.stop()
      requireFact((await node.start()).ready, 'fixed_port_restart_failed')
      durations.push(await run.online())
      await run.transfer()
      await run.transfer(true)
    }
    run.facts.passive_reconnect_ms = durations
  } },
  { id: 'L03', topology: 'vpn', async run(run) {
    await learnedAddressReconnect(run, false)
  } },
  { id: 'L03C', topology: 'vpn', async run(run) {
    await learnedAddressReconnect(run, true)
  } },
  { id: 'L04', topology: 'lan', async run(run) {
    await run.pair()
    await run.changedPorts()
    run.facts.reconnect_ms = await run.online()
    await run.transfer()
    await run.transfer(true)
    run.facts.both_stored_ports_stale = true
  } },
  { id: 'L05', topology: 'vpn', async run(run) {
    await run.pair()
    await run.changedPorts()
    await delay(5_000)
    requireFact((await run.nodes[0].call('peers')).every(peer => !peer.connected), 'reconnected_without_an_address_source')
    const [a, b] = run.nodes
    await a.settings({ peer_addresses: { [b.id]: { ip: '10.44.0.12', port: 23211 } } })
    run.facts.configured_hint_reconnect_ms = await run.online()
    await run.transfer()
    await run.transfer(true)
    run.facts.applied_without_restart = true
  } },
  { id: 'L06', topology: 'lan', async run(run) {
    const [a] = run.nodes
    run.created = await a.call('create', { name: 'A' })
    await a.stop()
    const blocker = await holdUdp(run, a, 23110)
    const started = await a.start()
    requireFact(!started.ready && started.code, 'occupied_fixed_port_did_not_fail_start')
    run.facts.occupied_port_error_code = started.code
    await a.stop()
    run.facts.failed_start_exited_without_fallback = true
    blocker.kill('SIGKILL')
    await once(blocker, 'exit')
    requireFact((await a.start()).ready, 'released_port_restart_failed')
    requireFact(net(a, 'ss', '-Hlun').includes(':23110'), 'saved_port_not_bound')
    run.facts.fixed_port_bound_after_release = true
    await run.pair()
    await run.transfer()
    await run.transfer(true)
  } },
  { id: 'S01', topology: 'local', async run(run) {
    const [a] = run.nodes
    const one = '00000000-0000-4000-8000-000000000001'
    const two = '00000000-0000-4000-8000-000000000002'
    const first = { ip: '10.44.0.12', port: 23111 }
    const second = { ip: 'fd00::2', port: 23112 }
    await a.settings({ peer_addresses: { [one]: first, [two]: second } })
    await a.settings({ peer_addresses: { [one]: { ...first, port: 23113 } } })
    let saved = (await a.call('settings')).network
    requireFact(saved.peer_addresses[two]?.ip === second.ip && saved.peer_addresses[one]?.port === 23113, 'patch_changed_unlisted_peer')
    for (const invalid of [{ ip: 'host.invalid', port: 2 }, { ip: 'broken-ip', port: 2 }, { ip: '10.0.0.1', port: 0 }]) {
      const before = JSON.stringify((await a.call('settings')).network)
      const result = await a.call('settings_update', { patch: { network: { bind_port: 24567, peer_addresses: { [one]: invalid, [two]: first } } } })
      requireFact(result.Rejected, 'invalid_address_accepted')
      requireFact(JSON.stringify((await a.call('settings')).network) === before, 'rejected_patch_not_atomic')
    }
    for (const device of ['', 'x'.repeat(1024)]) {
      const before = JSON.stringify((await a.call('settings')).network)
      const result = await a.call('settings_update', { patch: { network: { peer_addresses: { [device]: first } } } })
      requireFact(result.Rejected && JSON.stringify((await a.call('settings')).network) === before, 'invalid_device_key_not_atomic')
    }
    await a.settings({ peer_addresses: { [one]: null } })
    saved = (await a.call('settings')).network
    requireFact(!saved.peer_addresses[one] && saved.peer_addresses[two]?.ip === second.ip, 'delete_changed_other_peer')
    run.facts.settings_atomic_patch_and_delete = true
  } },
  { id: 'S02', topology: 'vpn', async run(run) {
    await run.pair()
    const [a, b] = run.nodes
    await b.stop()
    // 相同地址和端口上的另一个真实 Engine 身份不能冒充原设备。
    const previousRoot = b.root
    const previousStorage = b.storage
    b.root = join(run.root, 'unrelated')
    b.storage = undefined
    requireFact((await b.start()).ready, 'unrelated_engine_start_failed')
    await b.settings({ allow_relay_fallback: false, bind_port: 23111 })
    await b.call('create', { name: 'unrelated' })
    await a.settings({ peer_addresses: { [b.id]: { ip: '10.44.0.12', port: 23111 } } })
    await a.stop()
    requireFact((await a.start()).ready, 'identity_probe_restart_failed')
    const text = `uc343-identity-${randomUUID()}`
    run.canaries.push(text)
    const response = await a.raw({ command: 'send', peer: b.id, text })
    requireFact(response.error || response.ok?.total_accepted === 0, 'wrong_identity_accepted_delivery')
    await delay(5_000)
    requireFact(!(await a.call('peers')).some(peer => peer.peer_id === b.id && peer.connected), 'wrong_identity_marked_online')
    requireFact((await b.call('history')).length === 0, 'wrong_identity_received_content')
    run.facts.wrong_identity_no_online_or_delivery = true
    await b.stop()
    b.root = previousRoot
    b.storage = previousStorage
  } },
  { id: 'S03C', topology: 'vpn', async run(run) {
    await run.pair()
    const [a, b] = run.nodes
    await b.stop()
    // 受控公网地址在未设默认路由的测试命名空间内可达，避免防火墙替 Engine 证明过滤。
    net(a, 'ip', 'route', 'add', '203.0.113.12/32', 'dev', 'wg0')
    const key = net(a, 'wg', 'show', 'wg0', 'peers').trim()
    net(a, 'wg', 'set', 'wg0', 'peer', key, 'allowed-ips', '10.44.0.12/32,203.0.113.12/32')
    net(b, 'ip', 'addr', 'add', '203.0.113.12/32', 'dev', 'wg0')
    net(a, 'ping', '-c', '1', '-W', '3', '203.0.113.12')
    command('ip', ['netns', 'exec', a.namespace, 'nft', '-f', '-'], 'table inet policyproof { chain output { type filter hook output priority -60; policy accept; ip daddr 203.0.113.12 udp dport 23999 counter; }; }\n')
    await a.settings({ peer_addresses: { [b.id]: { ip: '203.0.113.12', port: 23999 } } })
    await a.stop()
    requireFact((await a.start()).ready, 'policy_probe_restart_failed')
    const text = `uc343-policy-${randomUUID()}`
    run.canaries.push(text)
    await a.raw({ command: 'send', peer: b.id, text })
    const rules = JSON.parse(net(a, 'nft', '-j', 'list', 'table', 'inet', 'policyproof'))
    const counters = rules.nftables.flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).map(expr => expr.counter.packets)
    requireFact(counters.length === 1 && counters[0] === 0, 'public_configured_hint_was_dialed')
    run.facts.public_hint_probe = { reachable_control: true, outbound_udp_packets: counters[0] }
    await a.settings({ allow_relay_fallback: true })
    await a.stop()
    requireFact((await a.start()).ready, 'default_mode_control_start_failed')
    const overview = await a.call('relay_overview')
    requireFact(overview.applied_mode !== 'Disabled', 'default_mode_control_not_applied')
    const controlText = `uc343-policy-control-${randomUUID()}`
    run.canaries.push(controlText)
    await a.raw({ command: 'send', peer: b.id, text: controlText })
    await until(() => {
      const control = JSON.parse(net(a, 'nft', '-j', 'list', 'table', 'inet', 'policyproof'))
      const packets = control.nftables.flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).reduce((sum, expr) => sum + expr.counter.packets, 0)
      run.facts.public_hint_probe.default_mode_packets = packets
      return Promise.resolve(packets > 0)
    }, 20_000, 'public_hint_positive_control_did_not_dial')
    run.facts.configured_public_hint_filtered = true
  } },
  { id: 'S03A', topology: 'vpn', async run(run) {
    environment(invitationFixture, 'invitation_fixture_required')
    const [a, b] = run.nodes
    for (const node of run.nodes) {
      const peer = run.nodes[1 - node.index]
      net(node, 'ip', 'addr', 'add', `100.101.0.${11 + node.index}/24`, 'dev', 'wg0')
      net(node, 'ip', '-6', 'addr', 'add', `fd7a:115c:a1e0:343::${11 + node.index}/64`, 'dev', 'wg0', 'nodad')
      net(node, 'wg', 'set', 'wg0', 'peer', net(node, 'wg', 'show', 'wg0', 'peers').trim(), 'allowed-ips', `10.44.0.${11 + peer.index}/32,100.101.0.${11 + peer.index}/32,fd7a:115c:a1e0:343::${11 + peer.index}/128`)
      await node.settings({ allow_overlay_network_addrs: true })
      await node.stop()
    }
    net(a, 'ping', '-c', '1', '-W', '3', '100.101.0.12')
    net(a, 'ping', '-6', '-c', '1', '-W', '3', 'fd7a:115c:a1e0:343::12')
    for (const node of run.nodes) requireFact((await node.start()).ready, 'overlay_policy_start_failed')
    run.invitationTransform = { replace: true, direct: ['100.101.0.11:23110'], contains_direct: ['100.101.0.11:23110'] }
    await run.pair()
    await run.transfer()
    await run.transfer(true)
    run.facts.cgnat_invitation_readback = true
    await run.changedPorts()
    // 旧端口失效且组播被阻断，只有新配置的 ULA 候选可以找到 A。
    await b.settings({ peer_addresses: { [a.id]: { ip: 'fd7a:115c:a1e0:343::11', port: 23210 } } })
    await run.online()
    await run.transfer()
    await run.transfer(true)
    run.facts.ula_configured_hint_readback = true
    run.facts.overlay_addresses_explicitly_enabled = true
  } },
  { id: 'S03I', topology: 'vpn', async run(run) {
    publicCandidateLab(run)
    const [a, b] = run.nodes
    net(a, 'ip', 'addr', 'add', '198.18.0.11/32', 'dev', 'wg0')
    net(a, 'ip', '-6', 'addr', 'add', '2001:db8:343:1::11/64', 'dev', 'wg0', 'nodad')
    net(b, 'ip', '-6', 'addr', 'add', '2001:db8:343:2::12/64', 'dev', 'wg0', 'nodad')
    net(b, 'ip', 'route', 'add', '198.18.0.11/32', 'dev', 'wg0')
    net(b, 'ip', '-6', 'route', 'add', '2001:db8:343:1::11/128', 'dev', 'wg0')
    net(a, 'ip', '-6', 'route', 'add', '2001:db8:343:2::12/128', 'dev', 'wg0')
    net(b, 'wg', 'set', 'wg0', 'peer', net(b, 'wg', 'show', 'wg0', 'peers').trim(), 'allowed-ips', '10.44.0.11/32,203.0.113.11/32,198.18.0.11/32,2001:db8:343:1::11/128')
    net(a, 'wg', 'set', 'wg0', 'peer', net(a, 'wg', 'show', 'wg0', 'peers').trim(), 'allowed-ips', '10.44.0.12/32,2001:db8:343:2::12/128')
    net(b, 'ping', '-c', '1', '-W', '3', '198.18.0.11')
    net(b, 'ping', '-6', '-c', '1', '-W', '3', '2001:db8:343:1::11')
    command('ip', ['netns', 'exec', b.namespace, 'nft', '-f', '-'], 'add rule inet publicproof output ip daddr 198.18.0.11 udp dport 23110 counter\nadd rule inet publicproof output ip6 daddr 2001:db8:343:1::11 udp dport 23110 counter\n')
    const direct = ['203.0.113.11:23110', '198.18.0.11:23110', '[2001:db8:343:1::11]:23110']
    run.invitationTransform = { direct, relays: ['http://203.0.113.11:19090'], contains_direct: direct }
    run.facts.invitation_policy_phases = [{ phase: 'before_admission', packets: publicCandidateCounters(b) }]
    await run.pair()
    run.facts.invitation_policy_phases.push({ phase: 'admitted', packets: publicCandidateCounters(b) })
    await run.transfer()
    run.facts.invitation_policy_phases.push({ phase: 'text_readback', packets: publicCandidateCounters(b) })
    await run.transfer(true)
    run.facts.invitation_policy_phases.push({ phase: 'file_readback', packets: publicCandidateCounters(b) })
    const packets = publicCandidateCounters(run.nodes[1])
    run.facts.invitation_policy = { public_udp_packets: packets[0], relay_tcp_syn_packets: packets[1], fake_ip_udp_packets: packets[2], off_subnet_ipv6_udp_packets: packets[3], private_readback: true }
    requireFact(packets.length === 4 && packets.every(count => count === 0), 'invitation_non_local_candidate_dialed')
  } },
  { id: 'S03P', topology: 'vpn', async run(run) {
    publicCandidateLab(run)
    const [a, b] = run.nodes
    await b.settings({ allow_relay_fallback: true })
    await b.stop()
    b.defaultMode = true
    requireFact((await b.start()).ready, 'public_store_control_start_failed')
    run.invitationTransform = { replace: true, direct: ['203.0.113.11:23110'], contains_direct: ['203.0.113.11:23110'] }
    await run.pair()
    await run.transfer()
    environment(publicCandidateCounters(b)[0] > 0, 'public_invitation_positive_control_not_hit')
    await b.stop()
    const controlBefore = publicCandidateCounters(b)[0]
    command('ip', ['netns', 'exec', a.namespace, 'nft', '-f', '-'], 'table inet storeddirection { chain output { type filter hook output priority -60; policy accept; ip daddr 10.44.0.12 meta l4proto udp ct state new counter drop; }; }\n')
    net(a, 'conntrack', '-F')
    requireFact((await b.start()).ready, 'stored_public_default_restart_failed')
    await run.online()
    await run.transfer()
    environment(publicCandidateCounters(b)[0] > controlBefore, 'stored_public_default_control_not_hit')
    await b.settings({ allow_relay_fallback: false })
    await b.stop()
    await a.stop()
    // 原进程退出后只观察新进程的出站拨号，避免远端主动接入绕过出站过滤。
    const packetsBefore = publicCandidateCounters(b)
    const before = run.diagnostics('B')
    b.defaultMode = false
    requireFact((await b.start()).ready, 'stored_public_lan_start_failed')
    const text = `uc343-stored-policy-${randomUUID()}`
    run.canaries.push(text)
    await b.raw({ command: 'send', peer: a.id, text })
    await b.call('flush')
    const after = run.diagnostics('B')
    const packets = publicCandidateCounters(b).map((count, index) => count - packetsBefore[index])
    run.facts.stored_policy_observation = { before, after, public_udp_packets: packets[0] }
    requireFact(packets.every(count => count === 0), 'stored_public_hint_was_dialed')
    environment(after.address_loaded > before.address_loaded && after.non_local_candidates_dropped > before.non_local_candidates_dropped, 'stored_public_input_filter_evidence_missing')
    run.facts.stored_policy = { default_public_transfer: true, fresh_process_public_transfer: true, loaded_record_observed: true, candidates_dropped: after.non_local_candidates_dropped - before.non_local_candidates_dropped, public_udp_packets: packets[0] }
  } },
  { id: 'S03D', topology: 'lan', async run(run) {
    await run.pair()
    const [a, b] = run.nodes
    environment(invitationFixture, 'invitation_fixture_required')
    for (const node of run.nodes) {
      net(node, 'ip', '-6', 'addr', 'add', `2001:db8:343:${1 + node.index}::${11 + node.index}/64`, 'dev', 'eth0', 'nodad')
      net(node, 'ip', '-6', 'route', 'add', `2001:db8:343:${2 - node.index}::${12 - node.index}/128`, 'dev', 'eth0')
    }
    net(b, 'ping', '-6', '-c', '1', '-W', '3', '2001:db8:343:1::11')
    await a.settings({ bind_port: 23210 })
    await a.stop()
    // A 不能主动连接 B；B 必须从真实 mDNS 获得 A 的新端口。
    command('ip', ['netns', 'exec', a.namespace, 'nft', '-f', '-'], 'table inet directionproof { chain output { type filter hook output priority -60; policy accept; ip daddr 10.233.0.12 meta l4proto udp ct state new counter drop; ip6 daddr 2001:db8:343:2::12 meta l4proto udp ct state new counter drop; }; }\n')
    command('ip', ['netns', 'exec', b.namespace, 'nft', '-f', '-'], 'table inet discoveryproof { chain output { type filter hook output priority -60; policy accept; ip6 daddr 2001:db8:343:1::11 udp dport 23210 counter; ip daddr 10.233.0.11 udp dport 23210 counter; }; }\n')
    await b.stop()
    requireFact((await a.start()).ready, 'mdns_policy_source_start_failed')
    const invitation = await a.call('invite')
    run.canaries.push(invitation.invitation)
    const check = JSON.parse(command(invitationFixture, [], JSON.stringify({ invitation: invitation.invitation, contains_direct: ['[2001:db8:343:1::11]:23210'] })))
    environment(check.contains_direct[0], 'mdns_off_subnet_candidate_not_advertised')
    requireFact((await b.start()).ready, 'mdns_policy_receiver_start_failed')
    await run.online()
    await run.transfer()
    await run.transfer(true)
    await b.call('flush')
    const packets = JSON.parse(net(b, 'nft', '-j', 'list', 'table', 'inet', 'discoveryproof')).nftables.flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).map(expr => expr.counter.packets)
    run.facts.discovered_policy_observation = { off_subnet_udp_packets: packets[0], private_udp_packets: packets[1] }
    run.facts.discovered_policy_input = { mdns_multi_address_result: run.diagnostics('B').mdns_multi_address_result }
    environment(run.facts.discovered_policy_input.mdns_multi_address_result > 0, 'mdns_candidate_input_not_observed')
    requireFact(packets[0] === 0 && packets[1] > 0, 'discovered_off_subnet_candidate_was_dialed')
    run.facts.discovered_policy = { advertised_off_subnet_candidate: true, mdns_multiple_candidates: true, off_subnet_udp_packets: packets[0], private_udp_packets: packets[1] }
  } },
  { id: 'S04', topology: 'lan', async run(run) {
    environment(regressionHost && relayBinary && legacyBinary, 'transport_regression_binaries_required')
    for (const node of run.nodes) {
      await node.settings({ allow_relay_fallback: true })
      await node.stop()
      node.defaultMode = true
      requireFact((await node.start()).ready, 'default_transport_control_start_failed')
    }
    await run.pair()
    await run.transfer()
    await run.transfer(true)
    for (const node of run.nodes) await node.stop()
    for (const mode of ['direct', 'known-peer', 'relay']) await transportRegression(run, mode)
    for (const side of [0, 1]) await transportRegression(run, 'legacy', side)
  } },
  { id: 'S05', topology: 'local', async run(run) {
    const [a] = run.nodes
    environment(baselineBinary, 'baseline_host_required')
    for (const node of run.nodes) {
      await node.stop()
      rmSync(node.root, { recursive: true, force: true })
      node.storage = undefined
      node.binaryOverride = baselineBinary
      requireFact((await node.start()).ready, 'baseline_profile_start_failed')
      await node.settings({ allow_relay_fallback: false })
      await node.stop()
      requireFact((await node.start()).ready, 'baseline_lan_profile_start_failed')
    }
    await run.pair()
    await run.transfer()
    const text = `uc343-upgrade-${randomUUID()}`
    run.canaries.push(text)
    await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now() })
    for (const node of run.nodes) await node.stop()
    for (const node of run.nodes) {
      node.binaryOverride = undefined
      requireFact((await node.start()).ready, 'baseline_profile_upgrade_failed')
      await ensureSessionReady(node)
    }
    await run.online()
    await run.transfer()
    const history = await a.call('history')
    let readable = false
    for (const entry of history) if ((await a.call('entry', { entry: entry.entry_id })).content === text) readable = true
    requireFact(readable, 'baseline_encrypted_content_unreadable')
    const before = await a.call('settings')
    await a.settings({ bind_port: 24123, peer_addresses: { 'e2e-device': { ip: '10.0.0.2', port: 24124 } } })
    await a.stop()
    requireFact((await a.start()).ready, 'settings_profile_reopen_failed')
    await ensureSessionReady(a)
    const after = await a.call('settings')
    requireFact(after.network.bind_port === 24123 && after.network.peer_addresses['e2e-device']?.port === 24124, 'settings_not_persisted')
    requireFact(after.schema_version === before.schema_version, 'unexpected_schema_change')
    run.facts.candidate_profile_reopen = true
    run.facts.baseline_encrypted_content_readback = true
    run.facts.baseline_binary_sha256 = hash(readFileSync(baselineBinary))
    await a.stop()
    a.binaryOverride = baselineBinary
    const downgrade = await a.start()
    run.facts.downgrade = { ready: downgrade.ready, code: downgrade.code }
    if (downgrade.ready) {
      await ensureSessionReady(a)
      let found = false
      for (const entry of await a.call('history')) if ((await a.call('entry', { entry: entry.entry_id })).content === text) found = true
      requireFact(found, 'downgrade_lost_history')
      run.facts.downgrade.encrypted_content_readback = true
    }
  } },
  { id: 'M01', topology: 'lan', async run(run) {
    for (const node of run.nodes) {
      net(node, 'ip', 'link', 'add', 'default0', 'type', 'dummy')
      net(node, 'ip', 'addr', 'add', `192.168.${node.index + 90}.2/24`, 'dev', 'default0')
      net(node, 'ip', 'link', 'set', 'default0', 'up')
      net(node, 'ip', 'route', 'add', 'default', 'dev', 'default0')
    }
    await run.pair()
    await run.changedPorts()
    run.facts.non_default_nic_ipv4_reconnect_ms = await run.online()
    await run.transfer()
    for (const node of run.nodes) {
      net(node, 'ip', '-6', 'addr', 'add', `2001:db8:343::${11 + node.index}/64`, 'dev', 'eth0', 'nodad')
      command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], `table inet v6proof { chain output { type filter hook output priority -40; policy accept; ip daddr 10.233.0.${12 - node.index} counter drop; }; }\n`)
      await node.settings({ bind_port: 23310 + node.index })
      await node.stop()
    }
    for (const node of run.nodes) requireFact((await node.start()).ready, 'ipv6_restart_failed')
    run.facts.ipv6_only_reconnect_ms = await run.online()
    await run.transfer()
    await run.transfer(true)
    run.facts.ipv4_peer_unicast_blocked = true
  } },
  { id: 'M02', topology: 'lan', async run(run) {
    await run.pair()
    for (const node of run.nodes) {
      net(node, 'ip', 'addr', 'add', `10.233.1.${11 + node.index}/24`, 'dev', 'eth0')
      net(node, 'ip', 'addr', 'delete', `10.233.0.${11 + node.index}/24`, 'dev', 'eth0')
    }
    run.phase = 'retained_address'
    for (const node of run.nodes) net(node, 'ping', '-c', '1', '-W', '3', `10.233.1.${12 - node.index}`)
    run.facts.retained_address_network_preflight = true
    run.facts.retained_address_reconnect_ms = await run.online(75_000)
    await run.transfer()
    await run.transfer(true)
  } },
  { id: 'M02R', topology: 'lan', async run(run) {
    await run.pair()
    run.phase = 'replacement_nic'
    for (const node of run.nodes) net(node, 'ip', 'link', 'delete', 'eth0')
    await delay(2_000)
    for (const node of run.nodes) {
      const veth = `${run.prefix}${node.index}r`.slice(0, 15)
      ip('link', 'add', veth, 'type', 'veth', 'peer', 'name', 'eth1', 'netns', node.namespace)
      ip('link', 'set', veth, 'master', run.bridge)
      ip('link', 'set', veth, 'up')
      net(node, 'ip', 'addr', 'add', `10.234.0.${11 + node.index}/24`, 'dev', 'eth1')
      net(node, 'ip', 'link', 'set', 'eth1', 'up')
    }
    for (const node of run.nodes) net(node, 'ping', '-c', '1', '-W', '3', `10.234.0.${12 - node.index}`)
    run.facts.replacement_nic_network_preflight = true
    run.facts.replacement_nic_reconnect_ms = await run.online(75_000)
    await run.transfer()
    await run.transfer(true)
    run.facts.real_interface_deleted_and_replaced = true
  } },
  { id: 'M03', topology: 'lan', async run(run) {
    const blockers = run.mdnsBlockers
    run.created = await run.nodes[0].call('create', { name: 'A' })
    const unavailable = await run.nodes[0].raw({ command: 'invite' })
    run.facts.invitation_during_discovery_fault = { available: Boolean(unavailable.ok), code: unavailable.code }
    if (unavailable.ok?.invitation) run.canaries.push(unavailable.ok.invitation)
    else requireFact(Number.isInteger(unavailable.code), 'discovery_fault_has_no_stable_invitation_error')
    await delay(1_000)
    const failed = run.diagnostics()
    run.facts.mdns_before_release = failed
    environment(failed.mdns_initial_failure + failed.mdns_interface_retry > 0, 'mdns_socket_fault_not_observed')
    for (const blocker of blockers) { blocker.kill('SIGKILL'); await once(blocker, 'exit') }
    run.facts.discovery_recovery_ms = await until(() => Promise.resolve(run.diagnostics().mdns_recovered > failed.mdns_recovered), 75_000, 'mdns_did_not_recover_without_restart')
    run.facts.mdns_after_release = run.diagnostics()
    await run.pair()
    run.phase = 'discovery_recovered_without_restart'
    await run.transfer()
    await run.transfer(true)
    run.facts.no_restart_discovery_readback = true
    if (run.id === 'M03') return
    await run.changedPorts()
    run.phase = 'later_port_change_restart'
    await run.online(75_000)
    await run.transfer()
    await run.transfer(true)
    run.facts.exclusive_mdns_port_held_and_released = true
    run.facts.mdns_after_release = run.diagnostics()
  } },
  { id: 'M04', topology: 'lan', async run(run) {
    const [a] = run.nodes
    const blocker = run.mdnsBlockers[0]
    await a.call('create', { name: 'A' })
    await delay(1_000)
    const failed = run.diagnostics()
    run.facts.mdns_fault = failed
    await a.stop()
    blocker.kill('SIGKILL')
    await once(blocker, 'exit')
    await delay(3_000)
    requireFact(!net(a, 'ss', '-Hlun').includes(':5353'), 'mdns_listener_after_shutdown')
    run.facts.no_listener_after_shutdown = true
    environment(failed.mdns_initial_failure + failed.mdns_interface_retry > 0, 'mdns_pending_retry_not_observed')
  } },
]

scenarios.push({ id: 'M03R', topology: 'lan', run: scenarios.find(scenario => scenario.id === 'M03').run })

scenarios.push({ id: 'M03V', topology: 'lan', async run(run) {
  // 失败方式：无 IPv4 默认成员时选择无效发送器；冷启动后只保留旧端口；IPv6 仅单向可用。
  for (const node of run.nodes) {
    await node.stop()
    net(node, 'ip', '-4', 'addr', 'flush', 'dev', 'eth0')
    net(node, 'ip', '-6', 'addr', 'add', `2001:db8:343::${11 + node.index}/64`, 'dev', 'eth0', 'nodad')
    net(node, 'ip', '-6', 'route', 'replace', 'ff00::/8', 'dev', 'eth0')
    const control = JSON.parse(net(node, 'python3', '-c', 'import socket,json,errno;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.bind(("0.0.0.0",0));r={"wildcard_bound":True};\ntry:s.setsockopt(socket.IPPROTO_IP,socket.IP_ADD_MEMBERSHIP,socket.inet_aton("224.0.0.251")+socket.inet_aton("0.0.0.0"));r["default_join_failed"]=False\nexcept OSError as e:r.update(default_join_failed=True,error_number=e.errno)\nprint(json.dumps(r))'))
    environment(control.wildcard_bound && control.default_join_failed, 'ipv6_only_default_v4_failure_not_established')
    run.facts.ipv6_only_bootstrap_controls ??= []
    run.facts.ipv6_only_bootstrap_controls.push({ node: node.alias, ...control })
  }
  for (const node of run.nodes) {
    net(node, 'ping', '-6', '-c', '1', '-W', '3', `2001:db8:343::${12 - node.index}`)
    requireFact((await node.start()).ready, 'ipv6_only_start_failed')
  }
  run.invitationTransform = { replace: true, direct: ['[2001:db8:343::11]:23110'], contains_direct: ['[2001:db8:343::11]:23110'] }
  await run.pair()
  run.phase = 'ipv6_only_initial'
  await run.transfer()
  await run.transfer(true)
  const discovered = run.diagnostics().mdns_candidate_result
  await run.changedPorts()
  await run.online(75_000)
  run.phase = 'ipv6_only_changed_ports_cold_restart'
  await run.transfer()
  await run.transfer(true)
  requireFact(run.diagnostics().mdns_candidate_result > discovered, 'ipv6_only_cold_discovery_result_missing')
} })

async function subnetState(run, node) {
  const snapshot = await node.call('subnet_probe', { action: 'snapshot' })
  requireFact(snapshot && snapshot.dropped_events === 0 && !snapshot.events.some(event => event.stage === 'hold_timed_out'), 'subnet_probe_evidence_invalid')
  run.facts.subnet_latest = snapshot
  return snapshot
}

async function armSubnetRead(run, node, fault) {
  const before = await subnetState(run, node)
  const sequence = before.events.at(-1)?.sequence ?? 0
  requireFact((await node.call('subnet_probe', { action: 'arm', fault })).accepted, 'subnet_read_fault_not_armed')
  const state = await subnetState(run, node)
  const armed = state.events.find(event => event.sequence > sequence && event.stage === 'fault_armed' && event.owner_alias === before.owner_alias && event.fault === fault)
  requireFact(armed && state.owner_alias === before.owner_alias, 'exact_subnet_fault_arm_missing')
  run.facts.subnet_fault_arms ??= []
  run.facts.subnet_fault_arms.push(armed)
  return armed
}

async function subnetMark(node, marker) {
  requireFact((await node.call('subnet_probe', { action: 'mark', marker })).accepted, 'subnet_marker_rejected')
}

function subnetPackets(node) {
  const counters = JSON.parse(net(node, 'nft', '-j', 'list', 'table', 'inet', 'subnetproof')).nftables.flatMap(row => row.rule?.expr ?? []).filter(expr => expr.counter).map(expr => expr.counter.packets)
  requireFact(counters.length === 3, 'subnet_packet_counters_missing')
  return { global: counters[0], fixed: counters[1] + counters[2] }
}

function blockFixedPaths(node, enabled) {
  if (!enabled) { net(node, 'nft', 'delete', 'table', 'inet', 'fixedblock'); return }
  command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], `table inet fixedblock { chain output { type filter hook output priority -45; policy accept; ip daddr 10.233.0.${12 - node.index} meta l4proto udp counter drop; ip6 daddr fe80::/10 meta l4proto udp counter drop; }; }\n`)
}

async function prepareSubnetLab(run, capture = true) {
  const [a, b] = run.nodes
  environment(a.capabilities.includes('network-test-probe'), 'subnet_probe_host_required')
  for (const node of run.nodes) {
    net(node, 'ip', '-6', 'addr', 'add', `2001:db8:343::${11 + node.index}/64`, 'dev', 'eth0', 'nodad')
    net(node, 'ip', '-6', 'route', 'replace', `2001:db8:343::${12 - node.index}/128`, 'dev', 'eth0')
    blockFixedPaths(node, true)
    if (capture) await startSubnetCapture(run, node)
    command('ip', ['netns', 'exec', node.namespace, 'nft', '-f', '-'], `table inet subnetproof { chain output { type filter hook output priority -60; policy accept; ip6 daddr 2001:db8:343::${12 - node.index} meta l4proto udp counter${capture ? ' log group 343 snaplen 96 queue-threshold 1' : ''}; ip daddr 10.233.0.${12 - node.index} meta l4proto udp counter; ip6 daddr fe80::/10 meta l4proto udp counter; }; }\n`)
    await node.stop()
  }
  for (const node of run.nodes) net(node, 'ping', '-6', '-c', '1', '-W', '3', `2001:db8:343::${12 - node.index}`)
  a.subnetProbe = { expected: '2001:db8:343::12' }
  for (const node of run.nodes) requireFact((await node.start()).ready, 'subnet_profile_start_failed')
  const initial = await subnetState(run, a)
  requireFact(initial.owner_alias && initial.dynamic_view_matches_now && initial.events.some(event => event.stage === 'published' && event.matches_expected), 'initial_global_subnet_view_missing')
  run.facts.subnet_owner_alias = initial.owner_alias
  run.invitationTransform = { replace: true, direct: ['[2001:db8:343::11]:23110'], contains_direct: ['[2001:db8:343::11]:23110'] }
  await run.pair()
  run.phase = 'global_on_link'
  await run.transfer()
  await run.transfer(true)
  requireFact(subnetPackets(a).global > 0 && subnetPackets(b).global > 0, 'global_path_positive_control_missing')
  return { a, b }
}

async function changeSubnetPrefix(run, node, beforePrefix, afterPrefix) {
  const address = `2001:db8:343::${11 + node.index}`
  const addresses = () => JSON.parse(net(node, 'ip', '-j', '-6', 'addr', 'show', 'dev', 'eth0')).flatMap(row => row.addr_info ?? []).filter(row => row.family === 'inet6').map(row => ({ ip: row.local, prefix: row.prefixlen })).sort((left, right) => left.ip.localeCompare(right.ip))
  const before = addresses()
  requireFact(before.some(row => row.ip === address && row.prefix === beforePrefix), 'prefix_mutation_input_not_present')
  const port = listenerPort(node)
  await subnetMark(node, 'kernel_mutation_begin')
  const mark = (await subnetState(run, node)).events.filter(event => event.stage === 'kernel_mutation_begin').at(-1).sequence
  const started = new Date().toISOString()
  net(node, 'ip', '-6', 'addr', 'delete', `${address}/${beforePrefix}`, 'dev', 'eth0')
  net(node, 'ip', '-6', 'addr', 'add', `${address}/${afterPrefix}`, 'dev', 'eth0', 'nodad')
  await subnetMark(node, 'kernel_mutation_end')
  const after = addresses()
  requireFact(JSON.stringify(before.map(row => row.ip)) === JSON.stringify(after.map(row => row.ip)) && after.some(row => row.ip === address && row.prefix === afterPrefix), 'prefix_mutation_final_address_set_changed')
  requireFact(listenerPort(node) === port, 'prefix_mutation_changed_listener_port')
  net(node, 'ping', '-6', '-c', '1', '-W', '3', `2001:db8:343::${12 - node.index}`)
  run.facts.prefix_mutations ??= []
  run.facts.prefix_mutations.push({ before_prefix: beforePrefix, after_prefix: afterPrefix, same_final_address_set: true, listener_unchanged: true, route_reachable: true, method: 'delete_then_add_same_address', transient_address_removal: true, started_at_utc: started, completed_at_utc: new Date().toISOString() })
  return mark
}

scenarios.push({ id: 'S03N', topology: 'lan', async run(run) {
  const { a, b } = await prepareSubnetLab(run)
  let heldRead
  if (run.id === 'S03G') {
    const armed = await armSubnetRead(run, a, 'hold_after_native')
    await until(async () => {
      const state = await subnetState(run, a)
      heldRead = state.events.filter(event => event.stage === 'held_after_native' && event.owner_alias === state.owner_alias).at(-1)
      if (!heldRead || state.held_phase !== 'held_after_native' || heldRead.owner_alias !== armed.owner_alias || heldRead.sequence <= armed.sequence) return false
      const segment = state.events.filter(event => event.owner_alias === heldRead.owner_alias && event.generation === heldRead.generation && event.sequence < heldRead.sequence)
      const entered = segment.filter(event => event.stage === 'reader_entered').at(-1)
      const started = entered && segment.find(event => event.stage === 'native_read_started' && event.sequence > entered.sequence)
      const finished = started && segment.find(event => event.stage === 'native_read_finished' && event.sequence > started.sequence)
      const hit = entered && segment.find(event => event.stage === 'fault_hit' && event.fault === 'hold_after_native' && event.sequence > entered.sequence)
      const scheduled = entered && segment.filter(event => event.stage === 'read_scheduled' && event.sequence < entered.sequence).at(-1)
      const valid = scheduled && entered.sequence > armed.sequence && hit && started && hit.sequence < started.sequence && finished?.matches_expected && finished.success && state.fault_active
      if (valid) run.facts.stale_held_native_chain = [armed, scheduled, entered, hit, started, finished, heldRead]
      return valid
    }, 15_000, 'real_matching_stale_read_not_held')
    run.facts.stale_held_read = heldRead
  }
  if (run.id === 'S03F') {
    await until(async () => {
      const state = await subnetState(run, a)
      const published = state.events.filter(event => event.stage === 'published' && event.matches_expected).at(-1)
      return published && published.lease_until_micros - state.elapsed_micros > 25_000_000
    }, 15_000, 'fresh_read_before_failure_control_missing')
    await armSubnetRead(run, a, 'fail_result_once')
  }
  const before = subnetPackets(a)
  await subnetMark(a, 'traffic_begin')
  const marker = await changeSubnetPrefix(run, a, 64, 128)
  if (heldRead) {
    await until(async () => {
      const state = await subnetState(run, a)
      return state.held_phase === 'held_after_native' && !state.dynamic_view_matches_now && state.events.some(event => event.sequence > marker && event.stage === 'native_invalidated' && event.owner_alias === heldRead.owner_alias && event.generation > heldRead.generation)
    }, 15_000, 'new_generation_did_not_revoke_held_old_read')
    requireFact((await a.call('subnet_probe', { action: 'release' })).accepted, 'stale_read_release_failed')
    await until(async () => {
      const state = await subnetState(run, a)
      const invalidated = state.events.find(event => event.sequence > marker && event.stage === 'native_invalidated' && event.owner_alias === heldRead.owner_alias && event.generation > heldRead.generation)
      const released = state.events.find(event => event.sequence > invalidated?.sequence && event.stage === 'read_released' && event.owner_alias === heldRead.owner_alias && event.generation === heldRead.generation)
      requireFact(!state.events.some(event => event.sequence > invalidated?.sequence && event.stage === 'published' && event.owner_alias === heldRead.owner_alias && event.generation === heldRead.generation), 'stale_generation_was_published')
      const discarded = released && state.events.find(event => event.sequence > released.sequence && event.stage === 'stale_generation_discarded' && event.owner_alias === heldRead.owner_alias && event.generation === heldRead.generation)
      if (discarded) run.facts.stale_discard_chain = [invalidated, released, discarded]
      return discarded
    }, 15_000, 'stale_generation_result_not_discarded')
  }
  let revocation
  await until(async () => {
    const state = await subnetState(run, a)
    const invalidated = state.events.find(event => event.sequence > marker && event.stage === 'native_invalidated' && event.owner_alias === state.owner_alias)
    const published = invalidated && state.events.find(event => event.sequence > invalidated.sequence && event.stage === 'published' && event.matches_expected === false && event.owner_alias === state.owner_alias && event.generation === invalidated.generation)
    const matched = state.owner_alias === run.facts.subnet_owner_alias && published && !state.dynamic_view_matches_now
    if (matched) revocation = { invalidated, published }
    return matched
  }, 15_000, 'native_prefix_permission_not_revoked')
  run.facts.subnet_revocation_interval = { owner_alias: revocation.invalidated.owner_alias, generation: revocation.invalidated.generation, invalidated_unix_micros: revocation.invalidated.unix_micros, published_false_unix_micros: revocation.published.unix_micros }
  if (run.id === 'S03F') {
    const state = await subnetState(run, a)
    const events = state.events.filter(event => event.owner_alias === state.owner_alias && event.generation === revocation.invalidated.generation && event.sequence > revocation.invalidated.sequence)
    const stages = ['native_read_started', 'native_read_finished', 'read_failure_injected', 'owner_read_failed', 'published'].map(stage => events.find(event => event.stage === stage))
    requireFact(stages.every(Boolean) && stages.every((event, index) => index === 0 || event.sequence > stages[index - 1].sequence) && stages[1].success && stages[4].matches_expected === false && stages[4].count === 0 && !state.fault_active && !state.fault_armed, 'actual_native_failure_did_not_fail_closed')
    run.facts.native_read_failure_chain = stages
  }
  for (const node of run.nodes) blockFixedPaths(node, false)
  run.phase = 'prefix_revoked_fixed_local_control'
  let revoked
  try { await run.transfer(); await run.transfer(true) }
  finally {
    revoked = subnetPackets(a)
    run.facts.prefix_revocation_packets = { initial: before, after: revoked, global_delta: revoked.global - before.global, fixed_delta: revoked.fixed - before.fixed, historical_whole_window_zero: revoked.global === before.global, whole_window_interpretation: 'conservative_diagnostic_spans_initial_authorized_and_mutation_detection_intervals' }
  }
  requireFact(revoked.fixed > before.fixed, 'fixed_local_continuity_not_proved')
  await subnetMark(a, 'traffic_end')
  const restoredAt = await changeSubnetPrefix(run, a, 128, 64)
  const restoredState = await subnetState(run, a)
  const restoreBegin = restoredState.events.find(event => event.sequence === restoredAt && event.stage === 'kernel_mutation_begin')
  requireFact(revocation.invalidated.unix_micros > 0 && revocation.invalidated.unix_micros <= revocation.published.unix_micros && restoreBegin?.unix_micros > revocation.published.unix_micros, 'subnet_revocation_clock_order_invalid')
  run.facts.subnet_revocation_interval.restore_begin_unix_micros = restoreBegin.unix_micros
  await until(async () => {
    const state = await subnetState(run, a)
    const invalidated = state.events.find(event => event.sequence > restoredAt && event.stage === 'native_invalidated' && event.owner_alias === run.facts.subnet_owner_alias && event.generation > revocation.invalidated.generation)
    const published = invalidated && state.events.find(event => event.sequence > invalidated.sequence && event.stage === 'published' && event.matches_expected && event.owner_alias === invalidated.owner_alias && event.generation === invalidated.generation)
    if (published) run.facts.subnet_native_restore = { invalidated, published }
    return state.owner_alias === run.facts.subnet_owner_alias && state.dynamic_view_matches_now && published
  }, 15_000, 'native_prefix_permission_not_restored')
  for (const node of run.nodes) blockFixedPaths(node, true)
  run.phase = 'global_on_link_restored'
  await run.transfer()
  await run.transfer(true)
  requireFact(subnetPackets(a).global > revoked.global, 'restored_global_path_not_used')
} })

for (const id of ['S03G', 'S03F']) scenarios.push({ id, topology: 'lan', run: scenarios.find(scenario => scenario.id === 'S03N').run })

scenarios.push({ id: 'S03L', topology: 'lan', async run(run) {
  const { a } = await prepareSubnetLab(run)
  const armed = await armSubnetRead(run, a, 'hold_before_native')
  let held, published, scheduled
  await until(async () => {
    const state = await subnetState(run, a)
    held = state.events.filter(event => event.stage === 'held_before_native').at(-1)
    published = state.events.filter(event => event.stage === 'published' && event.matches_expected).at(-1)
    const entered = held && state.events.filter(event => event.stage === 'reader_entered' && event.owner_alias === held.owner_alias && event.generation === held.generation && event.sequence < held.sequence).at(-1)
    const hit = entered && state.events.find(event => event.stage === 'fault_hit' && event.fault === 'hold_before_native' && event.owner_alias === held.owner_alias && event.generation === held.generation && event.sequence > entered.sequence && event.sequence < held.sequence)
    scheduled = entered && state.events.filter(event => event.stage === 'read_scheduled' && event.owner_alias === held.owner_alias && event.sequence < entered.sequence && event.generation === held.generation).at(-1)
    return state.held_phase === 'held_before_native' && entered?.sequence > armed.sequence && hit && published && scheduled && state.dynamic_view_matches_now
  }, 15_000, 'renewal_before_native_not_held')
  run.facts.lease_hold = { held, published, scheduled }
  await until(async () => {
    const state = await subnetState(run, a)
    requireFact(state.held_phase === 'held_before_native' && !state.events.some(event => event.sequence > held.sequence && ['native_invalidated', 'published', 'native_read_started'].includes(event.stage)), 'held_lease_was_invalidated_or_extended')
    return state.lease_expired_now && !state.dynamic_view_matches_now && state.elapsed_micros >= published.lease_until_micros
  }, 35_000, 'real_original_lease_did_not_expire')
  const expired = await subnetState(run, a)
  const offsets = expired.events.map(event => event.unix_micros - event.elapsed_micros)
  run.facts.subnet_lease_denial_interval = { owner_alias: held.owner_alias, generation: held.generation, original_lease_until_micros: published.lease_until_micros, expired_unix_micros: Math.min(...offsets) + published.lease_until_micros, observed_expired_elapsed_micros: expired.elapsed_micros }
  for (const node of run.nodes) blockFixedPaths(node, false)
  run.phase = 'real_lease_expired_fixed_local'
  await run.transfer()
  await run.transfer(true)
  const beforeRelease = await subnetState(run, a)
  requireFact(beforeRelease.held_phase === 'held_before_native' && beforeRelease.lease_expired_now && !beforeRelease.dynamic_view_matches_now && !beforeRelease.events.some(event => event.sequence > held.sequence && ['native_invalidated', 'published', 'native_read_started'].includes(event.stage)), 'expired_lease_granted_before_release')
  await subnetMark(a, 'traffic_end')
  const release = (await subnetState(run, a)).events.filter(event => event.stage === 'traffic_end').at(-1)
  run.facts.subnet_lease_denial_interval.controller_end_marker_unix_micros = release.unix_micros
  const recoveryStarted = performance.now()
  const recoveryRemaining = () => 20_000 - (performance.now() - recoveryStarted)
  requireFact((await a.call('subnet_probe', { action: 'release' })).accepted, 'lease_read_release_failed')
  const releasedState = await subnetState(run, a)
  const released = releasedState.events.find(event => event.sequence > release.sequence && event.stage === 'read_released' && event.owner_alias === held.owner_alias && event.generation === held.generation)
  requireFact(released, 'exact_lease_read_release_missing')
  run.facts.subnet_lease_denial_interval.read_released_unix_micros = released.unix_micros
  await until(async () => {
    const state = await subnetState(run, a)
    const segment = state.events.filter(event => event.sequence > released.sequence && event.owner_alias === held.owner_alias && event.generation === held.generation)
    const started = segment.find(event => event.stage === 'native_read_started')
    const finished = started && segment.find(event => event.stage === 'native_read_finished' && event.sequence > started.sequence)
    const resumed = finished && segment.find(event => event.stage === 'published' && event.sequence > finished.sequence)
    if (!resumed) return false
    requireFact(finished.success && finished.matches_expected && resumed.matches_expected && resumed.lease_until_micros === scheduled.lease_until_micros, 'delayed_read_success_or_original_lease_not_proved')
    run.facts.delayed_read_publication = resumed
    run.facts.delayed_native_read_chain = [released, started, finished, resumed]
    return true
  }, Math.min(10_000, recoveryRemaining()), 'exact_delayed_native_read_did_not_publish')
  await until(async () => {
    const state = await subnetState(run, a)
    return state.dynamic_view_matches_now && state.events.some(event => event.sequence >= run.facts.delayed_read_publication.sequence && event.stage === 'published' && event.matches_expected)
  }, recoveryRemaining(), 'real_native_read_did_not_restore_lease')
  run.facts.lease_recovery_elapsed_ms = Math.round(performance.now() - recoveryStarted)
  for (const node of run.nodes) blockFixedPaths(node, true)
  run.phase = 'lease_native_read_recovered_global'
  await run.transfer()
  await run.transfer(true)
} })

scenarios.push({ id: 'S03T', topology: 'lan', async run(run) {
  const { a } = await prepareSubnetLab(run, false)
  const armed = await armSubnetRead(run, a, 'hold_after_native')
  await until(async () => {
    const state = await subnetState(run, a)
    return state.held_phase === 'held_after_native' && state.events.some(event => event.stage === 'held_after_native' && event.owner_alias === armed.owner_alias && event.sequence > armed.sequence)
  }, 15_000, 'shutdown_native_read_not_held')
  const held = (await subnetState(run, a)).events.filter(event => event.stage === 'held_after_native').at(-1)
  const started = performance.now()
  await a.stop()
  requireFact(performance.now() - started < 15_000, 'held_subnet_shutdown_deadline')
  const final = run.facts.subnet_probe_snapshots.filter(row => row.node === 'A' && row.phase === 'after_shutdown').at(-1)?.snapshot
  requireFact(final && final.dropped_events === 0 && final.owner_alias === held.owner_alias && final.owner_stopped && final.worker_body_finished && final.worker_joined && !final.held_phase && !final.fault_active && !final.fault_armed && !final.dynamic_view_matches_now, 'held_subnet_owner_not_joined')
  const events = final.events.filter(event => event.sequence > held.sequence)
  const stopped = events.find(event => event.stage === 'owner_stopped')
  requireFact(stopped && events.some(event => event.stage === 'hold_cancelled_by_stop') && events.some(event => event.stage === 'stopped_result_discarded' && event.generation === held.generation) && events.some(event => event.stage === 'worker_body_finished' && event.success) && events.some(event => event.stage === 'worker_joined' && event.success) && !events.some(event => event.sequence > stopped.sequence && event.stage === 'published'), 'held_shutdown_chain_incomplete')
  run.facts.held_subnet_shutdown = { elapsed_ms: Math.round(performance.now() - started), final }
} })

async function beginRecovery(node) {
  const epoch = await node.call('stream_fault', { action: 'begin_recovery' })
  node.recoveryObserved = true
  return epoch
}

async function observeReceivePath(run, node, resetActivePull) {
  const epoch = await node.call('stream_fault', { action: 'begin_receive', reset_active_pull: resetActivePull })
  node.receiveObserved = true
  node.receiveResetExpected = resetActivePull
  run.facts.receive_epoch = epoch
  return epoch
}

async function receiveEvidence(run, node, epoch) {
  const state = await node.call('stream_fault', { action: 'receive_snapshot' })
  run.facts.receive_observation = state
  requireFact(state.epoch_alias === epoch, 'receive_observation_epoch_changed')
  requireFact(state.reset_active_pull === node.receiveResetExpected, 'receive_fault_mode_changed')
  return state
}

function assertResetAttempts(state, expected, excluded = []) {
  requireFact(state.dropped_events === 0 && state.remaining_hits === 0 && !state.armed_action, 'reset_plan_not_fully_consumed')
  const resets = state.events.filter(event => event.kind === 'SendReset')
  const aliases = new Set(resets.map(event => event.attempt_alias))
  requireFact(resets.length === expected && aliases.size === expected, 'distinct_real_reset_count_incorrect')
  for (const alias of aliases) {
    const events = state.events.filter(event => event.attempt_alias === alias)
    requireFact(events.filter(event => event.kind === 'ResultReadyFailure').length === 1 && events.filter(event => event.kind === 'ReturnedFailure').length === 1 && !events.some(event => event.kind === 'ReturnedSuccess'), 'reset_attempt_failure_chain_incomplete')
  }
  requireFact(state.events.filter(event => event.kind === 'Started').every(event => aliases.has(event.attempt_alias) || excluded.includes(event.attempt_alias)), 'unexpected_ordinary_dispatch_attempt')
}

async function isolatedReceiveEvidence(run, node, epoch) {
  let state
  await until(async () => {
    state = await receiveEvidence(run, node, epoch)
    const pulls = state.events.filter(event => event.path === 'active_pull')
    requireFact(!pulls.some(event => ['envelope_received', 'apply_started', 'applied', 'duplicate'].includes(event.stage)), 'active_pull_bypassed_native_reset')
    let pending = false
    let resetHits = 0
    let preflightFailures = 0
    for (const alias of new Set(pulls.map(event => event.attempt_alias))) {
      const events = pulls.filter(event => event.attempt_alias === alias)
      requireFact(new Set(events.map(event => event.item_alias)).size === 1, 'receive_attempt_item_changed')
      const started = events.find(event => event.stage === 'started')
      const opened = events.find(event => event.stage === 'stream_opened')
      const reset = events.find(event => event.stage === 'send_reset')
      const failed = events.find(event => event.stage === 'failed')
      requireFact(started, 'pull_attempt_start_missing')
      if (!opened) { if (failed) preflightFailures++; continue }
      if (!reset || !failed) { pending = true; continue }
      requireFact(started.seq < opened.seq && opened.seq < reset.seq && reset.seq < failed.seq, 'pull_native_reset_failure_order_incorrect')
      resetHits++
    }
    run.facts.pull_isolation = { attempts: new Set(pulls.map(event => event.attempt_alias)).size, native_reset_hits: resetHits, preflight_failures: preflightFailures }
    return !pending
  }, 10_000, 'opened_pull_native_reset_chain_incomplete')
  return state
}

function retryTiming(observation, epoch, after = 0, expectedRounds = 3) {
  requireFact(observation.epoch_alias === epoch, 'retry_timing_epoch_changed')
  const window = observation.events.filter(event => event.seq > after)
  const due = window.filter(event => event.kind === 'retry_due')
  const starts = window.filter(event => event.kind === 'round_started' && event.trigger === 'retry')
  const dispatches = window.filter(event => event.kind === 'dispatch_started' && event.trigger === 'retry')
  requireFact(due.length === expectedRounds && starts.length === expectedRounds && dispatches.length === expectedRounds, 'real_stream_retry_count_incorrect')
  return due.map((consumed, index) => {
    const expectedDelay = [2, 5, 10][index] * 1_000_000_000
    const origin = consumed.schedule_origin
    requireFact(origin?.epoch_alias === epoch, 'retry_origin_epoch_mismatch')
    const source = observation.events.find(event => event.seq === origin.seq)
    const scheduled = window.find(event => event.kind === 'schedule_updated' && event.schedule_origin?.epoch_alias === epoch && event.schedule_origin.seq === origin.seq && event.due_ns === consumed.due_ns)
    const matches = event => event.target_alias === consumed.target_alias && event.schedule_origin?.epoch_alias === epoch && event.schedule_origin.seq === origin.seq && event.rounds_started === consumed.rounds_started
    const matchedStarts = starts.filter(matches)
    const matchedDispatches = dispatches.filter(matches)
    requireFact(matchedStarts.length === 1 && matchedDispatches.length === 1, 'retry_dispatch_join_not_unique')
    const started = matchedStarts[0]
    const dispatched = matchedDispatches[0]
    requireFact(source?.kind === 'result_recoverable' && scheduled, 'retry_causal_schedule_chain_missing')
    requireFact(source.target_alias === consumed.target_alias && source.item_alias === scheduled.tracked_item_alias && dispatched.item_alias === source.item_alias, 'retry_item_target_alias_mismatch')
    requireFact(scheduled.due_ns - scheduled.at_ns === expectedDelay, 'retry_backoff_interval_incorrect')
    requireFact(consumed.rounds_started === index + 1 && consumed.at_ns >= consumed.due_ns && started.at_ns >= consumed.at_ns && dispatched.at_ns >= started.at_ns, 'retry_started_before_due')
    const lateness = dispatched.at_ns - consumed.due_ns
    requireFact(lateness <= 500_000_000, 'retry_dispatch_lateness_exceeded')
    return { round: index + 1, backoff_ns: expectedDelay, result_handling_ns: scheduled.at_ns - source.at_ns, due_consumption_lateness_ns: consumed.at_ns - consumed.due_ns, gate_delay_ns: started.at_ns - consumed.at_ns, owner_checks_ns: dispatched.at_ns - started.at_ns, actual_dispatch_lateness_ns: lateness }
  })
}

scenarios.push({ id: 'R01', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  environment(a.capabilities.includes('clipboard-stream-fault'), 'real_stream_fault_host_required')
  for (const node of run.nodes) { await node.drain(); node.timeline = [] }
  const recoveryEpoch = await beginRecovery(a)
  const receiveEpoch = await observeReceivePath(run, b, true)
  await a.call('stream_fault', { action: 'reset' })
  await a.call('stream_fault', { action: 'arm', phase: 'BeforeFrame', fault: 'ResetSend', hits: 1 })
  const text = `uc343-reset-${randomUUID()}`
  const digest = hash(text)
  run.canaries.push(text)
  const observed = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(observed.report && observed.report.total_accepted === 0, 'real_reset_copy_did_not_fail')
  const initial = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.stream_fault_initial = initial
  assertResetAttempts(initial, 1)
  requireFact(initial.dropped_events === 0, 'stream_fault_evidence_overflow')
  const reset = initial.events.find(event => event.kind === 'SendReset')
  requireFact(reset && initial.events.some(event => event.attempt_alias === reset.attempt_alias && event.kind === 'ReturnedFailure'), 'pre_frame_reset_failure_not_observed')
  let found = false
  for (const entry of await b.call('history')) if (hash((await b.call('entry', { entry: entry.entry_id })).content ?? '') === digest) found = true
  requireFact(!found, 'failed_pre_frame_attempt_delivered_content')
  const checkpoint = await a.call('stream_fault', { action: 'recovery_snapshot' })
  run.facts.pre_retry_absence_checkpoint = checkpoint
  requireFact(checkpoint.epoch_alias === recoveryEpoch && !checkpoint.events.some(event => event.kind === 'retry_due'), 'pre_retry_absence_checkpoint_not_proved')
  const initialReceive = await receiveEvidence(run, b, receiveEpoch)
  requireFact(!initialReceive.events.some(event => ['applied', 'duplicate'].includes(event.stage)), 'failed_attempt_applied_content')
  run.facts.receiver_absent_after_failed_attempt = true
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'snapshot' })
    run.facts.stream_fault_final = state
    requireFact(state.dropped_events === 0, 'stream_fault_evidence_overflow')
    for (const node of run.nodes) {
      requireFact((await node.call('peers')).some(peer => peer.connected), 'real_stream_reset_changed_presence')
      await node.drain()
    }
    return state.events.some(event => event.attempt_alias !== reset.attempt_alias && event.kind === 'ReturnedSuccess')
  }, 20_000, 'real_stream_retry_did_not_succeed')
  await until(async () => {
    for (const entry of await b.call('history')) if (hash((await b.call('entry', { entry: entry.entry_id })).content ?? '') === digest) return true
    return false
  }, 25_000, 'real_stream_retry_readback_failed')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', bytes: Buffer.byteLength(text), sha256: digest, matched: true, failed_attempt_could_not_submit_frame: true })
  await until(async () => {
    const state = await a.call('delivery', { entry: observed.report.entry_id })
    run.facts.final_sender_status = state.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
    return state.deliveries.some(target => ['delivered', 'duplicate'].includes(target.status.kind))
  }, 10_000, 'real_stream_retry_sender_not_settled')
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  const pushes = received.events.filter(event => event.path === 'clipboard_push')
  const applied = pushes.filter(event => event.stage === 'applied')
  requireFact(applied.length === 1 && applied[0].entry_alias && new Set(pushes.map(event => event.item_alias)).size === 1, 'retry_push_commit_not_unique')
  for (const stage of ['started', 'apply_started']) requireFact(pushes.some(event => event.attempt_alias === applied[0].attempt_alias && event.stage === stage && event.seq < applied[0].seq), 'retry_push_commit_chain_missing')
  requireFact(!received.events.some(event => event.path === 'active_pull' && ['applied', 'duplicate'].includes(event.stage)), 'active_pull_bypassed_retry_isolation')
  const recovery = await a.call('stream_fault', { action: 'recovery_snapshot' })
  run.facts.recovery_observation = recovery
  run.facts.retry_timing = retryTiming(recovery, recoveryEpoch, 0, 1)
  requireFact(new Set(recovery.events.map(event => event.target_alias)).size === 1 && new Set(recovery.events.flatMap(event => [event.item_alias, event.tracked_item_alias]).filter(Boolean)).size === 1, 'retry_origin_contains_unrelated_item_or_target')
  const finalStreams = run.facts.stream_fault_final
  requireFact(finalStreams.events.filter(event => event.kind === 'Started').length === 2 && finalStreams.events.filter(event => event.kind === 'ReturnedSuccess').length === 1 && finalStreams.events.filter(event => event.kind === 'SendReset').length === 1, 'retry_ordinary_attempt_correlation_ambiguous')
  requireFact(!run.nodes.some(node => node.timeline.some(event => /offline|disconnected/i.test(event.state ?? ''))), 'real_stream_fault_presence_transition')
} })

async function boundedRetry(run, isolate = true) {
  await run.pair()
  const [a, b] = run.nodes
  environment(a.capabilities.includes('clipboard-stream-fault'), 'real_stream_fault_host_required')
  for (const node of run.nodes) { await node.drain(); node.timeline = [] }
  const receiveEpoch = await observeReceivePath(run, b, isolate)
  const epoch = await beginRecovery(a)
  await a.call('stream_fault', { action: 'reset' })
  await a.call('stream_fault', { action: 'arm', phase: 'BeforeFrame', fault: 'ResetSend', hits: 4 })
  const text = `uc343-bounded-${randomUUID()}`
  run.canaries.push(text)
  const digest = hash(text)
  run.facts.failed_payload = { bytes: Buffer.byteLength(text), sha256: digest }
  const copy = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(copy.report && copy.report.total_accepted === 0, 'bounded_real_stream_copy_did_not_fail')
  let exhausted
  await until(async () => {
    const recovery = await a.call('stream_fault', { action: 'recovery_snapshot' })
    const streams = await a.call('stream_fault', { action: 'snapshot' })
    run.facts.recovery_observation = recovery
    run.facts.stream_fault_final = streams
    requireFact(recovery.epoch_alias === epoch && streams.dropped_events === 0, 'bounded_recovery_evidence_epoch_invalid')
    for (const node of run.nodes) {
      requireFact((await node.call('peers')).some(peer => peer.connected), 'real_stream_reset_changed_presence')
      await node.drain()
    }
    exhausted = recovery.events.find(event => event.kind === 'schedule_updated' && event.rounds_started === 3 && event.due_ns === null)
    return recovery.events.filter(event => event.kind === 'result_recoverable').length >= 4 && streams.events.filter(event => event.kind === 'SendReset').length === 4 && Boolean(exhausted)
  }, 100_000, 'real_stream_three_retries_not_exhausted')
  const observation = run.facts.recovery_observation
  run.facts.retry_timing = retryTiming(observation, epoch)
  assertResetAttempts(run.facts.stream_fault_final, 4)
  const quietStart = observation.now_ns
  while (true) {
    await delay(1_000)
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.recovery_observation = state
    requireFact(state.epoch_alias === epoch, 'retry_quiet_epoch_changed')
    requireFact(state.events.filter(event => event.kind === 'retry_due').length === 3, 'retry_fourth_due_after_exhaustion')
    requireFact(state.events.filter(event => event.kind === 'round_started' && event.trigger === 'retry').length === 3, 'retry_fourth_start_after_exhaustion')
    requireFact(state.events.filter(event => event.kind === 'dispatch_started' && event.trigger === 'retry').length === 3, 'retry_fourth_dispatch_after_exhaustion')
    for (const node of run.nodes) {
      requireFact((await node.call('peers')).some(peer => peer.connected), 'presence_lost_in_quiet_window')
      await node.drain()
    }
    if (state.now_ns - quietStart >= 30_000_000_000) { run.facts.quiet_window_ns = state.now_ns - quietStart; break }
  }
  requireFact(!run.nodes.some(node => node.timeline.some(event => /offline|disconnected/i.test(event.state ?? ''))), 'presence_transition_reset_retry_count')
  const delivery = await a.call('delivery', { entry: copy.report.entry_id })
  run.facts.exhausted_sender_status = delivery.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
  requireFact(delivery.deliveries.length === 1 && delivery.deliveries.every(target => target.status.kind === 'unreachable'), 'exhausted_target_not_recoverable')
  const received = isolate ? await isolatedReceiveEvidence(run, b, receiveEpoch) : await receiveEvidence(run, b, receiveEpoch)
  if (isolate) {
    requireFact(!received.events.some(event => ['applied', 'duplicate'].includes(event.stage)), 'bounded_fault_applied_content')
    for (const entry of await b.call('history')) requireFact(hash((await b.call('entry', { entry: entry.entry_id })).content ?? '') !== digest, 'reset_before_frame_delivered_payload')
  } else {
    const applied = received.events.find(event => event.path === 'active_pull' && event.stage === 'applied')
    requireFact(applied?.entry_alias, 'alternative_active_pull_commit_not_attributed')
    for (const stage of ['started', 'stream_opened', 'envelope_received', 'apply_started']) requireFact(received.events.some(event => event.attempt_alias === applied.attempt_alias && event.item_alias === applied.item_alias && event.stage === stage && event.seq < applied.seq), 'alternative_active_pull_chain_missing')
    requireFact(!received.events.some(event => event.path === 'clipboard_push' && ['applied', 'duplicate'].includes(event.stage)), 'ordinary_push_unexpectedly_delivered')
    let matched = false
    for (const entry of await b.call('history')) if (hash((await b.call('entry', { entry: entry.entry_id })).content ?? '') === digest) matched = true
    requireFact(matched, 'alternative_pull_hash_readback_missing')
    run.readbacks.push({ from: 'A', to: 'B', kind: 'text', bytes: Buffer.byteLength(text), sha256: digest, matched: true, path: 'active_pull' })
  }
  return { a, b, epoch, entry: copy.report.entry_id, digest }
}

scenarios.push({ id: 'R02', topology: 'vpn', run: boundedRetry })
scenarios.push({ id: 'R02A', topology: 'vpn', run: run => boundedRetry(run, false) })

scenarios.push({ id: 'R03', topology: 'vpn', async run(run) {
  const { a, b, epoch, entry, digest } = await boundedRetry(run)
  run.facts.first_retry_window = run.facts.recovery_observation
  const before = run.facts.first_retry_window.events.at(-1).seq
  await b.stop()
  await until(async () => !(await a.call('peers')).some(peer => peer.connected), 25_000, 'offline_transition_not_observed')
  await a.call('stream_fault', { action: 'arm', phase: 'BeforeFrame', fault: 'ResetSend', hits: 4 })
  b.resetActivePull = true
  b.receiveResetExpected = true
  requireFact((await b.start()).ready, 'online_reset_peer_start_failed')
  const receiveEpoch = (await b.call('stream_fault', { action: 'receive_snapshot' })).epoch_alias
  await run.online()
  let exhausted
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    requireFact(state.epoch_alias === epoch, 'online_reset_recovery_epoch_changed')
    run.facts.second_retry_window = state
    const events = state.events.filter(event => event.seq > before)
    exhausted = events.find(event => event.kind === 'schedule_updated' && event.rounds_started === 3 && event.due_ns === null)
    return Boolean(exhausted) && events.filter(event => event.kind === 'result_recoverable').length >= 4
  }, 100_000, 'online_reset_window_not_exhausted')
  run.facts.second_retry_timing = retryTiming(run.facts.second_retry_window, epoch, before)
  const events = run.facts.second_retry_window.events.filter(event => event.seq > before)
  requireFact(events.filter(event => event.kind === 'round_started' && event.trigger === 'peer_online').length === 1, 'online_transition_did_not_dispatch_exactly_once')
  requireFact(events.filter(event => event.kind === 'retry_due').length === 3 && events.filter(event => event.kind === 'round_started' && event.trigger === 'retry').length === 3 && events.filter(event => event.kind === 'dispatch_started' && event.trigger === 'retry').length === 3, 'online_window_retry_budget_incorrect')
  requireFact(events.filter(event => event.kind === 'dispatch_started' && event.trigger === 'peer_online').length === 1, 'online_window_complete_dispatch_count_incorrect')
  const quietStart = run.facts.second_retry_window.now_ns
  while (true) {
    await delay(1_000)
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    requireFact(state.epoch_alias === epoch, 'online_quiet_epoch_changed')
    run.facts.second_retry_window = state
    const tail = state.events.filter(event => event.seq > before)
    requireFact(tail.filter(event => event.kind === 'retry_due').length === 3, 'online_window_fourth_retry')
    requireFact(tail.filter(event => event.kind === 'dispatch_started' && event.trigger === 'retry').length === 3, 'online_window_fourth_dispatch')
    requireFact(tail.filter(event => event.kind === 'round_started' && event.trigger === 'peer_online').length === 1, 'online_window_duplicate_dispatch')
    for (const node of run.nodes) requireFact((await node.call('peers')).some(peer => peer.connected), 'online_reset_quiet_presence_lost')
    if (state.now_ns - quietStart >= 30_000_000_000) { run.facts.second_quiet_window_ns = state.now_ns - quietStart; break }
  }
  const streams = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.stream_fault_final = streams
  assertResetAttempts(streams, 8)
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(!received.events.some(event => ['applied', 'duplicate'].includes(event.stage)), 'online_window_received_blocked_content')
  for (const row of await b.call('history')) requireFact(hash((await b.call('entry', { entry: row.entry_id })).content ?? '') !== digest, 'online_window_delivered_blocked_content')
  await a.drain()
  const transitions = a.timeline.filter(event => event.kind === 'peer').map(event => event.state?.toLowerCase()).filter(state => state === 'offline' || state === 'online')
  run.facts.controlled_presence_transitions = transitions
  requireFact(transitions.filter(state => state === 'offline').length === 1 && transitions.filter(state => state === 'online').length === 1, 'online_window_presence_transition_count_incorrect')
  const view = await a.call('delivery', { entry })
  run.facts.second_exhausted_sender_status = view.deliveries.map(target => target.status.kind)
  requireFact(view.deliveries.length === 1 && view.deliveries.every(target => target.status.kind === 'unreachable'), 'online_window_sender_not_unreachable')
} })

async function lateResultCase(run, lateKind, exhaustBeforeRelease = false) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, true)
  const epoch = await beginRecovery(a)
  const action = lateKind === 'settled' ? 'Hold' : 'ResetSendThenHoldCompletion'
  await a.call('stream_fault', { action: 'arm', phase: lateKind === 'settled' ? 'BeforeCompletion' : 'BeforeFrame', fault: action, hits: 1 })
  const olderText = `uc343-late-X-${randomUUID()}`
  const newerText = `uc343-late-Y-${randomUUID()}`
  run.canaries.push(olderText, newerText)
  const older = await a.call('capture', { kind: 'text', text: olderText, observed_at_ms: Date.now(), dispatch: true })
  requireFact(older.report, 'held_old_copy_report_missing')
  const heldState = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.held_old_attempt = heldState
  const hold = heldState.holds.find(hold => hold.phase === 'BeforeCompletion')
  requireFact(hold && heldState.dropped_events === 0, 'old_completion_not_held')
  const readyKind = lateKind === 'settled' ? 'ResultReadySuccess' : 'ResultReadyFailure'
  requireFact(heldState.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === readyKind), 'old_real_result_not_ready')
  if (lateKind !== 'settled') requireFact(heldState.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === 'SendReset'), 'held_old_failure_was_not_native_reset')
  requireFact(!heldState.events.some(event => event.attempt_alias === hold.attempt_alias && ['ReturnedSuccess', 'ReturnedFailure'].includes(event.kind)), 'old_result_returned_before_release')
  let olderMatched = false
  for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(olderText)) olderMatched = true
  requireFact(olderMatched === (lateKind === 'settled'), 'old_result_receiver_boundary_incorrect')
  if (olderMatched) run.readbacks.push({ from: 'A', to: 'B', kind: 'text', bytes: Buffer.byteLength(olderText), sha256: hash(olderText), matched: true })
  await a.call('stream_fault', { action: 'arm', phase: 'BeforeFrame', fault: 'ResetSend', hits: 4 })
  const newer = await a.call('capture', { kind: 'text', text: newerText, observed_at_ms: Date.now(), dispatch: true })
  requireFact(newer.report?.total_accepted === 0, 'new_copy_real_reset_not_observed')
  let saved, item, olderItem
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    requireFact(state.epoch_alias === epoch, 'late_result_observation_epoch_changed')
    run.facts.before_late_release = state
    item = state.events.find(event => event.kind === 'result_recoverable')?.item_alias
    olderItem = state.events.find(event => event.kind === 'result_settled' && event.item_alias && event.item_alias !== item)?.item_alias
    const schedules = state.events.filter(event => event.kind === 'schedule_updated' && event.tracked_item_alias === item)
    saved = schedules.at(-1)
    return olderItem && saved && saved.rounds_started === (exhaustBeforeRelease ? 3 : 1) && (exhaustBeforeRelease ? saved.due_ns === null : saved.due_ns - state.now_ns > 1_000_000_000)
  }, 35_000, 'new_copy_retry_budget_not_established')
  const beforeRelease = run.facts.before_late_release
  requireFact(new Set(beforeRelease.events.map(event => event.target_alias)).size === 1 && new Set(beforeRelease.events.flatMap(event => [event.item_alias, event.tracked_item_alias]).filter(Boolean)).size === 2, 'new_budget_alias_selection_ambiguous')
  requireFact(saved.schedule_origin?.epoch_alias === epoch && saved.schedule_origin.seq, 'new_budget_origin_missing')
  const superseded = await a.call('delivery', { entry: older.report.entry_id })
  run.facts.old_status_before_release = superseded.deliveries.map(target => target.status.kind)
  requireFact(superseded.deliveries.length === 1 && superseded.deliveries.every(target => target.status.kind === 'superseded'), 'old_copy_not_superseded_while_held')
  const released = await a.call('stream_fault', { action: 'release_hold', hold_alias: hold.hold_alias })
  requireFact(released.released, 'exact_old_hold_release_failed')
  requireFact(!(await a.call('stream_fault', { action: 'release_hold', hold_alias: hold.hold_alias })).released, 'stale_hold_token_released_again')
  let late
  await until(async () => {
    const streams = await a.call('stream_fault', { action: 'snapshot' })
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.after_late_release = state
    run.facts.stream_fault_final = streams
    requireFact(streams.dropped_events === 0 && state.epoch_alias === epoch, 'late_result_evidence_invalid')
    requireFact(!streams.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === 'HoldCancelled'), 'old_hold_cancelled_instead_of_released')
    const releasedEvent = streams.events.find(event => event.attempt_alias === hold.attempt_alias && event.hold_alias === hold.hold_alias && event.kind === 'Released')
    const returned = streams.events.find(event => event.attempt_alias === hold.attempt_alias && event.kind === (lateKind === 'settled' ? 'ReturnedSuccess' : 'ReturnedFailure'))
    late = state.events.find(event => event.seq > saved.seq && event.kind === 'result_write_fenced' && event.item_alias === olderItem && event.target_alias === saved.target_alias)
    if (!releasedEvent || !returned || !late) return false
    return true
  }, 10_000, 'old_real_result_not_received_after_new_budget')
  run.facts.late_write_fenced = { item_alias: olderItem, event_seq: late.seq, target_alias: late.target_alias }
  if (!exhaustBeforeRelease) {
    await until(async () => {
      const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
      requireFact(state.epoch_alias === epoch, 'late_result_due_epoch_changed')
      run.facts.after_late_release = state
      return state.events.some(event => event.seq > late.seq && event.kind === 'retry_due' && event.tracked_item_alias === item && event.rounds_started === 2 && event.due_ns === saved.due_ns && event.schedule_origin?.epoch_alias === epoch && event.schedule_origin.seq === saved.schedule_origin.seq)
    }, 8_000, 'new_budget_due_changed_after_late_result')
    await until(async () => {
      const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
      requireFact(state.epoch_alias === epoch, 'late_result_exhaustion_epoch_changed')
      run.facts.after_late_release = state
      return state.events.some(event => event.kind === 'schedule_updated' && event.tracked_item_alias === item && event.rounds_started === 3 && event.due_ns === null)
    }, 20_000, 'new_copy_budget_not_exhausted')
  }
  const quiet = run.facts.after_late_release.now_ns
  while (true) {
    await delay(1_000)
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    requireFact(state.epoch_alias === epoch, 'late_result_quiet_epoch_changed')
    run.facts.after_late_release = state
    requireFact(!state.events.some(event => event.seq > late.seq && event.item_alias === olderItem && ['result_recoverable', 'result_not_persisted', 'result_settled'].includes(event.kind)), 'fenced_old_result_published_false_persisted_notification')
    requireFact(new Set(state.events.map(event => event.target_alias)).size === 1 && new Set(state.events.flatMap(event => [event.item_alias, event.tracked_item_alias]).filter(Boolean)).size === 2, 'late_result_alias_correlation_ambiguous')
    requireFact(state.events.filter(event => event.kind === 'retry_due' && event.tracked_item_alias === item).length === 3, 'late_result_restarted_exhausted_budget')
    requireFact(state.events.filter(event => event.kind === 'dispatch_started' && event.item_alias === item).length === 3, 'late_result_extra_new_item_dispatch')
    if (exhaustBeforeRelease) requireFact(!state.events.some(event => event.seq > late.seq && event.tracked_item_alias === item && (event.kind === 'cleared' || (event.kind === 'schedule_updated' && event.due_ns !== null))), 'late_result_mutated_exhausted_budget')
    for (const node of run.nodes) requireFact((await node.call('peers')).some(peer => peer.connected), 'late_result_presence_changed')
    if (state.now_ns - quiet >= 30_000_000_000) break
  }
  const completedStreams = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.stream_fault_final = completedStreams
  assertResetAttempts(completedStreams, lateKind === 'settled' ? 4 : 5, lateKind === 'settled' ? [hold.attempt_alias] : [])
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(!received.events.some(event => event.path === 'active_pull' && ['applied', 'duplicate'].includes(event.stage)), 'late_result_active_pull_bypass')
  for (const row of await b.call('history')) requireFact(hash((await b.call('entry', { entry: row.entry_id })).content ?? '') !== hash(newerText), 'held_new_copy_unexpectedly_received')
  const oldView = await a.call('delivery', { entry: older.report.entry_id })
  const newView = await a.call('delivery', { entry: newer.report.entry_id })
  run.facts.old_sender_status = oldView.deliveries.map(target => target.status.kind)
  run.facts.new_sender_status = newView.deliveries.map(target => target.status.kind)
  requireFact(oldView.deliveries.length === 1 && oldView.deliveries.every(target => target.status.kind === 'superseded'), 'late_old_result_changed_superseded_state')
  requireFact(newView.deliveries.length === 1 && newView.deliveries.every(target => target.status.kind === 'unreachable'), 'late_result_changed_new_final_status')
}
scenarios.push({ id: 'R04', topology: 'vpn', run: run => lateResultCase(run, 'settled') })
scenarios.push({ id: 'R04F', topology: 'vpn', run: run => lateResultCase(run, 'recoverable') })
scenarios.push({ id: 'R04E', topology: 'vpn', run: run => lateResultCase(run, 'recoverable', true) })

scenarios.push({ id: 'R05T', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, false)
  await beginRecovery(a)
  await b.call('settings_update', { patch: { sync: { sync_enabled: false } } })
  const text = `uc343-terminal-${randomUUID()}`
  run.canaries.push(text)
  const observed = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(observed.report, 'terminal_probe_not_dispatched')
  const view = await a.call('delivery', { entry: observed.report.entry_id })
  run.facts.terminal_delivery = view.deliveries.map(target => target.status)
  requireFact(view.deliveries.length === 1 && view.deliveries.every(target => target.status.kind === 'failed' && target.status.reason === 'peer_rejected'), 'actual_receiver_refusal_not_terminal')
  const first = await a.call('stream_fault', { action: 'recovery_snapshot' })
  while (true) {
    await delay(1_000)
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.terminal_recovery_observation = state
    requireFact(!state.events.some(event => ['retry_due', 'dispatch_started'].includes(event.kind) && event.trigger === 'retry'), 'terminal_refusal_entered_timed_retry')
    if (state.now_ns - first.now_ns >= 30_000_000_000) break
  }
  await receiveEvidence(run, b, receiveEpoch)
  for (const row of await b.call('history')) requireFact(hash((await b.call('entry', { entry: row.entry_id })).content ?? '') !== hash(text), 'terminal_receiver_stored_payload_before_resume')
  await a.call('suspend')
  await a.call('resume')
  await run.online()
  const final = await a.call('delivery', { entry: observed.report.entry_id })
  run.facts.terminal_after_resume = final.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
  requireFact(final.deliveries.length === 1 && final.deliveries.every(target => target.status.kind === 'failed' && target.status.reason === 'peer_rejected'), 'terminal_refusal_changed_after_resume')
  requireFact((await b.call('settings')).sync.sync_enabled === false, 'terminal_receive_setting_changed')
  await receiveEvidence(run, b, receiveEpoch)
  for (const row of await b.call('history')) requireFact(hash((await b.call('entry', { entry: row.entry_id })).content ?? '') !== hash(text), 'terminal_receiver_stored_refused_payload')
} })

scenarios.push({ id: 'R05O', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, true)
  await beginRecovery(a)
  await a.call('stream_fault', { action: 'arm', phase: 'BeforeOpen', fault: 'Hold', hits: 1 })
  const text = `uc343-pause-open-${randomUUID()}`
  run.canaries.push(text)
  const captured = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(captured.report, 'pause_open_copy_report_missing')
  const held = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.before_pause = held
  const hold = held.holds.find(hold => hold.phase === 'BeforeOpen')
  requireFact(hold && held.dropped_events === 0, 'acquired_before_open_hold_not_hit')
  requireFact(!held.events.some(event => event.attempt_alias === hold.attempt_alias && event.phase === 'BeforeFrame'), 'paused_attempt_already_opened_stream')
  const beforeReceive = await receiveEvidence(run, b, receiveEpoch)
  requireFact(!beforeReceive.events.some(event => ['applied', 'duplicate'].includes(event.stage)), 'pause_open_already_received_content')
  const pauseStarted = performance.now()
  await a.call('suspend')
  run.facts.pause_duration_ms = Math.round(performance.now() - pauseStarted)
  requireFact(run.facts.pause_duration_ms <= 20_000, 'suspend_did_not_cancel_held_open_attempt')
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'snapshot' })
    run.facts.after_pause = state
    return !state.holds.some(row => row.hold_alias === hold.hold_alias) && state.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === 'ReturnedFailure')
  }, 10_000, 'paused_open_attempt_not_drained')
  requireFact(!run.facts.after_pause.events.some(event => event.attempt_alias === hold.attempt_alias && event.phase === 'BeforeFrame'), 'old_request_opened_after_pause')
  const paused = await a.raw({ command: 'delivery', entry: captured.report.entry_id })
  run.facts.paused_delivery_query = { available: Boolean(paused.ok), code: paused.code }
  await until(async () => {
    const recovery = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.paused_recovery_observation = recovery
    return recovery.events.some(event => event.kind === 'result_recoverable')
  }, 10_000, 'paused_open_failure_not_recoverable')
  await a.call('resume')
  await run.online()
  await until(async () => {
    for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(text)) return true
    return false
  }, 25_000, 'paused_open_copy_not_recovered')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', bytes: Buffer.byteLength(text), sha256: hash(text), matched: true })
  await until(async () => {
    const view = await a.call('delivery', { entry: captured.report.entry_id })
    run.facts.resumed_sender_status = view.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
    return view.deliveries.some(target => ['delivered', 'duplicate'].includes(target.status.kind))
  }, 10_000, 'paused_open_sender_not_settled')
  const streams = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.stream_fault_final = streams
  requireFact(streams.dropped_events === 0 && streams.events.some(event => event.attempt_alias !== hold.attempt_alias && event.kind === 'ReturnedSuccess'), 'resumed_open_new_attempt_missing')
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(received.events.some(event => event.path === 'clipboard_push' && event.stage === 'applied'), 'resumed_open_push_commit_missing')
  requireFact(!received.events.some(event => event.path === 'active_pull' && ['applied', 'duplicate'].includes(event.stage)), 'resumed_open_active_pull_bypass')
} })

scenarios.push({ id: 'R05I', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, true)
  await beginRecovery(a)
  await a.call('stream_fault', { action: 'reset_acquisition' })
  await a.call('stream_fault', { action: 'arm_acquisition', hits: 1 })
  a.acquisitionObserved = true
  const text = `uc343-pause-acquire-${randomUUID()}`
  run.canaries.push(text)
  const captured = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(captured.report, 'acquisition_copy_report_missing')
  const acquisition = await a.call('stream_fault', { action: 'acquisition_snapshot' })
  run.facts.acquisition_before_pause = acquisition
  requireFact(acquisition.remaining_hits === 0 && acquisition.holds.length === 1, 'actual_acquisition_hold_not_hit')
  const hold = acquisition.holds[0]
  const events = acquisition.events.filter(event => event.attempt_alias === hold.attempt_alias)
  const held = events.find(event => event.kind === 'held' && event.hold_alias === hold.hold_alias)
  requireFact(held && events.some(event => event.kind === 'flight_registered' && event.seq < held.seq) && events.some(event => event.kind === 'backend_polled_pending' && event.seq < held.seq), 'pending_native_acquisition_not_proved')
  requireFact(!events.some(event => ['returned_success', 'returned_failure'].includes(event.kind)), 'acquisition_returned_before_pause')
  const streams = await a.call('stream_fault', { action: 'snapshot' })
  requireFact(!streams.events.some(event => event.attempt_alias === hold.attempt_alias && ['BeforeOpen', 'BeforeFrame'].includes(event.phase)), 'inflight_acquisition_already_entered_exchange')
  run.facts.acquisition_boundary = { native_pending_at_first_poll: true, lease_returned: false, native_still_pending_at_suspend_observed: false }
  const pausedAt = performance.now()
  await a.call('suspend')
  run.facts.pause_duration_ms = Math.round(performance.now() - pausedAt)
  requireFact(run.facts.pause_duration_ms <= 20_000, 'suspend_did_not_cancel_acquisition')
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'acquisition_snapshot' })
    run.facts.acquisition_after_pause = state
    requireFact(state.clock_alias === acquisition.clock_alias, 'acquisition_clock_changed')
    return !state.holds.some(row => row.hold_alias === hold.hold_alias) && state.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === 'returned_failure')
  }, 10_000, 'paused_acquisition_not_drained')
  await until(async () => {
    const state = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.paused_recovery_observation = state
    return state.events.some(event => event.kind === 'result_recoverable')
  }, 10_000, 'paused_acquisition_not_recoverable')
  const beforeReceive = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(!beforeReceive.events.some(event => ['applied', 'duplicate'].includes(event.stage)), 'paused_acquisition_delivered_content')
  await a.call('resume')
  await run.online()
  await until(async () => {
    for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(text)) return true
    return false
  }, 25_000, 'paused_acquisition_copy_not_recovered')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', bytes: Buffer.byteLength(text), sha256: hash(text), matched: true })
  await until(async () => {
    const view = await a.call('delivery', { entry: captured.report.entry_id })
    run.facts.resumed_sender_status = view.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
    return view.deliveries.some(target => ['delivered', 'duplicate'].includes(target.status.kind))
  }, 10_000, 'paused_acquisition_sender_not_settled')
  const final = await a.call('stream_fault', { action: 'snapshot' })
  run.facts.stream_fault_final = final
  requireFact(final.dropped_events === 0 && final.events.some(event => event.attempt_alias !== hold.attempt_alias && event.kind === 'ReturnedSuccess'), 'resumed_acquisition_new_attempt_missing')
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(received.events.some(event => event.path === 'clipboard_push' && event.stage === 'applied'), 'resumed_acquisition_push_commit_missing')
} })

scenarios.push({ id: 'R05A', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, true)
  await beginRecovery(a)
  await b.call('stream_fault', { action: 'arm', phase: 'ReceiverAppliedBeforeAck', fault: 'Hold', hits: 1 })
  const text = `uc343-pause-ack-${randomUUID()}`
  run.canaries.push(text)
  const captured = await a.call('capture', { kind: 'text', text, observed_at_ms: Date.now(), dispatch: true })
  requireFact(captured.report, 'pre_ack_copy_report_missing')
  const state = await b.call('stream_fault', { action: 'snapshot' })
  run.facts.receiver_before_pause = state
  const hold = state.holds.find(row => row.phase === 'ReceiverAppliedBeforeAck')
  const applied = state.events.find(event => event.attempt_alias === hold?.attempt_alias && event.kind === 'ReceiverApplied')
  requireFact(hold && applied?.receive_commit && state.dropped_events === 0, 'real_applied_before_ack_hold_missing')
  requireFact(!state.events.some(event => event.attempt_alias === hold.attempt_alias && event.kind === 'ReceiverAckWriteStarted'), 'ack_started_before_controlled_pause')
  const committed = applied.receive_commit
  const received = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(committed.epoch_alias === receiveEpoch && committed.path === 'clipboard_push' && received.events.some(event => event.seq === committed.applied_seq && event.stage === 'applied' && event.attempt_alias === committed.receive_attempt_alias && event.item_alias === committed.item_alias && event.entry_alias === committed.entry_alias), 'pre_ack_receipt_commit_association_invalid')
  let readback = false
  for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(text)) readback = true
  requireFact(readback, 'pre_ack_committed_hash_readback_missing')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', phase: 'before_pause_ack', bytes: Buffer.byteLength(text), sha256: hash(text), matched: true })
  const beforeSender = await a.call('delivery', { entry: captured.report.entry_id })
  run.facts.sender_before_pause = beforeSender.deliveries.map(target => target.status.kind)
  requireFact(beforeSender.deliveries.length === 1 && beforeSender.deliveries.every(target => target.status.kind === 'pending'), 'pre_ack_sender_already_settled')
  const pauseAt = performance.now()
  await a.call('suspend')
  run.facts.pause_duration_ms = Math.round(performance.now() - pauseAt)
  requireFact(run.facts.pause_duration_ms <= 20_000, 'suspend_did_not_close_pre_ack_exchange')
  let releaseRequested = false
  let controlledRelease = false
  await until(async () => {
    const receiver = await b.call('stream_fault', { action: 'snapshot' })
    run.facts.receiver_after_pause = receiver
    requireFact(receiver.dropped_events === 0, 'receiver_ack_evidence_overflow')
    const events = receiver.events.filter(event => event.attempt_alias === hold.attempt_alias)
    const abandoned = events.find(event => event.kind === 'ReceiverAckAbandoned')
    const cancelled = events.find(event => event.kind === 'HoldCancelled' && event.hold_alias === hold.hold_alias)
    const dropped = events.find(event => event.kind === 'Dropped')
    const stillHeld = receiver.holds.some(row => row.hold_alias === hold.hold_alias)
    if (abandoned || (cancelled && dropped)) {
      requireFact(!events.some(event => event.kind === 'ReceiverAckWriteStarted'), 'cancelled_pre_ack_exchange_started_ack')
      const endedAt = abandoned?.elapsed_micros ?? dropped.elapsed_micros
      requireFact(endedAt - applied.elapsed_micros < 60_000_000, 'pre_ack_cancellation_was_natural_deadline')
      run.facts.receiver_ack_release = { outcome: abandoned ? 'abandoned' : 'future_cancelled', controlled_release_after_sender_suspend: controlledRelease }
      return !stillHeld
    }
    if (stillHeld && !releaseRequested) {
      releaseRequested = true
      controlledRelease = (await b.call('stream_fault', { action: 'release_hold', hold_alias: hold.hold_alias })).released
      return false
    }
    if (controlledRelease && dropped && !events.some(event => event.kind === 'ReceiverAckWriteStarted')) {
      run.facts.receiver_ack_release = { outcome: 'future_cancelled_after_release', controlled_release_after_sender_suspend: true }
      return !stillHeld
    }
    const ended = events.some(event => event.kind === 'ReceiverExchangeEnded')
    if (controlledRelease && ended && events.some(event => event.kind === 'Released' && event.hold_alias === hold.hold_alias) && events.some(event => event.kind === 'ReceiverAckWriteStarted')) {
      run.facts.receiver_ack_release = { outcome: 'original_ack_helper_returned', controlled_release_after_sender_suspend: true }
      return !stillHeld
    }
    return false
  }, 10_000, 'pre_ack_receiver_exchange_not_drained')
  await until(async () => {
    const recovery = await a.call('stream_fault', { action: 'recovery_snapshot' })
    run.facts.paused_recovery_observation = recovery
    return recovery.events.some(event => event.kind === 'result_recoverable')
  }, 10_000, 'pre_ack_failure_not_recoverable')
  await a.call('resume')
  await run.online()
  await until(async () => {
    const view = await a.call('delivery', { entry: captured.report.entry_id })
    run.facts.resumed_sender_status = view.deliveries.map(target => ({ kind: target.status.kind, reason: target.status.reason }))
    return view.deliveries.some(target => ['delivered', 'duplicate'].includes(target.status.kind))
  }, 25_000, 'pre_ack_copy_not_settled_after_resume')
  const finalReceive = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(finalReceive.events.filter(event => event.path === 'clipboard_push' && event.stage === 'applied').length === 1, 'pre_ack_retry_persisted_second_copy')
  requireFact(finalReceive.events.some(event => event.path === 'clipboard_push' && event.stage === 'duplicate' && event.attempt_alias !== committed.receive_attempt_alias && event.entry_alias === committed.entry_alias), 'pre_ack_retry_duplicate_confirmation_missing')
  let finalMatched = false
  for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(text)) finalMatched = true
  requireFact(finalMatched, 'pre_ack_persisted_content_lost_after_resume')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', phase: 'after_resume_ack', bytes: Buffer.byteLength(text), sha256: hash(text), matched: true })
} })

scenarios.push({ id: 'R05G', topology: 'vpn', async run(run) {
  await run.pair()
  const [a, b] = run.nodes
  const receiveEpoch = await observeReceivePath(run, b, true)
  await a.call('sync_preference', { sync_on_restore: true, auto_sync_enabled: true })
  await b.call('sync_preference', { sync_on_restore: true, auto_sync_enabled: true })
  const seedText = `uc343-existing-seed-${randomUUID()}`
  const alternateText = `uc343-existing-alternate-${randomUUID()}`
  run.canaries.push(seedText, alternateText)
  const seed = await a.call('capture', { kind: 'text', text: seedText, observed_at_ms: Date.now(), dispatch: true })
  requireFact(seed.report?.entry_id, 'existing_seed_capture_missing')
  let seedReceiverEntry
  await until(async () => {
    for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(seedText)) { seedReceiverEntry = row.entry_id; return true }
    return false
  }, 25_000, 'existing_seed_readback_missing')
  const seeded = await receiveEvidence(run, b, receiveEpoch)
  const applied = seeded.events.filter(event => event.path === 'clipboard_push' && event.stage === 'applied')
  requireFact(applied.length === 1 && applied[0].entry_alias, 'existing_seed_commit_not_attributed')
  const seedCommit = applied[0]
  run.facts.seed_commit = { seq: seedCommit.seq, item_alias: seedCommit.item_alias, entry_alias: seedCommit.entry_alias }
  run.readbacks.push({ from: 'A', to: 'B', kind: 'text', phase: 'existing_seed', bytes: Buffer.byteLength(seedText), sha256: hash(seedText), matched: true })
  const alternate = await a.call('capture', { kind: 'text', text: alternateText, observed_at_ms: Date.now(), dispatch: true })
  requireFact(alternate.report?.entry_id, 'existing_alternate_capture_missing')
  let alternateReceiverEntry
  await until(async () => {
    for (const row of await b.call('history')) if (hash((await b.call('entry', { entry: row.entry_id })).content ?? '') === hash(alternateText)) { alternateReceiverEntry = row.entry_id; return true }
    return false
  }, 25_000, 'existing_alternate_readback_missing')
  requireFact((await b.call('restore', { entry: alternateReceiverEntry })).restored, 'existing_baseline_restore_failed')
  const clipboardContains = (snapshot, digest) => snapshot.representations.some(row => row.kind === 'inline' && row.sha256 === digest)
  await until(async () => clipboardContains(await b.call('clipboard'), hash(alternateText)) && (await b.call('active'))?.entry_id === alternateReceiverEntry, 10_000, 'existing_baseline_not_active')
  const baselineClipboard = await b.call('clipboard')
  const baselineActive = await b.call('active')
  const baseline = await receiveEvidence(run, b, receiveEpoch)
  const armSequence = baseline.events.at(-1)?.seq ?? 0
  run.facts.convergence_baseline = { os_write_calls: baseline.os_write_calls, register_sha256: hash(JSON.stringify(baselineActive)), clipboard: baselineClipboard }
  await b.call('stream_fault', { action: 'arm_convergence', hits: 1 })
  requireFact((await a.call('restore', { entry: seed.report.entry_id })).restored, 'existing_seed_reactivation_failed')
  let held, prepared
  await until(async () => {
    const state = await receiveEvidence(run, b, receiveEpoch)
    const holds = state.convergence_holds
    if (!holds.length) return false
    requireFact(holds.length === 1 && state.convergence_remaining_hits === 0, 'existing_convergence_hold_count_incorrect')
    held = holds[0]
    prepared = state.events.find(event => event.seq > armSequence && event.attempt_alias === held.attempt_alias && event.path === 'active_convergence' && event.stage === 'prepared')
    requireFact(prepared?.item_alias === seedCommit.item_alias && prepared.entry_alias === seedCommit.entry_alias, 'existing_hold_did_not_select_seed_entry')
    return state.events.some(event => event.attempt_alias === held.attempt_alias && event.stage === 'preparation_held')
  }, 10_000, 'existing_prepared_convergence_not_held')
  const disabled = await b.call('settings_update', { patch: { sync: { sync_enabled: false } } })
  requireFact(disabled.Updated && (await b.call('settings')).sync.sync_enabled === false, 'existing_convergence_disable_failed')
  requireFact((await b.call('stream_fault', { action: 'release_convergence', hold_alias: held.hold_alias })).released, 'existing_exact_convergence_release_failed')
  requireFact(!(await b.call('stream_fault', { action: 'release_convergence', hold_alias: held.hold_alias })).released, 'stale_convergence_token_released_twice')
  let rejected
  await until(async () => {
    const state = await receiveEvidence(run, b, receiveEpoch)
    const events = state.events.filter(event => event.attempt_alias === held.attempt_alias)
    rejected = events.find(event => event.stage === 'global_gate_rejected')
    const released = events.find(event => event.stage === 'preparation_released')
    requireFact(!events.some(event => ['preparation_cancelled', 'preparation_expired', 'write_handoff', 'os_write_started'].includes(event.stage)), 'disabled_existing_convergence_crossed_write_boundary')
    return rejected && released && released.seq < rejected.seq
  }, 10_000, 'existing_final_global_gate_not_observed')
  const rejectedAt = (await receiveEvidence(run, b, receiveEpoch)).now_ns
  while (true) {
    const state = await receiveEvidence(run, b, receiveEpoch)
    const events = state.events.filter(event => event.attempt_alias === held.attempt_alias)
    requireFact(state.os_write_calls === baseline.os_write_calls && !events.some(event => ['write_handoff', 'os_write_started', 'os_write_succeeded'].includes(event.stage)), 'global_disable_allowed_existing_os_write')
    requireFact(JSON.stringify(await b.call('active')) === JSON.stringify(baselineActive) && JSON.stringify(await b.call('clipboard')) === JSON.stringify(baselineClipboard), 'global_disable_changed_existing_register_or_clipboard')
    if (state.now_ns - rejectedAt >= 2_000_000_000) { run.facts.convergence_rejection_quiet_ns = state.now_ns - rejectedAt; break }
    await delay(100)
  }
  run.facts.rejected_convergence = { attempt_alias: held.attempt_alias, event_seq: rejected.seq, item_alias: prepared.item_alias, entry_alias: prepared.entry_alias }
  requireFact((await a.call('restore', { entry: alternate.report.entry_id })).restored, 'positive_activation_preparation_failed')
  const enabled = await b.call('settings_update', { patch: { sync: { sync_enabled: true } } })
  requireFact(enabled.Updated, 'existing_convergence_enable_failed')
  const positiveBaseline = await receiveEvidence(run, b, receiveEpoch)
  const positiveSequence = positiveBaseline.events.at(-1)?.seq ?? 0
  await b.call('stream_fault', { action: 'arm_convergence', hits: 1 })
  requireFact((await a.call('restore', { entry: seed.report.entry_id })).restored, 'positive_seed_reactivation_failed')
  let positiveHold
  await until(async () => {
    const state = await receiveEvidence(run, b, receiveEpoch)
    positiveHold = state.convergence_holds[0]
    if (!positiveHold) return false
    return state.events.some(event => event.seq > positiveSequence && event.attempt_alias === positiveHold.attempt_alias && event.stage === 'prepared' && event.item_alias === seedCommit.item_alias && event.entry_alias === seedCommit.entry_alias)
  }, 10_000, 'enabled_existing_convergence_not_held')
  requireFact((await b.call('stream_fault', { action: 'release_convergence', hold_alias: positiveHold.hold_alias })).released, 'enabled_existing_hold_release_failed')
  await until(async () => {
    const state = await receiveEvidence(run, b, receiveEpoch)
    const events = state.events.filter(event => event.attempt_alias === positiveHold.attempt_alias)
    const stages = ['prepared', 'preparation_held', 'preparation_released', 'write_handoff', 'os_write_started', 'os_write_succeeded'].map(stage => events.find(event => event.stage === stage))
    if (stages.some(event => !event)) return false
    requireFact(stages.every((event, index) => index === 0 || stages[index - 1].seq < event.seq), 'enabled_existing_os_write_order_incorrect')
    requireFact(state.os_write_calls > positiveBaseline.os_write_calls, 'enabled_existing_os_call_missing')
    return clipboardContains(await b.call('clipboard'), hash(seedText)) && (await b.call('active'))?.entry_id === seedReceiverEntry
  }, 10_000, 'enabled_existing_write_did_not_converge')
  const final = await isolatedReceiveEvidence(run, b, receiveEpoch)
  requireFact(final.events.filter(event => event.path === 'clipboard_push' && event.item_alias === seedCommit.item_alias && event.stage === 'applied').length === 1, 'existing_convergence_created_another_seed_entry')
  requireFact(!final.events.some(event => event.seq > armSequence && event.item_alias === seedCommit.item_alias && event.path === 'active_pull'), 'existing_convergence_used_new_pull')
  requireFact(final.convergence_holds.length === 0 && final.convergence_remaining_hits === 0, 'existing_convergence_hold_not_drained')
  run.readbacks.push({ from: 'A', to: 'B', kind: 'host_clipboard', phase: 'enabled_existing_convergence', bytes: Buffer.byteLength(seedText), sha256: hash(seedText), matched: true })
} })

const results = []
for (const scenario of scenarios.filter(scenario => !selected || selected.split(',').some(prefix => scenario.id.startsWith(prefix)))) {
  for (let iteration = 1; iteration <= repeat; iteration++) {
    const run = new Run(scenario.id, scenario.topology)
    const result = { id: scenario.id, iteration, topology: scenario.topology }
    try { await run.prepare(); await scenario.run(run); result.outcome = 'passed' }
    catch (error) {
      if (!(error instanceof Failure)) { result.outcome = 'test_error'; result.reason = 'unclassified_test_error'; result.test_error_type = error.name; console.error(error.stack?.replaceAll(run.root, '<profile>')) }
      else { result.outcome = 'failed'; result.failed_attempt = true; result.category = error.category; result.reason = error.message; result.failure_detail = error.detail; result.parser_detail = error.parser_detail; result.command_shape = error.command_shape }
    } finally {
      result.primary_reason = result.reason
      result.diagnostics = run.diagnostics()
      result.cleanup = await run.cleanup()
      if (result.outcome === 'passed' && run.subnetCaptures?.length && !run.facts.subnet_packet_acceptance?.passed) {
        result.outcome = 'failed'; result.failed_attempt = true; result.category = 'product'; result.reason = 'subnet_packet_boundary_not_proved'
      }
      if (!result.cleanup.cleaned || !result.cleanup.plaintext_clean) {
        result.cleanup_reason = 'cleanup_or_privacy_failed'
        if (result.outcome !== 'test_error') {
          result.outcome = 'failed'; result.failed_attempt = true; result.category = 'product'; result.reason = result.cleanup_reason
        }
      }
      result.duration_ms = Math.round(performance.now() - run.start)
      result.facts = run.facts
      result.readbacks = run.readbacks
      result.timeline = run.nodes.map(node => ({ node: node.alias, events: node.timeline }))
      results.push(result)
      writeFileSync(join(evidence, 'result.json'), JSON.stringify({ schema: 1, complete: false, binary_sha256: binarySha256, driver_sha256: driverSha256, results }, null, 2))
      console.log(JSON.stringify({ id: result.id, iteration, outcome: result.outcome, reason: result.reason, duration_ms: result.duration_ms }))
    }
    if (result.outcome === 'test_error') throw new Error('repair the E2E harness before continuing')
    if (result.outcome === 'failed') break
  }
}
if (!results.length) throw new Error('no matching scenario')
writeFileSync(join(evidence, 'result.json'), JSON.stringify({ schema: 1, complete: true, binary_sha256: binarySha256, driver_sha256: driverSha256, results }, null, 2))
process.exitCode = results.some(result => result.outcome !== 'passed') ? 1 : 0
