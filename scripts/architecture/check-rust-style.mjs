#!/usr/bin/env node

import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const SCRIPT_PATH = fileURLToPath(import.meta.url)
const SCRIPT_DIR = dirname(SCRIPT_PATH)
const REPOSITORY_ROOT = resolve(SCRIPT_DIR, '../..')
const SOURCE_ROOTS = ['crates', 'bindings', 'compatibility', 'tests']
const ALLOW_MARKER = /\/\/\s*rust-style:\s*allow-qualified-path\s*--\s*\S.+$/
// 错误变量名：扫描规则与执行计划中的清单保持一致。
const ERROR_VARIABLE = String.raw`(?:e|err|error|source|cause|[a-z][a-z0-9_]*_err|[a-z][a-z0-9_]*_error)`
const TO_STRING_ERROR = String.raw`\b${ERROR_VARIABLE}\s*\.\s*to_string\s*\(\s*\)`
const INTERPOLATED_ERROR = String.raw`"[^"]*\{${ERROR_VARIABLE}(?::[^}]*)?\}[^"]*"`
const POSITIONAL_ERROR = String.raw`"[^"]*\{(?::[^}]*)?\}[^"]*"\s*,[^;]*\b${ERROR_VARIABLE}\b`
const ERROR_SOURCE_RULES = [
  {
    kind: 'S1',
    code: new RegExp(String.raw`\banyhow!\s*\(\s*${TO_STRING_ERROR}\s*\)|\bError::msg\s*\(\s*${ERROR_VARIABLE}\s*\)`),
    message: '不得把下层错误字符串化后重新包装成 anyhow；直接 ? 或 anyhow::Error::new',
  },
  {
    kind: 'S2',
    raw: new RegExp(String.raw`\b(?:anyhow|bail)!\s*\(\s*(?:${INTERPOLATED_ERROR}|${POSITIONAL_ERROR})`),
    message: '不得把下层错误拼进 anyhow 文本；改用 .context("固定动作")',
  },
  {
    kind: 'S3',
    code: new RegExp(
      String.raw`::\s*[A-Z]\w*\s*\(\s*${TO_STRING_ERROR}\s*[,)]` +
        String.raw`|\b(?:message|detail|details|reason|description|cause|error)\s*:\s*${TO_STRING_ERROR}` +
        String.raw`|\bmap_err\s*\(\s*\|\s*(\w+)\s*\|\s*\1\s*\.\s*to_string\s*\(\s*\)\s*\)`
    ),
    raw: new RegExp(
      String.raw`::\s*[A-Z]\w*\s*\(\s*format!\s*\(\s*(?:${INTERPOLATED_ERROR}|${POSITIONAL_ERROR})` +
        String.raw`|\b[a-z_]+\s*:\s*format!\s*\(\s*${INTERPOLATED_ERROR}`
    ),
    message: '错误变体不得只保存下层错误文本；改为 #[source] 携带具体错误',
  },
  {
    kind: 'L1',
    code: new RegExp(
      String.raw`\b(?:error|err|cause|source)\s*=\s*[%?]\s*&?${ERROR_VARIABLE}\b` +
        String.raw`|[(,]\s*[%?]\s*${ERROR_VARIABLE}\s*[,)]`
    ),
    raw: new RegExp(String.raw`\b(?:error|warn|info|debug|trace)!\s*\([^;]*${INTERPOLATED_ERROR}`),
    message: '日志不得输出错误正文；写固定 error_kind，并用 io_error_kind(..) 从来源链提取分类',
  },
]
// 错误文本与 panic 文本不得包含路径：`.display()` 出现在错误构造的同一行或其后三行内。
const ERROR_TEXT_START = /\b(?:with_context|anyhow!|bail!|panic!|custom)\s*\(|\bcontext\s*\(\s*format!/
const PATH_DISPLAY = /\.\s*display\s*\(\s*\)/
const DISCARDED_SOURCE = /\bmap_err\s*\(\s*(?:move\s*)?\|\s*_\w*\s*(?::[^|]*)?\|/
const CHINESE_COMMENT = /\/\/.*[\u4e00-\u9fff]/
const ANY_COMMENT = /\/\/.*\S/
// Matches the table in error-handling.md ("Allowed cases for discarding the source") one to one;
// reasons outside this list are rejected.
export const DISCARD_CATEGORIES = [
  'lock-poisoned',
  'int-conversion',
  'timeout',
  'channel',
  'no-information',
  'input-validation',
  'core-pure-validation',
  'observability-init',
  'business-outcome',
  'contract-boundary',
  'in-memory-encoding',
]
const DISCARD_TAG = /discarded-source\[([^\]]+)\]/
const FUNCTION_START = /(^|\n)\s*(pub(?:\s*\([^)]*\))?\s+)?(?:const\s+)?(?:unsafe\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\b/g

function git(args) {
  const result = spawnSync('git', args, {
    cwd: REPOSITORY_ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  })
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? '')
    throw new Error(`git ${args.join(' ')} failed`)
  }
  return result.stdout
}

