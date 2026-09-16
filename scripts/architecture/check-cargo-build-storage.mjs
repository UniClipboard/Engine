#!/usr/bin/env node

import { spawnSync } from 'node:child_process'
import { lstatSync, realpathSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = realpathSync(resolve(SCRIPT_DIR, '../..'))

function isInside(path, root) {
  const offset = relative(root, path)
  const separator = process.platform === 'win32' ? '\\' : '/'
  return (
    offset === '' ||
    (!isAbsolute(offset) && offset !== '..' && !offset.startsWith(`..${separator}`))
  )
}

function fail(message) {
  process.stderr.write(`ERROR Cargo 构建目录检查失败：${message}\n`)
  process.exitCode = 1
}

function canonicalTemporaryRoots() {
  const roots = new Set(['/tmp', '/private/tmp', tmpdir()].map(path => resolve(path)))
  for (const root of [...roots]) {
    try {
      roots.add(realpathSync(root))
    } catch {
      // 不存在的系统临时目录不会成为当前路径的父目录。
    }
  }
  return [...roots]
}

function checkTarget(target, allowMissing) {

  if (canonicalTemporaryRoots().some(root => isInside(target, root))) {
    fail(`不得把 CARGO_TARGET_DIR 放在内置临时目录：${target}`)
    return
  }

  let targetInfo
  try {
    targetInfo = lstatSync(target)
  } catch (error) {
    if (error?.code === 'ENOENT' && allowMissing) {
      process.stdout.write(`Cargo 构建目录检查通过：${target} 将由 Cargo 创建\n`)
      return
    }
    fail(`目录不可用：${target}`)
    return
  }

  let canonicalTarget
  try {
    canonicalTarget = realpathSync(target)
  } catch (error) {
    if (targetInfo.isSymbolicLink() && error?.code === 'ENOENT') {
      fail(`链接目标不存在：${target}`)
      return
    }
    fail(`无法解析目录：${target}`)
    return
  }

  if (!statSync(canonicalTarget).isDirectory()) {
    fail(`目标不是目录：${canonicalTarget}`)
    return
  }
  if (canonicalTemporaryRoots().some(root => isInside(canonicalTarget, root))) {
    fail(`不得把 CARGO_TARGET_DIR 放在内置临时目录：${canonicalTarget}`)
    return
  }

  process.stdout.write(`Cargo 构建目录检查通过：${canonicalTarget}\n`)
}

function main() {
  // 独立中间目录与最终目录必须同时检查；不能只检查仓库内的 target 链接。
  const explicitTarget = process.env.CARGO_TARGET_DIR ?? process.env.CARGO_BUILD_TARGET_DIR
  const explicitBuild = process.env.CARGO_BUILD_BUILD_DIR
  for (const target of [explicitTarget, explicitBuild].filter(Boolean)) {
    checkTarget(resolve(REPOSITORY_ROOT, target), false)
  }
  if (process.exitCode) return
  if (explicitTarget && explicitBuild) return

  const metadata = spawnSync('cargo', ['metadata', '--no-deps', '--offline', '--format-version', '1'], {
    cwd: REPOSITORY_ROOT,
    encoding: 'utf8',
  })
  if (metadata.status !== 0) {
    fail('无法读取 Cargo 实际使用的编译目录')
    return
  }
  const result = JSON.parse(metadata.stdout)
  for (const target of new Set([result.target_directory, result.build_directory].filter(Boolean))) {
    checkTarget(resolve(target), true)
  }
}

main()
