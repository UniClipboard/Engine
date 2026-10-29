#!/usr/bin/env node
// 固定旧源码、相同测试与真实二进制，验证双向混合密码消息及解密后资料兼容。
import { execFileSync, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const oldRevision = '11470feee1cc5d6e8e4b77b6b1f12ddaedeff628'
const testPath = 'crates/uc-infra-crypto/tests/opaque_dependency_compat.rs'
const reportDir = resolve(process.env.UC_OPAQUE_COMPAT_REPORT_DIR ?? join(root, 'target/test-reports/opaque-compat'))
mkdirSync(reportDir, { recursive: true })
mkdirSync(join(root, '.planning'), { recursive: true })
const target = join(root, 'target')
if (!existsSync(target)) throw new Error('shared target must exist before compatibility verification')
const scratch = mkdtempSync(join(root, '.planning/opaque-compat-'))
const sha = bytes => createHash('sha256').update(bytes).digest('hex')
const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim()
function build(source, label) {
  const args = ['cargo', 'test', '--manifest-path', join(source, 'Cargo.toml'), '--target-dir', target,
    '-p', 'uc-infra-crypto', '--test', 'opaque_dependency_compat', '--no-run', '--locked', '--message-format=json']
  // 归档源码没有独立.git，显式标明旧来源，防止误读父仓库当前HEAD。
  const env = label === 'old'
    ? { ...process.env, UC_ENGINE_SOURCE_COMMIT: oldRevision, UC_ENGINE_SOURCE_STATE: 'modified' }
    : process.env
  const result = spawnSync('just', args, { cwd: root, env, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 })
  writeFileSync(join(reportDir, `${label}-build.jsonl`), result.stdout ?? '')
  writeFileSync(join(reportDir, `${label}-build.log`), result.stderr ?? '')
  if (result.status !== 0) throw new Error(`${label} build failed; inspect retained compiler log`)
  const artifact = result.stdout.split('\n').filter(Boolean).map(line => JSON.parse(line))
    .find(row => row.reason === 'compiler-artifact' && row.target.name === 'opaque_dependency_compat' && row.executable)
  if (!artifact) throw new Error(`${label} test binary not found`)
  return artifact.executable
}
try {
  const source = join(scratch, 'source')
  mkdirSync(source)
  if (spawnSync('git', ['cat-file', '-e', `${oldRevision}^{commit}`], { cwd: root, stdio: 'ignore' }).status !== 0) {
    execFileSync('git', ['fetch', '--no-tags', 'https://github.com/UniClipboard/Engine.git', oldRevision], { cwd: root, stdio: 'inherit' })
  }
  const archive = execFileSync('git', ['archive', oldRevision], { cwd: root, maxBuffer: 512 * 1024 * 1024 })
  execFileSync('tar', ['-x', '-C', source], { input: archive })
  const submodule = git(['rev-parse', `${oldRevision}:third_party/iroh`])
  // 旧版 gitlink 与当前子模块可以不同。只按旧版提交归档，不切换当前工作树。
  const networkSource = join(root, 'third_party/iroh')
  if (spawnSync('git', ['-C', networkSource, 'cat-file', '-e', `${submodule}^{commit}`], { stdio: 'ignore' }).status !== 0) {
    execFileSync('git', ['-C', networkSource, 'fetch', '--no-tags', 'origin', submodule], { stdio: 'inherit' })
  }
  const oldNetwork = join(source, 'third_party/iroh')
  rmSync(oldNetwork, { recursive: true, force: true })
  mkdirSync(oldNetwork, { recursive: true })
  const networkArchive = execFileSync('git', ['-C', networkSource, 'archive', submodule], { maxBuffer: 512 * 1024 * 1024 })
  execFileSync('tar', ['-x', '-C', oldNetwork], { input: networkArchive })
  mkdirSync(join(source, 'crates/uc-infra-crypto/tests'), { recursive: true })
  copyFileSync(join(root, testPath), join(source, testPath))
  const oldBinary = join(scratch, 'fixed-old-peer')
  copyFileSync(build(source, 'old'), oldBinary)
  const candidate = build(root, 'candidate')
  const env = { ...process.env, UC_OPAQUE_OLD_PEER: oldBinary }
  const checks = []
  for (const [label, binary] of [['old-old', oldBinary], ['mixed', candidate]]) {
    const result = spawnSync(binary, ['--ignored', '--exact', 'fixed_old_and_candidate_are_compatible'], {
      cwd: root, env, encoding: 'utf8', timeout: 120_000,
    })
    writeFileSync(join(reportDir, `${label}.log`), `${result.stdout ?? ''}${result.stderr ?? ''}`)
    checks.push({ check: label, exitCode: result.status, signal: result.signal })
    if (result.status !== 0) throw new Error(`${label} compatibility failed`)
  }
  const report = {
    oldRevision, oldTree: git(['rev-parse', `${oldRevision}^{tree}`]), oldSubmodule: submodule,
    candidateRevision: git(['rev-parse', 'HEAD']), candidateIndexTree: git(['write-tree']),
    candidateDirty: Boolean(git(['status', '--porcelain'])),
    testSha256: sha(readFileSync(join(root, testPath))), oldLockSha256: sha(readFileSync(join(source, 'Cargo.lock'))),
    candidateLockSha256: sha(readFileSync(join(root, 'Cargo.lock'))),
    oldBinarySha256: sha(readFileSync(oldBinary)), candidateBinarySha256: sha(readFileSync(candidate)), checks,
  }
  writeFileSync(join(reportDir, 'report.json'), `${JSON.stringify(report, null, 2)}\n`)
  process.stdout.write('Fixed-old baseline and mixed OPAQUE compatibility passed; evidence retained\n')
} finally {
  rmSync(scratch, { recursive: true, force: true })
}