function validBase(value) {
  return value && !/^0+$/.test(value)
}

function diffText() {
  const configuredBase = process.env.RUST_STYLE_BASE_SHA
  if (validBase(configuredBase)) {
    return git(['diff', '--unified=0', '--no-ext-diff', configuredBase, 'HEAD', '--', ...SOURCE_ROOTS])
  }
  return git(['diff', '--unified=0', '--no-ext-diff', 'HEAD', '--', ...SOURCE_ROOTS])
}

function addedLinesFromDiff(input) {
  const additions = []
  let path
  let currentLine = 0
  for (const line of input.split('\n')) {
    if (line.startsWith('+++ b/')) {
      path = line.slice(6)
      continue
    }
    const hunk = line.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
    if (hunk) {
      currentLine = Number(hunk[1])
      continue
    }
    if (!path || line.startsWith('--- ')) continue
    if (line.startsWith('+')) {
      additions.push({ path, line: currentLine })
      currentLine += 1
    } else if (!line.startsWith('-')) {
      currentLine += 1
    }
  }
  return additions
}

function changedFunctionLinesFromDiff(input) {
  const changes = []
  let path
  for (const line of input.split('\n')) {
    if (line.startsWith('+++ b/')) {
      path = line.slice(6)
      continue
    }
    const hunk = line.match(/^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@/)
    if (!path || !hunk) continue
    changes.push({ path, line: Math.max(1, Number(hunk[1])) })
  }
  return changes
}

function untrackedRustFiles() {
  return git(['ls-files', '--others', '--exclude-standard', '--', ...SOURCE_ROOTS])
    .split('\n')
    .filter(path => path.endsWith('.rs'))
}

function stripStringsAndComments(line, state) {
  let output = ''
  for (let index = 0; index < line.length; index += 1) {
    const current = line[index]
    const next = line[index + 1]
    if (state.blockComment) {
      if (current === '*' && next === '/') {
        state.blockComment = false
        index += 1
      }
      continue
    }
    if (state.string) {
      if (state.escape) {
        state.escape = false
      } else if (current === '\\') {
        state.escape = true
      } else if (current === state.string) {
        state.string = null
      }
      continue
    }
    if (current === '/' && next === '/') break
    if (current === '/' && next === '*') {
      state.blockComment = true
      index += 1
      continue
    }
    if (current === '"') {
      state.string = current
      continue
    }
    output += current
  }
  return output
}

function testLineNumbers(lines, codeLines) {
  const testLines = new Set()
  for (let index = 0; index < lines.length; index += 1) {
    if (!lines[index].trim().startsWith('#[cfg(test)]')) continue
    // 属性与空行之后的第一行是被标注的条目（模块、函数、impl、use 等）。
    let itemLine = index + 1
    while (itemLine < lines.length && (!lines[itemLine].trim() || lines[itemLine].trim().startsWith('#'))) itemLine += 1
    if (itemLine >= lines.length) continue
    let depth = 0
    let opened = false
    for (let cursor = itemLine; cursor < lines.length; cursor += 1) {
      const code = codeLines[cursor]
      testLines.add(cursor + 1)
      depth += [...code].filter(character => character === '{').length
      depth -= [...code].filter(character => character === '}').length
      if (code.includes('{')) opened = true
      // 没有花括号的条目（如 `use ..;`）在分号处结束。
      if (!opened && code.includes(';')) break
      if (opened && depth <= 0) break
    }
  }
  return testLines
}

