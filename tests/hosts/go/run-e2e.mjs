// Go 绑定真实进程验收驱动。
//
//   node tests/hosts/go/run-e2e.mjs --native <stage-native.sh 输出目录> --evidence <新的证据目录> [--go <go 可执行文件>]
//
// 在 macOS 沙箱（拒绝全部网络）内依次运行三个独立进程：lifecycle（启动→查询→类型化错误→宿主回调→事件→
// 关闭）、restart（同一 profile 重启并核对身份）、negative（输入校验、清单错配、并发关闭等）。
// 证据目录保存每个命令、退出码、原始 stdout/stderr、工具版本、清单与产物哈希；任一阶段失败则退出码非 0。
// 只读写 --evidence 与其下的隔离根目录，不触碰真实 profile、钥匙串、剪贴板或网络。
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const options = new Map()
for (let index = 2; index < process.argv.length; index += 2) {
  options.set(process.argv[index], process.argv[index + 1])
}
const native = resolve(options.get('--native') ?? '')
const evidence = resolve(options.get('--evidence') ?? '')
const go = options.get('--go') ?? 'go'
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

// 启动会绑定本机 P2P 套接字，因此允许 bind 与回环；其余入站/出站（含 DNS、中继、局域网发现）一律拒绝。
const sandboxProfile = [
  '(version 1)(allow default)(deny network*)',
  '(allow network-bind)',
  '(allow network* (local ip "localhost:*") (remote ip "localhost:*"))',
].join('')
const sandboxed = (name, args, extra) =>
  run(name, '/usr/bin/sandbox-exec', ['-p', sandboxProfile, ...args], extra)

// 控制实验：同一沙箱下真实网络访问必须失败，否则“离线”不成立。
const control = sandboxed('network-control', ['/usr/bin/curl', '-sS', '--max-time', '5', 'https://example.com/'], {
  timeoutMs: 20_000,
})
const networkDenied = control.exit !== 0

const phases = []
if (build.exit === 0 && networkDenied) {
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

const allOk =
  build.exit === 0 && networkDenied && phases.length === 3 && phases.every((phase) => phase.ok)
const summary = {
  ok: allOk,
  network_isolation: { mechanism: 'sandbox-exec (deny network*; allow bind and loopback only)', profile: sandboxProfile, control_exit: control.exit, denied: networkDenied },
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
