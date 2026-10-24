#!/usr/bin/env node
// 仅显式人工运行：本脚本不挂起系统，操作者须在探针启动后自行挂起至少10秒并恢复。
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createReadStream, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

if (process.argv.length !== 3 || process.argv[2] !== '--physical') {
  throw new Error('usage: node scripts/testing/run-progress-suspend-proof.mjs --physical; manually suspend after the test starts')
}
const root = resolve(import.meta.dirname, '../..')
const directory = resolve(root, process.env.UC_TEST_ARTIFACTS_DIR ?? 'target/test-artifacts/ci', 'progress-suspend')
mkdirSync(directory, { recursive: true })
const sample = resolve(directory, 'native-sample.json')
const report = resolve(directory, 'report.json')
for (const file of [sample, report]) rmSync(file, { force: true })
const built = spawnSync('just', ['cargo', 'test', '-p', 'uc-infra-local', '--lib', '--no-run', '--locked', '--message-format=json'], {
  cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'],
})
if (built.error) throw built.error
if (built.status !== 0) {
  process.stderr.write(built.stdout)
  process.exit(built.status ?? 1)
}
const artifacts = built.stdout.split('\n').filter(Boolean).map(line => JSON.parse(line)).filter(row =>
  row.reason === 'compiler-artifact' && row.target.name === 'uc_infra_local' && row.profile.test && row.executable)
if (artifacts.length !== 1) throw new Error('expected one current native-clock test binary')
const binary = artifacts[0].executable
const test = 'time::elapsed_clock::suspend_probe::physical_suspend_consumes_original_elapsed_budgets'
const run = spawnSync(binary, [test, '--exact', '--ignored', '--nocapture'], {
  cwd: root, env: { ...process.env, UC_ELAPSED_SUSPEND_REPORT: sample }, stdio: 'inherit',
})
if (run.error) throw run.error
const git = (...args) => {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error('cannot record source provenance')
  return result.stdout.trim()
}
async function sha(file) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(file)) hash.update(chunk)
  return hash.digest('hex')
}
const observed = run.status === 0 ? JSON.parse(readFileSync(sample, 'utf8')) : null
if (run.status === 0 && (observed.physical_suspend_observed !== true || observed.suspend_millis_lower_bound < 10000 ||
    observed.original_5_second_budget_expired !== true)) throw new Error('physical suspend evidence is missing or invalid')
writeFileSync(report, JSON.stringify({
  source_commit: git('rev-parse', 'HEAD'),
  source_tree: git('rev-parse', 'HEAD^{tree}'),
  source_state: git('status', '--porcelain') ? 'modified' : 'clean',
  lockfile_sha256: await sha(resolve(root, 'Cargo.lock')),
  binary_sha256: await sha(binary),
  test, observed, exit_code: run.status, signal: run.signal,
}, null, 2) + '\n')
process.exit(run.status ?? 1)
