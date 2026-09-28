#!/usr/bin/env node
// 守护 CI 编译缓存的信任边界与配置一致性；取舍见 docs/design-docs/decisions/029-ci-r2-compile-cache.md。

import assert from 'node:assert/strict'
import { readdirSync, readFileSync } from 'node:fs'
import test from 'node:test'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const read = (path) => readFileSync(resolve(root, path), 'utf8')
const workflowDir = '.github/workflows'
const workflows = Object.fromEntries(
  readdirSync(resolve(root, workflowDir))
    .filter((name) => name.endsWith('.yml'))
    .map((name) => [name, read(`${workflowDir}/${name}`)]),
)
const action = read('.github/actions/rust-ci-setup/action.yml')
const startScript = read('scripts/build-cache/start-ci-sccache.sh')

const WRITER_ON_MAIN =
  "${{ github.ref == 'refs/heads/main' && 'engine-build-cache-writer' || 'engine-build-cache-reader' }}"
const ACCESS_ON_MAIN = "${{ github.ref == 'refs/heads/main' && 'write' || 'read' }}"

// 按两格缩进的 job 键切分；仓库工作流统一使用该缩进。
function jobsOf(workflow) {
  const body = workflow.slice(workflow.indexOf('\njobs:\n'))
  return body.split(/\n(?=  [A-Za-z0-9_-]+:\n)/).slice(1)
}

test('every CI Rust job binds the cache environment by ref and passes only its secrets', () => {
  for (const [name, workflow] of Object.entries(workflows)) {
    if (name === 'compile-cache-benchmark.yml') continue
    for (const job of jobsOf(workflow)) {
      if (!job.includes('uses: ./.github/actions/rust-ci-setup')) continue
      const label = `${name}: ${job.split('\n')[0].trim()}`
      assert.ok(job.includes(`name: ${WRITER_ON_MAIN}`), `${label} 未按 main 绑定写入/只读环境`)
      assert.match(job, /\n {6}deployment: false\n/, `${label} 不应创建部署记录`)
      assert.ok(job.includes(`compile-cache-access: ${ACCESS_ON_MAIN}`), `${label} 读写模式与环境不一致`)
      for (const secret of ['ENDPOINT', 'ACCESS_KEY_ID', 'SECRET_ACCESS_KEY']) {
        assert.match(job, new RegExp(`\\$\\{\\{ secrets\\.BUILD_CACHE_R2_${secret} \\}\\}`), label)
      }
    }
  }
})

test('the benchmark writes only through the branch-restricted writer environment and a disposable prefix', () => {
  const benchmark = workflows['compile-cache-benchmark.yml']
  assert.match(benchmark, /on:\n  workflow_dispatch:/)
  assert.match(benchmark, /environment:\n {6}name: engine-build-cache-writer\n {6}deployment: false/)
  assert.match(benchmark, /compile-cache-key-prefix: engine\/sccache-bench\/\$\{\{ github\.run_id \}\}/)
  assert.match(benchmark, /dependency-cache: "false"/)
})

test('release workflows and untrusted triggers never receive compile cache credentials', () => {
  for (const [name, workflow] of Object.entries(workflows)) {
    assert.doesNotMatch(workflow, /pull_request_target/, `${name} 不得使用 pull_request_target`)
    if (name.startsWith('release-')) {
      assert.doesNotMatch(workflow, /BUILD_CACHE_|engine-build-cache-|rust-ci-setup/, `${name} 标签构建不得接入跨运行编译缓存`)
    }
  }
})

test('credentials reach only the sccache server, never GITHUB_ENV', () => {
  assert.doesNotMatch(action, /secret-access-key[^\n]*GITHUB_ENV|AWS_[A-Z_]*=.*GITHUB_ENV/)
  const exported = startScript.slice(startScript.lastIndexOf('{\n  echo "RUSTC_WRAPPER'))
  assert.doesNotMatch(exported, /AWS_|SECRET|ENDPOINT|KEY_ID/)
  assert.match(startScript, /unset endpoint key_id secret/)
})

test('the pinned sccache version matches between the action and the binary digests', () => {
  const actionVersion = action.match(/\n {8}version: (v\d+\.\d+\.\d+)\n/)?.[1]
  const scriptVersion = startScript.match(/\nSCCACHE_VERSION=(v\d+\.\d+\.\d+)\n/)?.[1]
  assert.ok(actionVersion, 'sccache-action 必须显式固定 sccache 版本')
  assert.equal(actionVersion, scriptVersion)
  assert.match(action, /mozilla-actions\/sccache-action@[0-9a-f]{40}/)
  assert.match(startScript, /Linux-x86_64\) echo [0-9a-f]{64} ;;/)
  assert.match(startScript, /Darwin-arm64\) echo [0-9a-f]{64} ;;/)
})

test('build parallelism is numeric for tools that parse CARGO_BUILD_JOBS themselves', () => {
  assert.doesNotMatch(action, /CARGO_BUILD_JOBS=default/)
  assert.match(startScript, /echo "CARGO_BUILD_JOBS=\$\(logical_cpus\)"/)
})

test('each backend attempt runs a foreground server owned by the script, so a timed-out attempt cannot linger', () => {
  assert.match(startScript, /SCCACHE_START_SERVER=1 SCCACHE_NO_DAEMON=1 SCCACHE_IDLE_TIMEOUT=0/)
  assert.doesNotMatch(startScript, /--start-server/)
  assert.match(startScript, /start_server\(\) \{\n  kill_own_server\n/)
  assert.match(startScript, /echo "SCCACHE_SERVER_PORT=\$server_port"/)
})
