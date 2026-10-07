// Go 绑定真实进程验收驱动。
//
//   node tests/hosts/go/run-e2e.mjs --native <stage-native.sh 输出目录> --evidence <新的证据目录> [--go <go 可执行文件>] [--require-network-positive-control]
//
// 在 macOS 沙箱（拒绝全部网络）内依次运行三个独立进程：lifecycle（启动→查询→类型化错误→宿主回调→事件→
// 关闭）、restart（同一 profile 重启并核对身份）、negative（输入校验、清单错配、并发关闭等）。
// 证据目录保存每个命令、退出码、原始 stdout/stderr、工具版本、清单与产物哈希；任一阶段失败则退出码非 0。
// 只读写 --evidence 与其下的隔离根目录，不触碰真实 profile、钥匙串、剪贴板或网络。
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const options = new Map()
const args = process.argv.slice(2).filter((arg) => arg !== '--require-network-positive-control')
for (let index = 0; index < args.length; index += 2) {
  options.set(args[index], args[index + 1])
}
const native = resolve(options.get('--native') ?? '')
const evidence = resolve(options.get('--evidence') ?? '')
const go = options.get('--go') ?? 'go'
// CI 加此开关：沙箱外同一访问必须成功，才能证明“拒绝”来自沙箱而不是机器本身离线。
const requirePositiveControl = process.argv.includes('--require-network-positive-control')
if (!options.has('--native') || !options.has('--evidence')) {
  console.error('usage: run-e2e.mjs --native DIR --evidence DIR [--go PATH]')
  process.exit(2)
}
if (process.platform !== 'darwin') {
  console.error('network isolation uses sandbox-exec and is only implemented for macOS')
  process.exit(2)
}
if (existsSync(evidence) && readdirSync(evidence).length > 0) {
  console.error(`evidence directory must be empty: ${evidence}`)
  process.exit(2)
}
mkdirSync(evidence, { recursive: true, mode: 0o700 })

const hostDir = resolve(import.meta.dirname)
const manifestPath = join(native, 'native-manifest.json')
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex')
const commands = []

function run(name, command, args, extra = {}) {
  const started = Date.now()
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    timeout: extra.timeoutMs ?? 180_000,
    cwd: extra.cwd,
    env: { ...process.env, ...(extra.env ?? {}) },
    maxBuffer: 64 * 1024 * 1024,
  })
  writeFileSync(join(evidence, `${name}.stdout`), result.stdout ?? '')
  writeFileSync(join(evidence, `${name}.stderr`), result.stderr ?? '')
  const record = {
    name,
    command: [command, ...args],
    exit: result.status,
    signal: result.signal,
    timed_out: result.error?.code === 'ETIMEDOUT',
    duration_ms: Date.now() - started,
  }
  commands.push(record)
  return { ...record, stdout: result.stdout ?? '', stderr: result.stderr ?? '' }
}

const toolVersions = {
  go: run('tool-go-version', go, ['version']).stdout.trim(),
  node: process.version,
  host: run('tool-uname', 'uname', ['-srm']).stdout.trim(),
}

// 构建验收宿主：动态库经 -L 与 rpath 提供，loader 路径由命令显式给出而不是依赖开发者 shell。
const binary = join(evidence, 'go-engine-host')
const build = run('go-build', go, ['build', '-o', binary, '.'], {
  cwd: hostDir,
  env: { CGO_ENABLED: '1', CGO_LDFLAGS: `-L${native} -Wl,-rpath,${native}` },
})

const evidenceReal = realpathSync(evidence)
const isolatedHome = join(evidenceReal, 'home')
const isolatedTmp = join(evidenceReal, 'tmp')
mkdirSync(isolatedHome, { recursive: true, mode: 0o700 })
mkdirSync(isolatedTmp, { recursive: true, mode: 0o700 })
const realHome = process.env.HOME ?? ''

// 启动会绑定本机 P2P 套接字，因此允许 bind 与回环；其余入站/出站（含 DNS、中继、局域网发现）一律拒绝。
// 文件写入只允许证据目录与 /dev，读取拒绝真实用户的钥匙串目录；HOME 与 TMPDIR 改指证据目录。
const sandboxProfile = [
  '(version 1)(allow default)(deny network*)',
  '(allow network-bind)',
  '(allow network* (local ip "localhost:*") (remote ip "localhost:*"))',
  `(deny file-write* (require-not (require-any (subpath "${evidenceReal}") (subpath "/dev"))))`,
  realHome ? `(deny file-read* (subpath "${realHome}/Library/Keychains"))` : '',
].join('')
const sandboxed = (name, args, extra = {}) =>
  run(name, '/usr/bin/sandbox-exec', ['-p', sandboxProfile, ...args], {
    ...extra,
    env: { HOME: isolatedHome, TMPDIR: `${isolatedTmp}/`, ...(extra.env ?? {}) },
  })

