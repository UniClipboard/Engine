#!/usr/bin/env node
// 禁止直接使用 tracing 日志宏（ADR-030）。
//
// 用 clippy 的 disallowed_macros 统计 `tracing::{trace,debug,info,warn,error}!` 的直接使用，
// 默认特性与 lan-compat 各跑一轮取并集；任何一处都失败。日志一律使用 `uc_*!` 宏，
// 由字段目录与值类别在编译期约束。clippy 只认 crate 级 allow，故意保留原始 tracing 的测试文件
// 在文件顶部用 `#![allow(clippy::disallowed_macros)]` 并写明理由。
// 配置只通过 CLIPPY_CONF_DIR 启用，不影响日常 cargo clippy。
//
// 用法：
//   node scripts/architecture/check-direct-log-macros.mjs
//   node scripts/architecture/check-direct-log-macros.mjs --from-json <文件>...   读取已有的 clippy JSON

import { spawnSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const SCRIPT_PATH = fileURLToPath(import.meta.url)
const REPOSITORY_ROOT = resolve(dirname(SCRIPT_PATH), '../..')
const CLIPPY_CONFIG_DIR = resolve(REPOSITORY_ROOT, 'scripts/architecture/log-macro-clippy')
const LINT = 'clippy::disallowed_macros'
// `--cap-lints warn` 让仓库里已有的 deny 级 clippy 错误不阻断依赖 crate 的检查；只统计目标 lint。
const CLIPPY_LINT_ARGS = ['--cap-lints', 'warn', '-A', 'clippy::all', '-W', LINT]
const BASE_ARGS = ['clippy', '--workspace', '--all-targets', '--locked', '--keep-going', '--message-format=json']
// 默认特性之外，lan-compat 的代码只在该特性下编译。
const RUNS = [[], ['--features', 'uc-engine/lan-compat']]

// 从 clippy 的 JSON 行里取出仓库内的唯一违规位置；展开宏与重复编译产生的重复报告只算一次。
export function violationSites(jsonLines) {
  const sites = new Map()
  for (const line of jsonLines) {
    let message
    try {
      message = JSON.parse(line)
    } catch {
      continue
    }
    if (message.reason !== 'compiler-message') continue
    const diagnostic = message.message
    if (diagnostic?.code?.code !== LINT) continue
    const primary = diagnostic.spans?.find(span => span.is_primary)
    if (!primary || primary.file_name.startsWith('/')) continue
    sites.set(`${primary.file_name}:${primary.line_start}:${primary.column_start}`, {
      file: primary.file_name,
      line: primary.line_start,
    })
  }
  return sites
}

function runClippy(extraArgs) {
  const result = spawnSync('cargo', [...BASE_ARGS, ...extraArgs, '--', ...CLIPPY_LINT_ARGS], {
    cwd: REPOSITORY_ROOT,
    encoding: 'utf8',
    maxBuffer: 1024 * 1024 * 1024,
    env: { ...process.env, CLIPPY_CONF_DIR: CLIPPY_CONFIG_DIR },
  })
  if (result.status !== 0) {
    process.stderr.write(result.stderr.split('\n').slice(-40).join('\n'))
    throw new Error(`cargo clippy 失败（退出码 ${result.status}）：${extraArgs.join(' ') || '默认特性'}`)
  }
  return result.stdout.split('\n')
}

function main(argv) {
  const fromJson = argv.includes('--from-json') ? argv.slice(argv.indexOf('--from-json') + 1) : null
  const outputs = fromJson
    ? fromJson.map(path => readFileSync(path, 'utf8').split('\n'))
    : RUNS.map(runClippy)
  const sites = new Map()
  for (const output of outputs) for (const [key, site] of violationSites(output)) sites.set(key, site)
  if (sites.size === 0) {
    process.stdout.write('直接使用 tracing 日志宏检查通过：0 处\n')
    return 0
  }
  for (const { file, line } of sites.values()) {
    process.stderr.write(`ERROR ${file}:${line} 直接使用 tracing 日志宏；请改用 uc_*! 宏（docs/design-docs/decisions/030-typed-log-events-and-enforcement.md）\n`)
  }
  return 1
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try {
    process.exitCode = main(process.argv.slice(2))
  } catch (error) {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  }
}