function isTestPath(path) {
  return (
    path.startsWith('tests/') ||
    path.includes('/tests/') ||
    path.includes('/testing/') ||
    path.endsWith('/tests.rs') ||
    path.endsWith('/test_support.rs') ||
    path.includes('/test_support/') ||
    path.endsWith('_test.rs')
  )
}

function approvedException(lines, lineNumber) {
  return [lines[lineNumber - 1], lines[lineNumber - 2]].filter(Boolean).some(line => ALLOW_MARKER.test(line))
}

// 跨行写法（如 `Variant(` 换行后 `error.to_string(),`）拼接前一行一起判断。
function joinedWithPrevious(lines, lineNumber) {
  const current = lines[lineNumber - 1] ?? ''
  const previous = lines[lineNumber - 2] ?? ''
  return `${previous.trimEnd()} ${current.trim()}`
}

function errorSourceViolations(path, lines, codeLines, lineNumber) {
  const violations = []
  const code = codeLines[lineNumber - 1] ?? ''
  // 注释与空行跳过；只含字符串字面量的续行（如格式串单独成行）仍需检查。
  if (!code.trim() && !(lines[lineNumber - 1] ?? '').trim().startsWith('"')) return violations
  const joinedCode = joinedWithPrevious(codeLines, lineNumber)
  const joinedRaw = joinedWithPrevious(lines, lineNumber)
  const rawLine = lines[lineNumber - 1] ?? ''
  const previousCode = codeLines[lineNumber - 2] ?? ''
  const previousRaw = lines[lineNumber - 2] ?? ''
  // 当前行单独命中，或只有与前一行拼接后才命中（前一行单独命中时已在前一行报告）。
  const matches = (pattern, current, joined, previous) =>
    Boolean(pattern) && (pattern.test(current) || (pattern.test(joined) && !pattern.test(previous)))
  for (const rule of ERROR_SOURCE_RULES) {
    const matchesCode = matches(rule.code, code, joinedCode, previousCode)
    const matchesRaw = matches(rule.raw, rawLine, joinedRaw, previousRaw)
    if (!matchesCode && !matchesRaw) continue
    violations.push({ path, line: lineNumber, source: lines[lineNumber - 1].trim(), type: 'error-source', message: rule.message })
  }
  if (PATH_DISPLAY.test(code)) {
    const window = codeLines.slice(Math.max(0, lineNumber - 4), lineNumber).join('\n')
    if (ERROR_TEXT_START.test(window)) {
      violations.push({
        path,
        line: lineNumber,
        source: lines[lineNumber - 1].trim(),
        type: 'error-source',
        message: '错误与 panic 文本不得包含路径；改用固定动作文本，路径不进入错误链',
      })
    }
  }
  return violations
}