// 控制实验。正控制：沙箱外同一访问应成功，否则无法区分“沙箱拒绝”与“本机离线”。
// 负控制：沙箱内域名 TCP、IP 字面量 TCP、UDP 与越界文件写入都必须被拒绝。
const curlArgs = (url) => ['/usr/bin/curl', '-sS', '--max-time', '5', '-o', '/dev/null', url]
const outside = run('network-positive-control-outside-sandbox', curlArgs('https://example.com/')[0], curlArgs('https://example.com/').slice(1), {
  timeoutMs: 20_000,
})
const controlDns = sandboxed('network-control-dns', curlArgs('https://example.com/'), { timeoutMs: 20_000 })
const controlIp = sandboxed('network-control-ip-tcp', curlArgs('https://1.1.1.1/'), { timeoutMs: 20_000 })
const udpProbe =
  "import socket,sys\ns=socket.socket(socket.AF_INET,socket.SOCK_DGRAM)\ns.settimeout(2)\ntry:\n s.sendto(b'x',('1.1.1.1',53))\nexcept OSError:\n sys.exit(0)\nsys.exit(1)"
const controlUdp = sandboxed('network-control-ip-udp', ['python3', '-c', udpProbe], { timeoutMs: 20_000 })
const writeProbe = join(realpathSync('/private/tmp'), `uc-go-binding-write-probe-${process.pid}`)
const controlWrite = sandboxed('file-write-control', ['/usr/bin/touch', writeProbe], { timeoutMs: 20_000 })
const writeEscaped = existsSync(writeProbe)
if (writeEscaped) rmSync(writeProbe)
const networkDenied =
  controlDns.exit !== 0 && controlIp.exit !== 0 && controlUdp.exit === 0 && controlWrite.exit !== 0 && !writeEscaped
const positiveControl = outside.exit === 0
const isolationProven = networkDenied && positiveControl

const phases = []
if (build.exit === 0 && networkDenied && (!requirePositiveControl || positiveControl)) {
  const root = join(evidence, 'root')
  const negativeRoot = join(evidence, 'root-negative')
  mkdirSync(root, { recursive: true, mode: 0o700 })
  mkdirSync(negativeRoot, { recursive: true, mode: 0o700 })
  for (const [phase, phaseRoot] of [
    ['lifecycle', root],
    ['restart', root],
    ['negative', negativeRoot],
  ]) {
    const result = sandboxed(`phase-${phase}`, [binary, '--phase', phase, '--root', phaseRoot, '--manifest', manifestPath])
    let parsed = null
    try {
      parsed = JSON.parse(result.stdout)
    } catch {
      parsed = null
    }
    phases.push({ phase, exit: result.exit, ok: result.exit === 0 && parsed?.ok === true, report: parsed })
    if (result.exit !== 0 && phase === 'lifecycle') break
  }
}

// 链接进产物的来源身份：Rust 观测日志的每条记录携带构建时嵌入的 source_commit/source_state，
// 必须与来源清单的 revision 一致且工作区干净，证明被执行的库就是清单所述的源码。
const sourceCommits = new Set()
const sourceStates = new Set()
const logDir = join(evidence, 'root', 'cache', 'logs')
if (existsSync(logDir)) {
  for (const name of readdirSync(logDir)) {
    for (const line of readFileSync(join(logDir, name), 'utf8').split('\n')) {
      try {
        const record = JSON.parse(line)
        if (record.source_commit) sourceCommits.add(record.source_commit)
        if (record.source_state) sourceStates.add(record.source_state)
      } catch {
        // 非 JSON 行不含来源字段。
      }
    }
  }
}
const sourceIdentity = {
  manifest_revision: manifest.engine_revision,
  embedded_source_commits: [...sourceCommits],
  embedded_source_states: [...sourceStates],
  matches:
    sourceCommits.size === 1 &&
    sourceCommits.has(manifest.engine_revision) &&
    sourceStates.size === 1 &&
    sourceStates.has('clean'),
}

const allOk =
  build.exit === 0 &&
  networkDenied &&
  (!requirePositiveControl || positiveControl) &&
  sourceIdentity.matches &&
  phases.length === 3 &&
  phases.every((phase) => phase.ok)
const summary = {
  ok: allOk,
  source_identity: sourceIdentity,
  isolation: {
    mechanism: 'sandbox-exec: network denied except bind and loopback; file writes limited to the evidence directory; real Keychains directory unreadable; HOME/TMPDIR redirected',
    profile: sandboxProfile.replace(realHome, '<HOME>'),
    controls: {
      outside_sandbox_exit: outside.exit,
      sandbox_dns_tcp_exit: controlDns.exit,
      sandbox_ip_tcp_exit: controlIp.exit,
      sandbox_udp_denied: controlUdp.exit === 0,
      sandbox_out_of_tree_write_exit: controlWrite.exit,
      out_of_tree_file_created: writeEscaped,
    },
    denied: networkDenied,
    positive_control_succeeded: positiveControl,
    isolation_proven: isolationProven,
  },
  tools: toolVersions,
  native: {
    manifest: manifest,
    manifest_sha256: sha256(manifestPath),
    library_sha256_on_disk: sha256(join(native, manifest.library.file)),
  },
  host_binary_sha256: existsSync(binary) ? sha256(binary) : null,
  phases,
  commands,
}
writeFileSync(join(evidence, 'e2e-result.json'), `${JSON.stringify(summary, null, 2)}\n`)
console.log(JSON.stringify({ ok: allOk, evidence }, null, 2))
process.exit(allOk ? 0 : 1)
