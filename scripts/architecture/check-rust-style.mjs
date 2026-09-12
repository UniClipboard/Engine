#!/usr/bin/env node

import { spawnSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(SCRIPT_DIR, '../..')
const SOURCE_ROOTS = ['crates', 'bindings', 'compatibility', 'tests']
const ALLOW_MARKER = /\/\/\s*rust-style:\s*allow-qualified-path\s*--\s*\S.+$/

function git(args) {
  const result = spawnSync('git', args, {
    cwd: REPOSITORY_ROOT,
    encoding: 'utf8',
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
    let moduleLine = index + 1
    while (moduleLine < lines.length && !/\bmod\s+\w+\s*\{/.test(lines[moduleLine])) {
      if (lines[moduleLine].trim() && !lines[moduleLine].trim().startsWith('#')) break
      moduleLine += 1
    }
    if (moduleLine >= lines.length || !/\bmod\s+\w+\s*\{/.test(lines[moduleLine])) continue
    let depth = 0
    for (let cursor = moduleLine; cursor < lines.length; cursor += 1) {
      const code = codeLines[cursor]
      depth += [...code].filter(character => character === '{').length
      depth -= [...code].filter(character => character === '}').length
      testLines.add(cursor + 1)
      if (depth === 0) break
    }
  }
  return testLines
}

function isTestPath(path) {
  return (
    path.startsWith('tests/') ||
    path.includes('/tests/') ||
    path.includes('/src/testing/') ||
    path.endsWith('/tests.rs') ||
    path.endsWith('/test_support.rs') ||
    path.endsWith('_test.rs')
  )
}

function approvedException(lines, lineNumber) {
  return [lines[lineNumber - 1], lines[lineNumber - 2]].filter(Boolean).some(line => ALLOW_MARKER.test(line))
}

function violationsFor(path, selectedLines) {
  const absolutePath = resolve(REPOSITORY_ROOT, path)
  if (!existsSync(absolutePath) || isTestPath(path)) return []
  const lines = readFileSync(absolutePath, 'utf8').split('\n')
  const lexicalState = { blockComment: false, string: null, escape: false }
  const codeLines = lines.map(line => stripStringsAndComments(line, lexicalState))
  const testLines = testLineNumbers(lines, codeLines)
  const violations = []
  for (const lineNumber of selectedLines) {
    const raw = lines[lineNumber - 1] ?? ''
    const code = codeLines[lineNumber - 1] ?? ''
    if (!/\bcrate\s*::/.test(code)) continue
    if (/^\s*(?:pub(?:\([^)]*\))?\s+)?use\s+crate\s*::/.test(code)) continue
    if (testLines.has(lineNumber) || approvedException(lines, lineNumber)) continue
    violations.push({ path, line: lineNumber, source: raw.trim() })
  }
  return violations
}

function selectedFiles() {
  if (process.argv[2] === '--file') {
    const absolutePath = resolve(process.argv[3] ?? '')
    if (!existsSync(absolutePath)) throw new Error('用于检查的 Rust 文件不存在')
    const path = relative(REPOSITORY_ROOT, absolutePath)
    const lineCount = readFileSync(absolutePath, 'utf8').split('\n').length
    return new Map([[path, Array.from({ length: lineCount }, (_, index) => index + 1)]])
  }
  const selected = new Map()
  for (const addition of addedLinesFromDiff(diffText())) {
    if (!addition.path.endsWith('.rs')) continue
    const lines = selected.get(addition.path) ?? []
    lines.push(addition.line)
    selected.set(addition.path, lines)
  }
  for (const path of untrackedRustFiles()) {
    const lineCount = readFileSync(resolve(REPOSITORY_ROOT, path), 'utf8').split('\n').length
    selected.set(path, Array.from({ length: lineCount }, (_, index) => index + 1))
  }
  return selected
}

function main() {
  const violations = [...selectedFiles()].flatMap(([path, lines]) => violationsFor(path, lines))
  if (violations.length === 0) {
    process.stdout.write('Rust 编写规范检查通过\n')
    return
  }
  for (const violation of violations) {
    process.stderr.write(
      `ERROR ${violation.path}:${violation.line} 正文请先集中引入名称：${violation.source}\n`
    )
  }
  process.stderr.write(
    '确有必要时，在前一行添加 rust-style: allow-qualified-path 并写明具体理由。\n'
  )
  process.exitCode = 1
}

main()
