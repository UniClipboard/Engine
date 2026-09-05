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
  'uc_otlp',
])
const RETIRED_TARGETS = new Set([
  'admission.performance',
  'membership.performance',
  'storage.performance',
  'uc_otlp',
])
const REMOTE_ALLOWED_PREFIXES = [
  'crates/uc-observability-contract/src/diagnostics/',
]
const HEALTH_ALLOWED_PATHS = new Set([
  'crates/uc-observability-contract/src/diagnostics/mod.rs',
  'crates/uc-observability-runtime/src/remote_health.rs',
])
const REMOTE_ALLOWED_FIELDS = new Set([
  'target',
  'name',
  'parent',
  'event.name',
  'uc.flow.id',
  'uc.domain',
  'uc.operation',
  'uc.role',
  'uc.outcome',
  'error.type',
  'duration_ms',
  'otel.kind',
  'otel.name',
  'otel.status_code',
  'task.completed.count',
  'task.timed_out.count',
  'task.join_error.count',
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
      } else if (
        name.endsWith('.rs') &&
        name !== 'tests.rs' &&
        !name.endsWith('_tests.rs') &&
        name !== 'test_support.rs'
      ) {
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
  const chars = source.split('')
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

function maskStrings(source) {
  const chars = source.split('')
  let state = 'code'
  let rawHashes = ''
  for (let index = 0; index < chars.length; index += 1) {
    const char = chars[index]
    if (state === 'string') {
      if (char === '\\') {
        chars[index] = ' '
        if (chars[index + 1] !== '\n') chars[index + 1] = ' '
        index += 1
      } else {
        if (char === '"') state = 'code'
        if (char !== '\n') chars[index] = ' '
      }
      continue
    }
    if (state === 'raw') {
      const closes = char === '"' && source.startsWith(rawHashes, index + 1)
      if (char !== '\n') chars[index] = ' '
      if (closes) {
        for (let offset = 0; offset < rawHashes.length; offset += 1) chars[index + 1 + offset] = ' '
        index += rawHashes.length
        state = 'code'
      }
      continue
    }
    if (char === '"') {
      chars[index] = ' '
      state = 'string'
    } else if (char === 'r' || (char === 'b' && source[index + 1] === 'r')) {
      const raw = source.slice(index).match(/^b?r(#{0,16})"/)
      if (raw) {
        rawHashes = raw[1]
        for (let offset = 0; offset < raw[0].length; offset += 1) chars[index + offset] = ' '
        index += raw[0].length - 1
        state = 'raw'
      }
    }
  }
  return chars.join('')
}

function productionSource(source) {
  const masked = maskComments(source)
  const structural = maskStrings(masked)
  const chars = source.split('')
  const cfgTestModule = /#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+[a-zA-Z_]\w*\s*\{/g
  for (const match of masked.matchAll(cfgTestModule)) {
    const open = masked.indexOf('{', match.index)
    const end = balancedEnd(structural, open, '{', '}')
    for (let index = match.index; index < end; index += 1) {
      if (chars[index] !== '\n') chars[index] = ' '
    }
  }
  return chars.join('')
}

function tracingBlocks(source) {
  const production = productionSource(source)
  const masked = maskComments(production)
  const structural = maskStrings(masked)
  const blocks = []
  const macro = /(?:tracing::)?(trace|debug|info|warn|error|event|span|trace_span|debug_span|info_span|warn_span|error_span)!\s*([({[])/g
  for (const match of masked.matchAll(macro)) {
    const open = match.index + match[0].lastIndexOf(match[2])
    const close = { '(': ')', '{': '}', '[': ']' }[match[2]]
    const end = balancedEnd(structural, open, match[2], close)
    blocks.push({ index: match.index, kind: match[1], text: production.slice(open, end) })
  }
  const instrument = /#\s*\[\s*(?:tracing::)?instrument\b/g
  for (const match of masked.matchAll(instrument)) {
    const open = masked.indexOf('[', match.index)
    const end = balancedEnd(structural, open, '[', ']')
    blocks.push({ index: match.index, kind: 'instrument', text: production.slice(open, end) })
  }
  const record = /\.record\s*\(/g
  for (const match of masked.matchAll(record)) {
    const open = masked.indexOf('(', match.index)
    const end = balancedEnd(structural, open, '(', ')')
    blocks.push({ index: match.index, kind: 'record', text: production.slice(open, end) })
  }
  return blocks
}

function lineNumber(source, index) {
  return source.slice(0, index).split('\n').length
}

function targetOf(block) {
  const target = block.match(/\btarget\s*:\s*(?:"([^"]+)"|(TELEMETRY_TARGET|HEALTH_TARGET))/)
  if (target?.[1]) return target[1]
  if (target?.[2] === 'TELEMETRY_TARGET') return 'uc.telemetry'
  if (target?.[2] === 'HEALTH_TARGET') return 'observability.health'
  return '<module>'
}

function fieldNames(block) {
  const fields = new Set()
  for (const segment of topLevelSegments(block)) {
    const assignment = segment.match(/^([a-zA-Z_]\w*(?:\.[a-zA-Z_]\w*)*)\s*=/)
    if (assignment) {
      fields.add(assignment[1])
      continue
    }
    const shorthand = segment.match(/^[%?]\s*([a-zA-Z_]\w*)$/)
    if (shorthand) {
      fields.add(shorthand[1])
      continue
    }
    if (/^[a-zA-Z_]\w*(?:\.[a-zA-Z_]\w*)*$/.test(segment)) fields.add(segment)
  }
  return [...fields]
}

function topLevelSegments(block) {
  const structural = maskStrings(maskComments(block))
  const pairs = { '(': ')', '{': '}', '[': ']' }
  const stack = [pairs[structural[0]]]
  const segments = []
  let start = 1
  for (let index = 1; index < structural.length - 1; index += 1) {
    const char = structural[index]
    if (pairs[char]) stack.push(pairs[char])
    else if (char === stack.at(-1)) stack.pop()
    else if (char === ',' && stack.length === 1) {
      segments.push(block.slice(start, index).trim())
      start = index + 1
    }
  }
  const tail = block.slice(start, -1).trim()
  if (tail) segments.push(tail)
  return segments
}

function hasMessageBody(block) {
  if (!['trace', 'debug', 'info', 'warn', 'error', 'event'].includes(block.kind)) return false
  return topLevelSegments(block.text).some(segment => /^(?:b?"|b?r#*")/.test(segment) || /^message\s*=/.test(segment))
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
  if (hasMessageBody(block)) flags.push('message-body')
  return flags
}

function categoryFor(path, block) {
  const target = targetOf(block.text)
  if (target === 'uc.telemetry') return 'stable remote'
  if (target === 'observability.health') {
    return 'local operational'
  }
  if (RETIRED_TARGETS.has(target) || /uc-observability-contract\/src\/(?:otlp|stages)\.rs$/.test(path)) return 'delete'
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
    if (RETIRED_TARGETS.has(entry.target)) {
      problems.push(`${callsite}: retired observability target must not return: ${entry.target}`)
    }
    if (entry.flags.includes('sensitive-field')) {
      problems.push(`${callsite}: audited output contains a sensitive field`)
    }
    if (entry.flags.includes('raw-error')) {
      problems.push(`${callsite}: audited output contains raw error text`)
    }
    if (
      ['uc.telemetry', 'observability.health'].includes(entry.target) &&
      entry.flags.includes('message-body')
    ) {
      problems.push(`${callsite}: audited output must not contain a log body`)
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
    if (entry.target === 'observability.health') {
      if (!HEALTH_ALLOWED_PATHS.has(entry.path)) {
        problems.push(`${callsite}: health telemetry must be emitted by a fixed runtime owner`)
      }
      for (const field of entry.fields) {
        if (!['target', 'event.name', 'task.kind', 'error.type', 'task.completed.count', 'task.timed_out.count', 'task.join_error.count'].includes(field)) {
          problems.push(`${callsite}: health field is not allowlisted: ${field}`)
        }
      }
    }
  }

  const filterPath = 'crates/uc-observability-runtime/src/filter.rs'
  const runtimeFilter = readFileSync(join(ROOT, filterPath), 'utf8')
  for (const marker of ['remote_span_enabled', 'remote_log_enabled', 'health_log_enabled', 'local_sink_enabled']) {
    if (!runtimeFilter.includes(marker)) {
      problems.push(`${filterPath}: process runtime lacks ${marker}`)
    }
  }
  return problems
}

function inventoryMarkdown(entries) {
  const categories = ['stable remote', 'local operational', 'local debug', 'product analytics', 'delete']
  const lines = [
    '# 运行期观测清单',
    '',
    '> 由 `scripts/architecture/check-observability-privacy.mjs --write-inventory` 从生产 Rust 源生成。',
    '> 该文件只描述最后一次运行生成命令时的代码事实，不是新增埋点的授权清单。',
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
    // 中文🙂不得改变后续源码位置
    // tracing::info!(target: "uc.telemetry", path = %path, "comment");
    #[cfg(test)] mod tests { fn ignored() { tracing::info!(target: "uc.telemetry", %device_id); } }
    fn bad() {
      tracing::debug!(%transfer_id, "debug");
      tracing::event!{target: "uc.telemetry", tracing::Level::INFO, uc.operation = "pair", path = %path};
      tracing::info![target: "uc.telemetry", uc.operation = "pair", "private ) body"];
      tracing::warn!(target: "observability.health", event.name = "bad", secret,);
      tracing::event!(target: TELEMETRY_TARGET, tracing::Level::INFO, path = %path);
      tracing::event!(target: HEALTH_TARGET, tracing::Level::WARN, secret,);
      tracing::info_span!(target: "uc.telemetry", "pair", error = %error);
      tracing::warn!("failed: {e}");
    }
    #[tracing::instrument(err)] async fn instrumented() {}
  `
  const blocks = tracingBlocks(source)
  const remote = blocks.filter(block => targetOf(block.text) === 'uc.telemetry')
  const flags = blocks.flatMap(flagsFor)
  const health = blocks.find(block => targetOf(block.text) === 'observability.health')
  const constantRemote = blocks.find(block => block.text.includes('TELEMETRY_TARGET'))
  const constantHealth = blocks.find(block => block.text.includes('HEALTH_TARGET'))
  const retiredProblems = strictProblems([{
    category: 'delete',
    target: 'admission.performance',
    path: 'crates/uc-engine/src/retired.rs',
    line: 1,
    kind: 'event',
    flags: [],
    fields: [],
  }])
  if (
    blocks.length !== 9 ||
    remote.length !== 4 ||
    !flags.includes('sensitive-field') ||
    !flags.includes('raw-error') ||
    !flags.includes('message-body') ||
    !health ||
    !fieldNames(health.text).includes('secret') ||
    targetOf(constantRemote?.text ?? '') !== 'uc.telemetry' ||
    targetOf(constantHealth?.text ?? '') !== 'observability.health' ||
    !retiredProblems.some(problem => problem.includes('retired observability target'))
  ) {
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
