#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(fileURLToPath(new URL('../..', import.meta.url)))
const INVENTORY_PATH = join(ROOT, 'docs/generated/observability-inventory.md')
const SOURCE_ROOTS = [
  'crates/uc-core/src',
  'crates/uc-application/src',
  'crates/uc-infra/src',
  'crates/uc-engine/src',
  'crates/uc-observability-contract/src',
  'crates/uc-observability-runtime/src',
  'bindings/uc-engine-uniffi/src',
  'bindings/uc-ohos-napi/src',
]

const AUDITED_TARGETS = new Set([
  'uc.telemetry',
  'observability.health',
  'admission.performance',
  'membership.performance',
  'storage.performance',
])
const REMOTE_ALLOWED_PREFIXES = [
  'crates/uc-observability-contract/src/diagnostics/',
  'crates/uc-engine/src/assembly/observability/',
]
const REMOTE_ALLOWED_FIELDS = new Set([
  'target',
  'name',
  'parent',
  'message',
  'event.name',
  'uc.flow.id',
  'uc.domain',
  'uc.operation',
  'uc.role',
  'uc.message.kind',
  'uc.outcome',
  'error.type',
  'duration_ms',
  'item.count',
  'peer.count',
  'otel.kind',
])
const SENSITIVE_FIELD = /^(?:.*\.)?(?:device(?:_id)?|from_device|peer(?:_id)?|target_device_id|address|public_addr|selected_ip|relay_url|path|target_path|file(?:name)?|space_id|profile(?:_id)?|member(?:_id)?|entry_id|event_id|attempt_id|transfer_id|snapshot_hash|code_hash|invitation(?:_id)?|token|password|secret|payload|digest|hash)$/i
const ERROR_FIELD = /^(?:error|err|source)$/i
const ERROR_INTERPOLATION = /\{(?:error|err|source|e)(?::[^}]*)?\}/i

function rustFiles(root) {
  const absoluteRoot = resolve(ROOT, root)
  if (!existsSync(absoluteRoot)) return []
  const files = []
  const visit = path => {
    for (const name of readdirSync(path)) {
      const child = join(path, name)
      const stat = statSync(child)
      if (stat.isDirectory()) {
        if (!['tests', 'test_support', 'testing'].includes(name)) visit(child)
      } else if (name.endsWith('.rs') && !name.endsWith('_tests.rs') && name !== 'test_support.rs') {
        files.push(child)
      }
    }
  }
  visit(absoluteRoot)
  return files
}

