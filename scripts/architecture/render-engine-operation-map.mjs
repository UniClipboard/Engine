#!/usr/bin/env node
// 生成 Engine 操作的跨层调用链对照表。
//
// 从四个登记点静态提取同一操作在各层的名字：契约枚举、kind()、runtime 路由与
// 适配实现、应用入口类型与方法。输出 docs/generated/engine-operation-map.md。
// 只做导航，不复制业务规则；契约与行为以源码为准。
//
// 用法：
//   node scripts/architecture/render-engine-operation-map.mjs
//   node scripts/architecture/render-engine-operation-map.mjs --check    只校验生成物是否落后
//   node scripts/architecture/render-engine-operation-map.mjs --date=2026-11-04

import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const SCRIPT_PATH = fileURLToPath(import.meta.url)
const REPOSITORY_ROOT = resolve(dirname(SCRIPT_PATH), '../..')
const ENGINE_SRC = 'crates/uc-engine/src'
const OPERATION_FILE = `${ENGINE_SRC}/contract/operation.rs`
const DISPATCH_FILE = `${ENGINE_SRC}/runtime/dispatch.rs`
const APP_FACADE_FILE = 'crates/uc-application/src/facade/app_facade.rs'
const OUTPUT_FILE = 'docs/generated/engine-operation-map.md'

const options = { check: false, date: new Date().toISOString().slice(0, 10) }
for (const argument of process.argv.slice(2)) {
  if (argument === '--check') options.check = true
  else if (argument.startsWith('--date=')) options.date = argument.slice('--date='.length)
}

const read = (path) => readFileSync(join(REPOSITORY_ROOT, path), 'utf8')

// 从 `openIndex` 处的开括号找到配对的闭括号，跳过字符串字面量。
function matchingIndex(text, openIndex, open, close) {
  let depth = 0
  let quote = null
  let escaped = false
  for (let index = openIndex; index < text.length; index += 1) {
    const char = text[index]
    if (quote) {
      if (escaped) escaped = false
      else if (char === '\\') escaped = true
      else if (char === quote) quote = null
      continue
    }
    if (char === '"') quote = char
    else if (char === open) depth += 1
    else if (char === close) {
      depth -= 1
      if (depth === 0) return index
    }
  }
  return -1
}

function blockAt(text, openIndex) {
  const end = matchingIndex(text, openIndex, '{', '}')
  return end < 0 ? '' : text.slice(openIndex + 1, end)
}

// 契约枚举：变体名、载荷类型与 cfg 条件。
function parseOperationVariants(source) {
  const enumOpen = source.indexOf('pub enum Operation {')
  const body = blockAt(source, source.indexOf('{', enumOpen))
  const variants = []
  let cfg = null
  for (const raw of body.split('\n')) {
    const line = raw.trim()
    if (line.startsWith('#[cfg(')) {
      cfg = line
      continue
    }
    if (line.startsWith('//') || line === '') continue
    const match = /^([A-Z]\w*)(?:\(([^)]*)\))?(?:\s*\{|\s*,|$)/.exec(line)
    if (!match) continue
    variants.push({ name: match[1], payload: match[2] ?? null, cfg })
    cfg = null
  }
  return variants
}

function parseKindMap(source) {
  const map = new Map()
  for (const match of source.matchAll(
    /Self::(\w+)(?:\([^)]*\)|\s*\{\s*\.\.\s*\})?\s*=>\s*\{?\s*OperationKind::(\w+)/g,
  )) {
    map.set(match[1], match[2])
  }
  return map
}

// `match operation { ... }`：按缩进收集 `Operation::X => ...` 的分支文本。
function parseOperationArms(source) {
  const arms = new Map()
  let current = null
  let indent = 0
  const flush = () => {
    if (!current) return
    const previous = arms.get(current.name)
    // 前置特判分支没有 execute_ 调用；主匹配里的适配分支优先。
    for (const name of new Set([...current.text.matchAll(/Operation::([A-Z]\w*)/g)].map((m) => m[1]))) {
      const existing = arms.get(name)
      if (!existing || (!/execute_\w+/.test(existing) && /execute_\w+/.test(current.text))) {
        arms.set(name, current.text)
      }
    }
    current = null
  }
  for (const line of source.split('\n')) {
    const start = /^(\s+)Operation::([A-Z]\w*)/.exec(line)
    if (start) {
      flush()
      indent = start[1].length
      current = { name: start[2], text: line }
      continue
    }
    if (!current) continue
    current.text += `\n${line}`
    if (new RegExp(`^\\s{${indent}}\\}\\s*$`).test(line)) flush()
  }
  flush()
  return arms
}

