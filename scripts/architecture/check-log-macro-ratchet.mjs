#!/usr/bin/env node
// 直接使用 tracing 日志宏的棘轮检查（ADR-030）。
//
// 用 clippy 的 disallowed_macros 统计 `tracing::{trace,debug,info,warn,error}!` 的直接使用，
// 与 log-macro-baseline.json 按文件比较：任何文件的数量都不得增加；数量下降时基线必须同步下调，
// 使已迁移到 `uc_*!` 宏的文件不会退回。配置只通过 CLIPPY_CONF_DIR 启用，不影响日常 cargo clippy。
//
// 用法：
//   node scripts/architecture/check-log-macro-ratchet.mjs                 检查
//   node scripts/architecture/check-log-macro-ratchet.mjs --update        下调基线（数量增加时拒绝）
//   node scripts/architecture/check-log-macro-ratchet.mjs --update --allow-increase   显式接受增加
//   node scripts/architecture/check-log-macro-ratchet.mjs --from-json <文件>...      读取已有的 clippy JSON

import { spawnSync } from 'node:child_process'
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const SCRIPT_PATH = fileURLToPath(import.meta.url)
const REPOSITORY_ROOT = resolve(dirname(SCRIPT_PATH), '../..')
const BASELINE_PATH = resolve(REPOSITORY_ROOT, 'scripts/architecture/log-macro-baseline.json')
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

export function countByFile(sites) {
  const counts = {}
  for (const { file } of sites.values()) counts[file] = (counts[file] ?? 0) + 1
  return Object.fromEntries(Object.entries(counts).sort(([left], [right]) => left.localeCompare(right)))
}

export function compareToBaseline(baseline, current) {
  const increases = []
  const decreases = []
  for (const file of new Set([...Object.keys(baseline), ...Object.keys(current)])) {
    const before = baseline[file] ?? 0
    const after = current[file] ?? 0
    if (after > before) increases.push({ file, before, after })
    else if (after < before) decreases.push({ file, before, after })
  }
  return { increases, decreases }
}

export function nextBaseline(baseline, current, { allowIncrease = false } = {}) {
  const { increases } = compareToBaseline(baseline, current)
  if (increases.length > 0 && !allowIncrease) {
    throw new Error(`拒绝上调基线：${increases.map(item => item.file).join(', ')}；确需接受请加 --allow-increase`)
  }
  return { version: 1, total: Object.values(current).reduce((sum, count) => sum + count, 0), files: current }
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

function readBaseline() {
  return JSON.parse(readFileSync(BASELINE_PATH, 'utf8')).files
}

function main(argv) {
  const fromJson = argv.includes('--from-json') ? argv.slice(argv.indexOf('--from-json') + 1) : null
  const outputs = fromJson
    ? fromJson.map(path => readFileSync(path, 'utf8').split('\n'))
    : RUNS.map(runClippy)
  const sites = new Map()
  for (const output of outputs) for (const [key, site] of violationSites(output)) sites.set(key, site)
  const current = countByFile(sites)

  if (argv.includes('--update')) {
    let baseline = {}
    try {
      baseline = readBaseline()
    } catch {
      // 首次生成基线。
    }
    const next = nextBaseline(baseline, current, { allowIncrease: argv.includes('--allow-increase') || Object.keys(baseline).length === 0 })
    writeFileSync(BASELINE_PATH, `${JSON.stringify(next, null, 2)}\n`)
    process.stdout.write(`基线已更新：共 ${next.total} 处，${Object.keys(current).length} 个文件\n`)
    return 0
  }

  const { increases, decreases } = compareToBaseline(readBaseline(), current)
  for (const { file, before, after } of increases) {
    process.stderr.write(`ERROR ${file} 直接使用 tracing 日志宏 ${before} → ${after} 处；请改用 uc_*! 宏（docs/design-docs/decisions/030-typed-log-events-and-enforcement.md）\n`)
    for (const site of sites.values()) if (site.file === file) process.stderr.write(`  ${site.file}:${site.line}\n`)
  }
  for (const { file, before, after } of decreases) {
    process.stderr.write(`ERROR ${file} 已从 ${before} 降到 ${after} 处，基线需要同步下调：node scripts/architecture/check-log-macro-ratchet.mjs --update\n`)
  }
  if (increases.length > 0 || decreases.length > 0) return 1
  process.stdout.write(`日志宏棘轮检查通过：当前 ${Object.values(current).reduce((sum, count) => sum + count, 0)} 处，均在基线内\n`)
  return 0
}

if (process.argv[1] && resolve(process.argv[1]) === SCRIPT_PATH) {
  try {
    process.exitCode = main(process.argv.slice(2))
  } catch (error) {
    process.stderr.write(`${error.message}\n`)
    process.exitCode = 1
  }
}
