#!/usr/bin/env node

import { lstatSync, realpathSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
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

function main() {
  const configured = process.env.CARGO_TARGET_DIR
  const target = configured
    ? resolve(REPOSITORY_ROOT, configured)
    : join(REPOSITORY_ROOT, 'target')

  if (canonicalTemporaryRoots().some(root => isInside(target, root))) {
    fail(`不得把 CARGO_TARGET_DIR 放在内置临时目录：${target}`)
    return
  }

  let targetInfo
  try {
    targetInfo = lstatSync(target)
  } catch (error) {
    if (error?.code === 'ENOENT' && !configured) {
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

main()
