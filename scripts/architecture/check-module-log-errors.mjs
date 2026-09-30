#!/usr/bin/env node
// 模块日志只渲染 `log_safe_errors!` 登记过的错误类型的 `Display`；登记的类型其 #[error] 文本不得内插未包装的自由文本。
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const FREE_TEXT_TYPE = /\b(?:String|PathBuf|OsString|Vec<u8>|Cow<)|&\s*(?:'(?!static\b)\w+\s+)?str\b/
const ERROR_ATTRIBUTE = /^\s*#\[error\(\s*"([^"]*)"/

function rootFromArguments() {
  const index = process.argv.indexOf('--root')
  return index === -1 ? resolve(fileURLToPath(new URL('../..', import.meta.url))) : resolve(process.argv[index + 1])
}

function rustFiles(directory) {
  const found = []
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry)
    if (statSync(path).isDirectory()) found.push(...rustFiles(path))
    else if (entry.endsWith('.rs')) found.push(path)
  }
  return found
}

function registeredTypes(files) {
  const registered = new Map()
  for (const [path, text] of files) {
    for (const match of text.matchAll(/log_safe_errors!\s*\(([^;]*?)=>\s*\[([^\]]*)\]/gs)) {
      for (const name of match[2].split(',').map(item => item.trim()).filter(Boolean)) {
        registered.set(name.split('::').pop(), path)
      }
    }
  }
  return registered
}

function definition(lines, name) {
  const start = lines.findIndex(line => new RegExp(String.raw`\b(?:enum|struct)\s+${name}\b`).test(line))
  if (start === -1) return null
  let depth = 0
  let opened = false
  for (let index = start; index < lines.length; index += 1) {
    for (const character of lines[index]) {
      if (character === '{') {
        depth += 1
        opened = true
      }
      if (character === '}') depth -= 1
    }
    if (opened && depth === 0) return { first: start, last: index }
  }
  return { first: start, last: lines.length - 1 }
}

function fieldType(placeholder, following) {
  const name = placeholder.split(':')[0].trim()
  if (/^\d+$/.test(name)) {
    const tuple = following.find(line => /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct\s+)?[A-Z]\w*\s*\(/.test(line))
    if (!tuple) return null
    const inner = tuple.slice(tuple.indexOf('(') + 1, tuple.lastIndexOf(')'))
    return inner.split(/,(?![^<]*>)/)[Number(name)] ?? null
  }
  for (const line of following) {
    const match = line.match(new RegExp(String.raw`\b${name}\s*:\s*([^,}]+)`))
    if (match) return match[1]
  }
  return null
}

function main() {
  const root = rootFromArguments()
  const files = rustFiles(join(root, 'crates')).map(path => [path, readFileSync(path, 'utf8')])
  const problems = []
  for (const [name, registeredIn] of registeredTypes(files)) {
    let found = false
    for (const [path, text] of files) {
      const lines = text.split('\n')
      const span = definition(lines, name)
      if (!span) continue
      found = true
      for (let index = span.first; index <= span.last; index += 1) {
        const attribute = lines[index].match(ERROR_ATTRIBUTE)
        if (!attribute) continue
        const following = []
        for (const line of lines.slice(index + 1, index + 11)) {
          if (ERROR_ATTRIBUTE.test(line)) break
          following.push(line)
        }
        for (const placeholder of attribute[1].matchAll(/\{([^{}]*)\}/g)) {
          const type = fieldType(placeholder[1], following)
          if (type && FREE_TEXT_TYPE.test(type) && !type.includes('Sensitive<')) {
            problems.push(`${relative(root, path)}:${index + 1} 已登记的错误类型 ${name} 内插了未包装的自由文本：${lines[index].trim()}`)
            break
          }
        }
      }
    }
    if (!found) problems.push(`${relative(root, registeredIn)} 登记的错误类型 ${name} 找不到定义`)
  }
  if (problems.length > 0) {
    for (const problem of problems) process.stderr.write(`ERROR ${problem}\n`)
    process.exit(1)
  }
  process.stdout.write('模块日志错误类型登记检查通过\n')
}

main()
