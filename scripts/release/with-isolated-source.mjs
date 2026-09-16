#!/usr/bin/env node

import { execFileSync, spawn } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join, relative, resolve } from 'node:path'

const [scratchArg, command, ...args] = process.argv.slice(2)
if (!scratchArg || !command) throw new Error('usage: with-isolated-source.mjs <external-scratch-root> <command> [args...]')
const scratch = realpathSync(scratchArg)
const userHome = realpathSync(homedir())
const withinHome = relative(userHome, scratch)
if (!withinHome || (!withinHome.startsWith('..') && !withinHome.startsWith('/'))) {
  throw new Error('release scratch must be outside the user home')
}
for (let parent = scratch; ; parent = dirname(parent)) {
  for (const name of ['config', 'config.toml']) {
    if (existsSync(join(parent, '.cargo', name))) throw new Error('release scratch inherits an ancestor Cargo configuration')
  }
  if (dirname(parent) === parent) break
}
const repository = execFileSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' }).trim()
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
const directory = mkdtempSync(join(scratch, 'engine-release-'))
const source = join(directory, 'source')
const cargoHome = join(directory, 'cargo')
const originalCargoHome = resolve(process.env.CARGO_HOME || join(userHome, '.cargo'))
try {
  execFileSync('git', ['clone', '--quiet', '--no-hardlinks', '--no-checkout', repository, source], { stdio: 'inherit' })
  execFileSync('git', ['-C', source, 'checkout', '--quiet', '--detach', commit], { stdio: 'inherit' })
  mkdirSync(cargoHome)
  // 只复用下载内容，用户配置、源码和编译产物均不共享。
  for (const name of ['registry', 'git']) {
    const cache = join(originalCargoHome, name)
    if (existsSync(cache)) symlinkSync(cache, join(cargoHome, name), 'dir')
  }
  const child = spawn(command, args, {
    cwd: source,
    stdio: 'inherit',
    detached: true,
    env: { ...process.env, CARGO_HOME: cargoHome, CARGO_TARGET_DIR: join(source, 'target'), UC_OHOS_TARGET_DIR: join(source, 'target') },
  })
  const stop = signal => {
    if (child.pid) {
      try { process.kill(-child.pid, signal) } catch (error) { if (error.code !== 'ESRCH') throw error }
    }
  }
  const interrupt = () => stop('SIGINT')
  const terminate = () => stop('SIGTERM')
  process.on('SIGINT', interrupt)
  process.on('SIGTERM', terminate)
  try {
    process.exitCode = await new Promise((resolveExit, reject) => {
      child.on('error', reject)
      child.on('close', (code, signal) => resolveExit(code ?? (signal === 'SIGINT' ? 130 : 143)))
    })
  } finally {
    process.off('SIGINT', interrupt)
    process.off('SIGTERM', terminate)
  }
} finally {
  rmSync(directory, { recursive: true, force: true })
}
