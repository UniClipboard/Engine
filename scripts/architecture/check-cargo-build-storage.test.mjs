#!/usr/bin/env node

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const CHECKER = join(SCRIPT_DIR, 'check-cargo-build-storage.mjs')

function runChecker(target) {
  return spawnSync(process.execPath, [CHECKER], {
    encoding: 'utf8',
    env: { ...process.env, CARGO_TARGET_DIR: target },
  })
}

test('拒绝内置临时目录中的 Cargo 构建产物', () => {
  const target = mkdtempSync(join(tmpdir(), 'uc-cargo-target-test-'))
  try {
    const result = runChecker(target)
    assert.equal(result.status, 1)
    assert.match(result.stderr, /内置临时目录/)
  } finally {
    rmSync(target, { recursive: true, force: true })
  }
})

test('拒绝目标已丢失的符号链接', () => {
  const fixture = mkdtempSync(join(process.cwd(), '.cargo-target-link-test-'))
  const target = join(fixture, 'target')
  symlinkSync(join(fixture, 'missing'), target)
  try {
    const result = runChecker(target)
    assert.equal(result.status, 1)
    assert.match(result.stderr, /链接目标不存在/)
  } finally {
    rmSync(fixture, { recursive: true, force: true })
  }
})

test('接受仓库恢复后的默认 Cargo 构建目录', () => {
  const result = spawnSync(process.execPath, [CHECKER], {
    encoding: 'utf8',
    env: Object.fromEntries(
      Object.entries(process.env).filter(([name]) => name !== 'CARGO_TARGET_DIR')
    ),
  })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /Cargo 构建目录检查通过/)
})


test('拒绝单独覆盖到内置临时目录的编译中间文件', () => {
  const result = spawnSync(process.execPath, [CHECKER], {
    encoding: 'utf8',
    env: { ...process.env, CARGO_BUILD_BUILD_DIR: '/private/tmp/uc-hidden-build-cache' },
  })
  assert.equal(result.status, 1)
})