// 模块日志直接输出错误与 span 文本，因此 instrument 必须显式限定记录的字段，错误文本不得内插未包装的自由文本。
const INSTRUMENT_ATTRIBUTE = /#\[\s*(?:tracing::)?instrument\b/
const ERROR_ATTRIBUTE = /^\s*#\[error\(\s*"([^"]*)"/
const FREE_TEXT_TYPE = /\b(?:String|PathBuf|OsString|Vec<u8>|Cow<)|&\s*(?:'(?!static\b)\w+\s+)?str\b/

// span 字段与日志字段共用同一份字段目录：名称必须已登记，且不得用 err / ret 自动记录错误文本或返回值。
const LOG_FIELD_CATALOG_PATH = 'crates/uc-observability-contract/src/log_fields.rs'
let logFieldCatalog = null

function catalogFieldNames() {
  if (logFieldCatalog) return logFieldCatalog
  const source = readFileSync(new URL(`../../${LOG_FIELD_CATALOG_PATH}`, import.meta.url), 'utf8')
  logFieldCatalog = new Set([...source.matchAll(/^\s{4}([a-z][a-z0-9_]*):\s*[A-Z]\w*(?:\([a-z]+\))?,/gm)].map(match => match[1]))
  return logFieldCatalog
}

export function instrumentFieldProblems(attribute) {
  const problems = []
  const body = attribute.slice(attribute.indexOf('('))
  if (/[(,]\s*(?:err|ret)\b(?!\s*=)/.test(body.replace(/fields\s*\([\s\S]*\)\s*\)?/, ''))) {
    problems.push('#[instrument] 不得使用 err 或 ret，它们会把错误文本与返回值写入 span')
  }
  const fields = body.match(/\bfields\s*\(/)
  if (!fields) return problems
  let depth = 1
  let cursor = fields.index + fields[0].length
  let segment = ''
  const segments = []
  for (; cursor < body.length && depth > 0; cursor += 1) {
    const char = body[cursor]
    if ('([{'.includes(char)) depth += 1
    if (')]}'.includes(char)) depth -= 1
    if (depth === 0) break
    if (char === ',' && depth === 1) {
      segments.push(segment)
      segment = ''
    } else segment += char
  }
  segments.push(segment)
  for (const item of segments) {
    const name = item.trim().match(/^([A-Za-z_][\w.]*)/)?.[1]
    if (name && !catalogFieldNames().has(name)) {
      problems.push(`#[instrument] 字段 ${name} 未登记在日志字段目录（${LOG_FIELD_CATALOG_PATH}）`)
    }
  }
  return problems
}

function attributeText(lines, lineNumber) {
  let text = ''
  for (let index = lineNumber - 1; index < Math.min(lines.length, lineNumber + 15); index += 1) {
    text += `${lines[index]}\n`
    if (/\]\s*$/.test(lines[index].trimEnd()) && (text.match(/\[/g) ?? []).length <= (text.match(/\]/g) ?? []).length) break
  }
  return text
}

function fieldTypeFor(placeholder, followingLines) {
  const name = placeholder.split(':')[0].trim()
  if (/^\d+$/.test(name)) {
    const tuple = followingLines.find(line => /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct\s+)?[A-Z]\w*\s*\(/.test(line))
    if (!tuple) return null
    const inner = tuple.slice(tuple.indexOf('(') + 1, tuple.lastIndexOf(')'))
    return inner.split(/,(?![^<]*>)/)[Number(name)] ?? null
  }
  const pattern = new RegExp(String.raw`\b${name}\s*:\s*([^,}]+)`)
  for (const line of followingLines) {
    const match = line.match(pattern)
    if (match) return match[1]
  }
  return null
}

// 直接使用 tracing 日志宏已被禁止（ADR-030）：字段目录与值类别只由 `uc_*!` 宏在编译期保证。
// 唯一例外是故意测试运行期对未登记字段处理的观测运行期集成测试，这些文件顶部用 crate 级 allow 声明。
const RAW_LOG_MACRO_EXEMPT = [
  'crates/uc-observability-runtime/tests/host_composition.rs',
  'crates/uc-observability-runtime/tests/module_log_channel.rs',
  'crates/uc-observability-runtime/tests/otlp_http.rs',
]
// `uc_*!` 宏展开为 `tracing::event!`，所以只有观测 crate 自己可以直接使用它。
const EVENT_MACRO = /\b(?:tracing::)?event!\s*\(/
const EVENT_MACRO_OWNERS = ['crates/uc-observability-contract/', 'crates/uc-observability-runtime/']
const RAW_LOG_MACRO = /\b(?:tracing::)?(?:trace|debug|info|warn|error)!\s*\(/

function logPrivacyViolations(path, lines, codeLines, lineNumber) {
  const violations = []
  const code = codeLines[lineNumber - 1] ?? ''
  if (RAW_LOG_MACRO.test(code) && !RAW_LOG_MACRO_EXEMPT.some(exempt => path.endsWith(exempt))) {
    violations.push({
      path,
      line: lineNumber,
      source: lines[lineNumber - 1].trim(),
      type: 'error-source',
      message: '不得直接使用 tracing 日志宏；改用 uc_*! 宏（ADR-030）',
    })
  }
  if (EVENT_MACRO.test(code) && !EVENT_MACRO_OWNERS.some(owner => path.startsWith(owner))) {
    violations.push({
      path,
      line: lineNumber,
      source: lines[lineNumber - 1].trim(),
      type: 'error-source',
      message: '不得直接使用 tracing::event!；改用 uc_*! 日志宏（ADR-030）',
    })
  }
  if (INSTRUMENT_ATTRIBUTE.test(code)) {
    const text = attributeText(codeLines, lineNumber)
    if (!/\bskip_all\b|\bfields\s*\(/.test(text)) {
      violations.push({
        path,
        line: lineNumber,
        source: lines[lineNumber - 1].trim(),
        type: 'error-source',
        message: '#[instrument] 必须写 skip_all 或显式 fields(..)，避免参数自动进入 span 字段',
      })
    }
    for (const message of instrumentFieldProblems(text)) {
      violations.push({ path, line: lineNumber, source: lines[lineNumber - 1].trim(), type: 'error-source', message })
    }
  }
  const attribute = (lines[lineNumber - 1] ?? '').match(ERROR_ATTRIBUTE)
  if (attribute) {
    const following = []
    for (const line of lines.slice(lineNumber, lineNumber + 10)) {
      if (ERROR_ATTRIBUTE.test(line)) break
      following.push(line)
    }
    for (const placeholder of attribute[1].matchAll(/\{([^{}]*)\}/g)) {
      const type = fieldTypeFor(placeholder[1], following)
      if (type && FREE_TEXT_TYPE.test(type) && !type.includes('Sensitive<')) {
        violations.push({
          path,
          line: lineNumber,
          source: lines[lineNumber - 1].trim(),
          type: 'error-source',
          message: '#[error] 文本不得内插未包装的自由文本字段；用 Sensitive<..> 包装或改为固定文字/枚举名',
        })
        break
      }
    }
  }
  return violations
}

function lineNumberAt(source, offset) {
  return source.slice(0, offset).split('\n').length
}

function closingBraceAt(source, openingBrace) {
  let depth = 0
  for (let index = openingBrace; index < source.length; index += 1) {
    if (source[index] === '{') depth += 1
    if (source[index] === '}') depth -= 1
    if (depth === 0) return index
  }
  return null
}

function functionsIn(codeLines) {
  const source = codeLines.join('\n')
  const functions = []
  for (const match of source.matchAll(FUNCTION_START)) {
    const visibility = match[2]?.trim() ?? ''
    const name = match[3]
    const openingBrace = source.indexOf('{', match.index + match[0].length)
    const declarationEnd = source.indexOf(';', match.index + match[0].length)
    if (openingBrace === -1 || (declarationEnd !== -1 && declarationEnd < openingBrace)) continue
    const closingBrace = closingBraceAt(source, openingBrace)
    if (closingBrace === null) continue
    functions.push({
      body: source.slice(openingBrace + 1, closingBrace),
      endLine: lineNumberAt(source, closingBrace),
      name,
      startLine: lineNumberAt(source, match.index + match[1].length),
      visibility,
    })
  }
  return functions
}

function forwardingMethod(functionInfo) {
  if (functionInfo.visibility === 'pub') return null
  const body = functionInfo.body.trim().replace(/^return\s+/, '').replace(/;\s*$/, '').trim()
  if (body.includes(';')) return null
  const forwarding = body.match(
    /^self\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)\s*\([\s\S]*\)(?:\s*\.\s*await)?(?:\s*\?)?$/
  )
  if (!forwarding || forwarding[1] === functionInfo.name) return null
  return forwarding[1]
}

// 列出非测试代码中所有丢弃下层错误的 `map_err(|_| ..)`，附带紧邻注释里的类别标签（没有则为 null）。
export function discardedSourceSites(source, path = 'fixture.rs') {
  const lines = source.split('\n')
  const lexicalState = { blockComment: false, string: null, escape: false }
  const codeLines = lines.map(line => stripStringsAndComments(line, lexicalState))
  const testLines = testLineNumbers(lines, codeLines)
  const sites = []
  for (let lineNumber = 1; lineNumber <= lines.length; lineNumber += 1) {
    if (testLines.has(lineNumber) || !DISCARDED_SOURCE.test(codeLines[lineNumber - 1] ?? '')) continue
    sites.push({
      path,
      line: lineNumber,
      source: lines[lineNumber - 1].trim(),
      category: discardCategory(lines, lineNumber),
      commented: precedingComment(lines, lineNumber).length > 0,
    })
  }
  return sites
}

// 同一行，或紧邻其上连续的 `//` 注释行（至多 6 行）。
function precedingComment(lines, lineNumber) {
  const collected = []
  const current = lines[lineNumber - 1] ?? ''
  if (ANY_COMMENT.test(current)) collected.push(current)
  for (let index = lineNumber - 2; index >= 0 && lineNumber - 2 - index < 6; index -= 1) {
    const line = lines[index].trim()
    if (!line.startsWith('//')) break
    collected.unshift(line)
  }
  return collected
}

function discardCategory(lines, lineNumber) {
  const text = precedingComment(lines, lineNumber).join('\n')
  const tag = text.match(DISCARD_TAG)
  return tag ? tag[1] : null
}

function discardedSourceViolations(path, lines, sites) {
  const violations = []
  for (const site of sites) {
    let message = null
    if (site.category === null) {
      message =
        'map_err(|_| ..) 丢弃了下层错误；改为 #[source] 携带。属于 error-handling.md 允许的例外时，在同一行或紧邻上方的注释里写 ' +
        `"discarded-source[category]: reason"，category 只能是：${DISCARD_CATEGORIES.join(', ')}`
    } else if (!DISCARD_CATEGORIES.includes(site.category)) {
      message = `discarded-source category "${site.category}" is not allowed; use one of: ${DISCARD_CATEGORIES.join(', ')}`
    }
    if (message) violations.push({ path: site.path ?? path, line: site.line, source: site.source, type: 'error-source', message })
  }
  return violations
}

function violationsFor(path, addedLines, changedFunctionLines) {
  const absolutePath = resolve(REPOSITORY_ROOT, path)
  if (!existsSync(absolutePath) || isTestPath(path)) return []
  const lines = readFileSync(absolutePath, 'utf8').split('\n')
  const lexicalState = { blockComment: false, string: null, escape: false }
  const codeLines = lines.map(line => stripStringsAndComments(line, lexicalState))
  const testLines = testLineNumbers(lines, codeLines)
  const violations = []
  for (const lineNumber of addedLines) {
    const raw = lines[lineNumber - 1] ?? ''
    const code = codeLines[lineNumber - 1] ?? ''
    if (!testLines.has(lineNumber)) {
      violations.push(...errorSourceViolations(path, lines, codeLines, lineNumber))
      violations.push(...logPrivacyViolations(path, lines, codeLines, lineNumber))
    }
    // `$crate::` 是宏卫生所需的路径，不属于可改成集中引入的正文路径。
    if (!/(?<!\$)\bcrate\s*::/.test(code)) continue
    if (/^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate\s*::/.test(code)) continue
    if (testLines.has(lineNumber) || approvedException(lines, lineNumber)) continue
    violations.push({ path, line: lineNumber, source: raw.trim() })
  }
  const selected = new Set([...addedLines, ...changedFunctionLines])
  for (const functionInfo of functionsIn(codeLines)) {
    const selectedFunction = Array.from(
      { length: functionInfo.endLine - functionInfo.startLine + 1 },
      (_, index) => functionInfo.startLine + index
    ).some(line => selected.has(line))
    if (!selectedFunction || testLines.has(functionInfo.startLine)) continue
    const target = forwardingMethod(functionInfo)
    if (!target) continue
    violations.push({
      path,
      line: functionInfo.startLine,
      source: `${functionInfo.name} 只转调 ${target}`,
      type: 'forwarding-method',
    })
  }
  return violations
}

function selectedFiles() {
  if (process.argv[2] === '--file') {
    const absolutePath = resolve(process.argv[3] ?? '')
    if (!existsSync(absolutePath)) throw new Error('用于检查的 Rust 文件不存在')
    const path = relative(REPOSITORY_ROOT, absolutePath)
    const lineCount = readFileSync(absolutePath, 'utf8').split('\n').length
    const lines = Array.from({ length: lineCount }, (_, index) => index + 1)
    return new Map([[path, { addedLines: lines, changedFunctionLines: lines }]])
  }
  const selected = new Map()
  const diff = diffText()
  for (const addition of addedLinesFromDiff(diff)) {
    if (!addition.path.endsWith('.rs')) continue
    const lines = selected.get(addition.path) ?? { addedLines: [], changedFunctionLines: [] }
    lines.addedLines.push(addition.line)
    selected.set(addition.path, lines)
  }
  for (const change of changedFunctionLinesFromDiff(diff)) {
    if (!change.path.endsWith('.rs')) continue
    const lines = selected.get(change.path) ?? { addedLines: [], changedFunctionLines: [] }
    lines.changedFunctionLines.push(change.line)
    selected.set(change.path, lines)
  }
  for (const path of untrackedRustFiles()) {
    const lineCount = readFileSync(resolve(REPOSITORY_ROOT, path), 'utf8').split('\n').length
    const lines = Array.from({ length: lineCount }, (_, index) => index + 1)
    selected.set(path, { addedLines: lines, changedFunctionLines: lines })
  }
  return selected
}

// 全仓非测试 Rust 文件；丢弃来源的检查不只看新增行，存量代码同样适用。
function allProductionRustFiles() {
  return git(['ls-files', '--cached', '--others', '--exclude-standard', '--', ...SOURCE_ROOTS])
    .split('\n')
    .filter(path => path.endsWith('.rs') && !isTestPath(path) && existsSync(resolve(REPOSITORY_ROOT, path)))
}

function allDiscardedSites() {
  return allProductionRustFiles().flatMap(path =>
    discardedSourceSites(readFileSync(resolve(REPOSITORY_ROOT, path), 'utf8'), path)
  )
}

function discardedSourceScan(files) {
  if (files) {
    const absolutePath = resolve(files)
    const path = relative(REPOSITORY_ROOT, absolutePath)
    return discardedSourceViolations(path, [], discardedSourceSites(readFileSync(absolutePath, 'utf8'), path))
  }
  return discardedSourceViolations('', [], allDiscardedSites())
}

function main() {
  if (process.argv[2] === '--list-discarded') {
    process.stdout.write(`${JSON.stringify(allDiscardedSites(), null, 1)}\n`)
    return
  }
  const fileMode = process.argv[2] === '--file' ? process.argv[3] : null
  const violations = [
    ...[...selectedFiles()].flatMap(([path, lines]) => violationsFor(path, lines.addedLines, lines.changedFunctionLines)),
    ...discardedSourceScan(fileMode),
  ]
  if (violations.length === 0) {
    process.stdout.write('Rust 编写规范检查通过\n')
    return
  }
  for (const violation of violations) {
    if (violation.type === 'error-source') {
      process.stderr.write(`ERROR ${violation.path}:${violation.line} ${violation.message}：${violation.source}\n`)
      continue
    }
    if (violation.type === 'forwarding-method') {
      process.stderr.write(
        `ERROR ${violation.path}:${violation.line} 仓库内部方法不得只保留转调：${violation.source}\n`
      )
      continue
    }
    process.stderr.write(
      `ERROR ${violation.path}:${violation.line} 正文请先集中引入名称：${violation.source}\n`
    )
  }
  if (violations.some(violation => violation.type === undefined)) {
    process.stderr.write(
      '确有必要时，在前一行添加 rust-style: allow-qualified-path 并写明具体理由。\n'
    )
  }
  process.exitCode = 1
}

if (resolve(process.argv[1] ?? '') === SCRIPT_PATH) main()

export { changedFunctionLinesFromDiff }