function listFiles(directory) {
  const root = join(REPOSITORY_ROOT, directory)
  const found = []
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) found.push(...listFiles(path))
    else if (entry.name.endsWith('.rs')) found.push(path)
  }
  return found
}

// 适配实现索引：`execute_*` 函数或方法 → 文件、接收者类型、转调方法、结果变体。
// 适配实现分两处：`operations/`（转调应用 facade）与 `runtime/`（Engine 自持实现）。
function parseAdapterIndex() {
  const adapters = new Map()
  for (const path of listFiles(ENGINE_SRC)) {
    if (path.includes('/testing/') || path.includes('/tests/') || path.includes('/dev/')) continue
    const source = read(path)
    for (const match of source.matchAll(/\bfn (execute_\w+)\s*\(/g)) {
      const name = match[1]
      if (adapters.has(name)) continue
      const openParen = source.indexOf('(', match.index)
      const closeParen = matchingIndex(source, openParen, '(', ')')
      if (closeParen < 0) continue
      const parameters = source.slice(openParen + 1, closeParen)
      const bodyOpen = source.indexOf('{', closeParen)
      if (bodyOpen < 0) continue
      const body = blockAt(source, bodyOpen)
      const parameter = /^\s*(?:(?:mut )?(\w+)\s*:\s*&?(?:mut )?(?:dyn )?([\w:]+)|&?(?:mut )?self)/.exec(
        parameters,
      )
      const receiver = parameter?.[1] ?? 'self'
      const ownerType = parameter?.[2] ? parameter[2].split('::').pop() : 'ProductionRuntime'
      let call = null
      if (receiver !== 'self') {
        call = new RegExp(`${receiver}\\s*\\.\\s*(\\w+)\\s*\\(`).exec(body)?.[1] ?? null
      } else {
        const internal = [...body.matchAll(/self\s*\.\s*(\w+)\s*\(/g)]
          .map((entry) => entry[1])
          .filter((method) => !['current_facade', 'current_application', 'current_mobile_sync'].includes(method))
        call = internal[0] ?? null
      }
      adapters.set(name, {
        file: relative(REPOSITORY_ROOT, join(REPOSITORY_ROOT, path)),
        source,
        receiver,
        ownerType,
        call,
        body,
        results: [...new Set([...body.matchAll(/OperationResult::(\w+)/g)].map((entry) => entry[1]))],
      })
    }
  }
  return adapters
}

// 适配函数常把结果交给同文件的小助手函数返回，补一层同文件解析。
function helperResults(adapter) {
  if (adapter.results.length || !adapter.body) return adapter.results
  const helpers = new Map()
  for (const match of adapter.source.matchAll(/\n(?:pub(?:\(crate\))? )?(?:async )?fn (\w+)\s*\(/g)) {
    const openParen = adapter.source.indexOf('(', match.index)
    const closeParen = matchingIndex(adapter.source, openParen, '(', ')')
    const bodyOpen = adapter.source.indexOf('{', closeParen)
    if (bodyOpen < 0) continue
    const body = blockAt(adapter.source, bodyOpen)
    helpers.set(
      match[1],
      [...new Set([...body.matchAll(/OperationResult::(\w+)/g)].map((entry) => entry[1]))],
    )
  }
  const merged = new Set()
  for (const [name, results] of helpers) {
    if (adapter.body.includes(`${name}(`)) for (const result of results) merged.add(result)
  }
  return [...merged]
}

function parseAppFacade() {
  const source = read(APP_FACADE_FILE)
  const structOpen = source.indexOf('pub struct AppFacade {')
  const structBody = blockAt(source, source.indexOf('{', structOpen))
  const fields = new Map()
  for (const raw of structBody.split('\n')) {
    const match = /^\s*(\w+):\s*Arc<([\w:]+)>,/.exec(raw)
    if (match) fields.set(match[1], match[2].split('::').pop())
  }
  const methods = new Map()
  for (const chunk of source.split(/\n    pub (?:async )?fn /).slice(1)) {
    const name = /^(\w+)/.exec(chunk)?.[1]
    if (!name) continue
    const delegation = /self\.(\w+)\.(\w+)\s*\(/.exec(chunk)
    const field = delegation && fields.has(delegation[1]) ? delegation[1] : null
    methods.set(name, { type: field ? fields.get(field) : null })
  }
  return methods
}

const variants = parseOperationVariants(read(OPERATION_FILE))
const kinds = parseKindMap(read(OPERATION_FILE))
const dispatchArms = parseOperationArms(read(DISPATCH_FILE))
const adapters = parseAdapterIndex()
const facadeMethods = parseAppFacade()

// 逐操作解析：路由 → 适配实现；适配实现若是二级分派（如 LAN 兼容），继续下钻一层。
function resolveOperationRoute(variantName) {
  let armText = dispatchArms.get(variantName)
  for (let depth = 0; depth < 3 && armText; depth += 1) {
    const adapterName = /(execute_\w+)/.exec(armText)?.[1]
    if (!adapterName) break
    const adapter = adapters.get(adapterName)
    if (!adapter) return { adapterName, adapter: null, runtime: null }
    const nested = /match operation\s*\{/.test(adapter.body) ? parseOperationArms(adapter.body) : null
    const nestedArm = nested?.get(variantName)
    if (!nestedArm || nestedArm === armText) return { adapterName, adapter, runtime: null }
    armText = nestedArm
  }
  if (!armText) return { adapterName: null, adapter: null, runtime: null, hasArm: false }
  const runtimeCall = /self\s*\.\s*(\w+)(?:\s*\.\s*(\w+))?/.exec(armText)
  const accessors = ['current_facade', 'current_application', 'current_mobile_sync']
  if (runtimeCall && !accessors.includes(runtimeCall[1])) {
    return {
      adapterName: null,
      adapter: null,
      runtime: runtimeCall[2] ? `${runtimeCall[1]}.${runtimeCall[2]}` : runtimeCall[1],
    }
  }
  // 分支内联在 dispatch 里完成：报出最后一个被调用的方法名。
  const combinators = new Set(['map', 'map_err', 'as_ref', 'as_deref', 'clone', 'into', 'to_string', 'ok_or_else', 'unwrap_or_else', 'await'])
  const calls = [...armText.matchAll(/\.\s*(\w+)\s*\(/g)]
    .map((entry) => entry[1])
    .filter((name) => !combinators.has(name))
  return { adapterName: null, adapter: null, runtime: null, inline: calls.at(-1) ?? null, hasArm: true }
}

const rows = variants.map((variant) => {
  const resolved = resolveOperationRoute(variant.name)
  const adapter = resolved.adapter
  const call = adapter?.call ?? null
  let owner = adapter?.ownerType ?? null
  if (owner === 'AppFacade' && call) owner = facadeMethods.get(call)?.type ?? 'AppFacade'
  const adapterResults = adapter ? helperResults(adapter) : []
  const results = adapterResults.length
    ? adapterResults
    : [...new Set([...(dispatchArms.get(variant.name) ?? '').matchAll(/OperationResult::(\w+)/g)].map((m) => m[1]))]
  return {
    ...variant,
    kind: kinds.get(variant.name) ?? null,
    adapter: resolved.adapterName,
    adapterFile: adapter?.file ?? null,
    runtime: resolved.runtime,
    inline: resolved.inline ?? null,
    hasArm: resolved.hasArm ?? true,
    call,
    owner,
    results,
  }
})

// 领域按适配实现所在目录归类；没有适配实现的归入 runtime 自持。
const domainOf = (row) => {
  if (!row.adapterFile) return 'runtime 自持'
  const operations = row.adapterFile.split('/operations/')[1]
  if (operations) return operations.split('/')[0]
  const compatibility = row.adapterFile.split('/compatibility/')[1]
  if (compatibility) return `compatibility/${compatibility.split('/')[0]}`
  return 'runtime 自持'
}

const buckets = new Map()
for (const row of rows) {
  const domain = domainOf(row)
  if (!buckets.has(domain)) buckets.set(domain, [])
  buckets.get(domain).push(row)
}

const missingKind = rows.filter((row) => !row.kind)
const unrouted = rows.filter((row) => !row.hasArm)
const noCall = rows.filter((row) => row.adapter && !row.call)

const code = (value) => (value ? `\`${value}\`` : '—')

// 目录簇里的 `mod.rs` 只显示文件名会重名，显示最后两段。
const shortPath = (path) => {
  if (!path) return '?'
  const parts = path.split('/')
  return parts.at(-1) === 'mod.rs' ? parts.slice(-2).join('/') : parts.at(-1)
}

const lines = []
lines.push('# Engine 操作跨层调用链对照')
lines.push('')
lines.push(`> 生成日期：${options.date}`)
lines.push('>')
lines.push(
  `> 权威来源：\`${OPERATION_FILE}\`、\`${DISPATCH_FILE}\`、\`${ENGINE_SRC}/operations/\`、\`${APP_FACADE_FILE}\`。`,
)
lines.push('> 本文件是可再生导航快照；操作语义、错误码与结果字段以源码和')
lines.push('> [uc-engine 跨平台核心接口](../design-docs/uc-engine-interface.md) 为准。')
lines.push('>')
lines.push('> 重新生成：`node scripts/architecture/render-engine-operation-map.mjs`')
lines.push('')
lines.push(
  `当前共 **${rows.length}** 个操作变体，其中 \`dev-tools\` 专属 ${rows.filter((row) => row.cfg).length} 个。`,
)
lines.push('')
lines.push('## 怎么用')
lines.push('')
lines.push('1. 在“契约操作”列找到操作名，它就是 `Operation` 枚举的变体。')
lines.push('2. “适配函数”列是处理该操作的地方，括号里是文件名。')
lines.push('3. “应用入口”列给出接收者类型与方法；`AppFacade` 之外的接收者说明该操作不经过应用聚合 facade。')
lines.push('')
lines.push('层职责一句话：`contract` 定义名字；`runtime/dispatch.rs` 决定走哪条路；')
lines.push('`operations/` 在契约与应用之间翻译类型和错误；`AppFacade` 是应用层唯一对外类型；')
lines.push('子 facade 与 use case 拥有业务顺序；`uc-core` 的 port 拥有规则；`uc-infra-*` 提供能力。')
lines.push('')
lines.push('“runtime 自持”表示该操作不经过应用 facade，由 Engine 运行期直接实现（含系统剪贴板快照构造）。')

for (const [domain, bucket] of buckets) {
  lines.push('')
  lines.push(`## 领域：${domain}（${bucket.length}）`)
  lines.push('')
  lines.push('| 契约操作 | kind | 适配函数 | 应用入口 | 结果变体 |')
  lines.push('| --- | --- | --- | --- | --- |')
  for (const row of bucket) {
    const adapter = row.adapter
      ? `${code(row.adapter)}<br>\`${shortPath(row.adapterFile)}\``
      : '—'
    const entry = row.call
      ? `${code(row.call)}${row.owner ? `<br>${code(row.owner)}` : ''}`
      : row.adapter && row.owner
        ? `runtime 内部<br>${code(row.owner)}`
        : row.runtime
        ? `runtime: ${code(row.runtime)}`
        : row.inline
          ? `dispatch: ${code(row.inline)}`
          : row.hasArm
            ? 'dispatch 内联'
            : '—'
    const results = row.results.length ? row.results.map(code).join(', ') : '—'
    const payload = row.payload ? `(${row.payload})` : row.cfg ? ' *(dev-tools)*' : ''
    lines.push(
      `| ${code(row.name)}${payload} | ${code(row.kind)} | ${adapter} | ${entry} | ${results} |`,
    )
  }
}

lines.push('')
lines.push('## 生成时发现的缺口')
lines.push('')
if (missingKind.length) {
  lines.push(
    `- ${missingKind.length} 个变体在 \`kind()\` 中没有映射：${missingKind.map((row) => code(row.name)).join(', ')}`,
  )
} else {
  lines.push('- 全部变体都出现在 `Operation::kind()` 中。')
}
if (unrouted.length) {
  lines.push(
    `- ${unrouted.length} 个变体没有路由：${unrouted.map((row) => code(row.name)).join(', ')}`,
  )
} else {
  lines.push('- 全部变体都有路由分支。')
}
if (noCall.length) {
  lines.push(
    `- ${noCall.length} 个适配函数没有可见的转调方法（按调用名识别，可能漏判）：${noCall.map((row) => code(row.adapter)).join(', ')}`,
  )
} else {
  lines.push('- 全部适配函数都有可见的转调方法。')
}
lines.push('')
lines.push('## 登记点')
lines.push('')
lines.push('新增一个操作必须同时改动下列位置，漏改通常只有集成测试能发现：')
lines.push('')
lines.push('1. `crates/uc-engine/src/contract/operation.rs`：枚举变体与 `kind()`')
lines.push('2. `crates/uc-engine/src/contract/result.rs`：结果变体')
lines.push('3. `crates/uc-engine/src/operations/<领域>/`：适配函数')
lines.push('4. `crates/uc-engine/src/runtime/dispatch.rs`：路由分支')
lines.push('5. `bindings/uc-engine-uniffi/src/runtime.rs`：Swift / Kotlin 入口')
lines.push('6. `bindings/uc-ohos-napi/src/runtime.rs`：HarmonyOS 入口')
lines.push('')
const markdown = lines.join('\n')

if (options.check) {
  let current = ''
  try {
    current = read(OUTPUT_FILE)
  } catch {
    current = ''
  }
  if (current !== markdown) {
    console.error(`${OUTPUT_FILE} 落后于源码，请重新生成`)
    process.exit(1)
  }
  console.log(`${OUTPUT_FILE} 与源码一致`)
} else {
  writeFileSync(join(REPOSITORY_ROOT, OUTPUT_FILE), markdown)
  console.log(`已生成 ${OUTPUT_FILE}：${rows.length} 个操作，${buckets.size} 个领域`)
}