// 注释必须在扫描前移除，否则文档中的坏样例会被当作生产埋点。字符串与
// 换行原位保留，使 target、格式占位符和行号仍可准确检查。
function maskComments(source) {
  const chars = [...source]
  let state = 'code'
  let blockDepth = 0
  let rawHashes = ''
  for (let index = 0; index < chars.length; index += 1) {
    const char = chars[index]
    const next = chars[index + 1]
    if (state === 'line') {
      if (char === '\n') state = 'code'
      else chars[index] = ' '
      continue
    }
    if (state === 'block') {
      if (char === '/' && next === '*') {
        chars[index] = chars[index + 1] = ' '
        blockDepth += 1
        index += 1
      } else if (char === '*' && next === '/') {
        chars[index] = chars[index + 1] = ' '
        blockDepth -= 1
        index += 1
        if (blockDepth === 0) state = 'code'
      } else if (char !== '\n') chars[index] = ' '
      continue
    }
    if (state === 'string') {
      if (char === '\\') index += 1
      else if (char === '"') state = 'code'
      continue
    }
    if (state === 'raw') {
      if (char === '"' && source.startsWith(rawHashes, index + 1)) {
        index += rawHashes.length
        state = 'code'
      }
      continue
    }
    if (char === '/' && next === '/') {
      chars[index] = chars[index + 1] = ' '
      state = 'line'
      index += 1
    } else if (char === '/' && next === '*') {
      chars[index] = chars[index + 1] = ' '
      state = 'block'
      blockDepth = 1
      index += 1
    } else if (char === '"') {
      state = 'string'
    } else if (char === 'r') {
      const raw = source.slice(index).match(/^r(#{0,16})"/)
      if (raw) {
        rawHashes = raw[1]
        index += raw[0].length - 1
        state = 'raw'
      }
    }
  }
  return chars.join('')
}

function balancedEnd(source, start, open, close) {
  let depth = 0
  for (let index = start; index < source.length; index += 1) {
    if (source[index] === open) depth += 1
    else if (source[index] === close) {
      depth -= 1
      if (depth === 0) return index + 1
    }
  }
  return source.length
}

function productionSource(source) {
  const masked = maskComments(source)
  const chars = [...source]
  const cfgTestModule = /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+[a-zA-Z_]\w*\s*\{/g
  for (const match of masked.matchAll(cfgTestModule)) {
    const open = masked.indexOf('{', match.index)
    const end = balancedEnd(masked, open, '{', '}')
    for (let index = match.index; index < end; index += 1) {
      if (chars[index] !== '\n') chars[index] = ' '
    }
  }
  return chars.join('')
}

function tracingBlocks(source) {
  const production = productionSource(source)
  const masked = maskComments(production)
  const blocks = []
  const macro = /(?:tracing::)?(trace|debug|info|warn|error|event|span|trace_span|debug_span|info_span|warn_span|error_span)!\s*\(/g
  for (const match of masked.matchAll(macro)) {
    const open = masked.indexOf('(', match.index)
    const end = balancedEnd(masked, open, '(', ')')
    blocks.push({ index: match.index, kind: match[1], text: production.slice(open, end) })
  }
  const instrument = /#\s*\[\s*(?:tracing::)?instrument\b/g
  for (const match of masked.matchAll(instrument)) {
    const open = masked.indexOf('[', match.index)
    const end = balancedEnd(masked, open, '[', ']')
    blocks.push({ index: match.index, kind: 'instrument', text: production.slice(open, end) })
  }
  const record = /\.record\s*\(/g
  for (const match of masked.matchAll(record)) {
    const open = masked.indexOf('(', match.index)
    const end = balancedEnd(masked, open, '(', ')')
    blocks.push({ index: match.index, kind: 'record', text: production.slice(open, end) })
  }
  return blocks
}

function lineNumber(source, index) {
  return source.slice(0, index).split('\n').length
}

function targetOf(block) {
  return block.match(/\btarget\s*:\s*"([^"]+)"/)?.[1] ?? '<module>'
}

function fieldNames(block) {
  const fields = new Set()
  for (const match of block.matchAll(/(?:^|[,({\s])([a-zA-Z_]\w*(?:\.[a-zA-Z_]\w*)*)\s*=/g)) {
    fields.add(match[1])
  }
  for (const match of block.matchAll(/(?:^|[,({\s])[%?]\s*([a-zA-Z_]\w*)/g)) {
    fields.add(match[1])
  }
  return [...fields]
}

function flagsFor(block) {
  const flags = []
  const fields = fieldNames(block.text)
  if (fields.some(field => SENSITIVE_FIELD.test(field))) flags.push('sensitive-field')
  if (
    fields.some(field => ERROR_FIELD.test(field)) ||
    ERROR_INTERPOLATION.test(block.text) ||
    (block.kind === 'instrument' && /\berr\b/.test(block.text))
  ) flags.push('raw-error')
  if (block.kind === 'instrument' && !/\bskip_all\b/.test(block.text)) flags.push('implicit-arguments')
  return flags
}

function categoryFor(path, block) {
  const target = targetOf(block.text)
  if (target === 'uc.telemetry') return 'stable remote'
  if (['admission.performance', 'membership.performance', 'storage.performance', 'observability.health'].includes(target)) {
    return 'local operational'
  }
  if (target === 'uc_otlp' || /uc-observability-contract\/src\/(?:otlp|stages)\.rs$/.test(path)) return 'delete'
  if (path.includes('/analytics/')) return 'product analytics'
  return 'local debug'
}

function inventoryEntries() {
  const entries = []
  for (const root of SOURCE_ROOTS) {
    for (const file of rustFiles(root)) {
      const source = readFileSync(file, 'utf8')
      const path = relative(ROOT, file)
      for (const block of tracingBlocks(source)) {
        entries.push({
          category: categoryFor(path, block),
          target: targetOf(block.text),
          path,
          line: lineNumber(source, block.index),
          kind: block.kind,
          flags: flagsFor(block),
          fields: fieldNames(block.text),
        })
      }
    }
  }
  return entries.sort((left, right) =>
    left.category.localeCompare(right.category) ||
    left.path.localeCompare(right.path) ||
    left.line - right.line
  )
}

function strictProblems(entries) {
  const problems = []
  for (const entry of entries.filter(item => AUDITED_TARGETS.has(item.target))) {
    const callsite = `${entry.path}:${entry.line}`
    if (entry.flags.includes('sensitive-field')) {
      problems.push(`${callsite}: audited output contains a sensitive field`)
    }
    if (entry.flags.includes('raw-error')) {
      problems.push(`${callsite}: audited output contains raw error text`)
    }
    if (entry.target === 'uc.telemetry') {
      if (!REMOTE_ALLOWED_PREFIXES.some(prefix => entry.path.startsWith(prefix))) {
        problems.push(`${callsite}: remote telemetry must be emitted by the diagnostics owner`)
      }
      for (const field of entry.fields) {
        if (!REMOTE_ALLOWED_FIELDS.has(field)) {
          problems.push(`${callsite}: remote field is not allowlisted: ${field}`)
        }
      }
    }
  }

  for (const path of ['bindings/uc-engine-uniffi/src/apple.rs', 'bindings/uc-engine-uniffi/src/android.rs']) {
    const source = readFileSync(join(ROOT, path), 'utf8')
    if (!source.includes('persistent_sink_enabled')) {
      problems.push(`${path}: platform system sink lacks the audited target filter`)
    }
  }
  const fileLog = readFileSync(join(ROOT, 'bindings/uc-engine-uniffi/src/file_log.rs'), 'utf8')
  if (!fileLog.includes('.with_filter(filter_fn(persistent_sink_enabled))')) {
    problems.push('bindings/uc-engine-uniffi/src/file_log.rs: persistent file sink lacks the audited target filter')
  }
  return problems
}

function inventoryMarkdown(entries) {
  const categories = ['stable remote', 'local operational', 'local debug', 'product analytics', 'delete']
  const lines = [
    '# 运行期观测清单',
    '',
    '> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。',
    '> 生成日期：2026-09-04。该文件只描述生成时的代码事实，不是新增埋点的授权清单。',
    '',
  ]
  for (const category of categories) {
    const group = entries.filter(entry => entry.category === category)
    lines.push(`## ${category}`, '', `共 ${group.length} 个调用点。`, '')
    lines.push('| Target | 调用点 | 类型 | 风险标记 |', '| --- | --- | --- | --- |')
    for (const entry of group) {
      const flags = entry.flags.length > 0 ? entry.flags.join(', ') : '-'
      lines.push(`| \`${entry.target}\` | \`${entry.path}:${entry.line}\` | \`${entry.kind}\` | ${flags} |`)
    }
    lines.push('')
  }
  lines.push(
    '## 输出边界',
    '',
    '- 系统日志、本地文件和远程发送默认拒绝 `local debug` 与 `delete`。',
    '- `stable remote` 只能由诊断契约和 Engine 完整能力装饰器产生，并接受固定字段检查。',
    '- `product analytics` 使用独立合同，不共享诊断身份、流程号或发送路径。',
    '- 风险标记用于安排后续清理；被默认拒绝的历史调用点不等于允许输出。',
    ''
  )
  return lines.join('\n')
}

function selfTest() {
  const source = `
    // tracing::info!(target: "uc.telemetry", path = %path, "comment");
    #[cfg(test)] mod tests { fn ignored() { tracing::info!(target: "uc.telemetry", %device_id); } }
    fn bad() {
      tracing::debug!(%transfer_id, "debug");
      tracing::event!(target: "uc.telemetry", tracing::Level::INFO, uc.operation = "pair", path = %path);
      tracing::info_span!(target: "uc.telemetry", "pair", error = %error);
      tracing::warn!("failed: {e}");
    }
    #[tracing::instrument(err)] async fn instrumented() {}
  `
  const blocks = tracingBlocks(source)
  const remote = blocks.filter(block => targetOf(block.text) === 'uc.telemetry')
  const flags = blocks.flatMap(flagsFor)
  if (blocks.length !== 5 || remote.length !== 2 || !flags.includes('sensitive-field') || !flags.includes('raw-error')) {
    throw new Error(`privacy checker self-test failed: blocks=${blocks.length} remote=${remote.length} flags=${flags.join(',')}`)
  }
  process.stdout.write('Observability privacy checker self-test passed\n')
}

if (process.argv.includes('--self-test')) {
  selfTest()
} else {
  const entries = inventoryEntries()
  if (process.argv.includes('--write-inventory')) {
    writeFileSync(INVENTORY_PATH, inventoryMarkdown(entries))
    process.stdout.write(`Wrote ${relative(ROOT, INVENTORY_PATH)} with ${entries.length} callsites\n`)
  }
  const problems = strictProblems(entries)
  if (problems.length > 0) {
    process.stderr.write(`${problems.join('\n')}\n`)
    process.exit(1)
  }
  if (!process.argv.includes('--write-inventory')) {
    process.stdout.write(`Observability privacy check passed (${entries.length} inventoried callsites)\n`)
  }
}
