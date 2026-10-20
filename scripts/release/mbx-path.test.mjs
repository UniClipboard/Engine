import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { resolve } from 'node:path'
import test from 'node:test'

const wrapper = resolve(import.meta.dirname, '../build-cache/mbx.sh')

test('MBX wrapper removes every repository Cargo entry from the nested PATH', () => {
  // 隔离源码构建的 PATH 同时带有主仓、克隆源码和受信工具三份入口；任何一份残留都会让垫片互相递归。
  const entries = ['/a/scripts/build-cache/bin', '/b/scripts/build-cache/bin', '/c/scripts/build-cache/bin', '/usr/bin', '/bin']
  const result = spawnSync('bash', [wrapper, '--exec', 'printenv', 'PATH'], {
    encoding: 'utf8',
    env: { ...process.env, PATH: entries.join(':'), UC_ENGINE_MBX_BIN: '/c/scripts/build-cache/bin' },
  })
  assert.equal(result.status, 0, result.stderr)
  const nested = result.stdout.trim().split(':')
  assert.deepEqual(nested.filter(entry => entry.endsWith('/scripts/build-cache/bin')), [])
})
