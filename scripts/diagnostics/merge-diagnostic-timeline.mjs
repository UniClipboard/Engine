#!/usr/bin/env node
// 把多份诊断导出（zip 或已解压目录）按 trace_id 合并成一条时间线。
// 用法：merge-diagnostic-timeline.mjs [--trace <id>] [--offset <输入名>=<毫秒>]... <输入>...
// 偏移用于修正该输入的时钟：合并时间 = 记录时间 + 偏移；未给出偏移时按原始时间排序，并在末尾提示。
import { execFileSync } from 'node:child_process'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { basename, join } from 'node:path'

function parseArguments(argv) {
  const options = { trace: null, offsets: new Map(), inputs: [] }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--trace') options.trace = argv[++index]
    else if (argument === '--offset') {
      const [name, value] = (argv[++index] ?? '').split('=')
      const milliseconds = Number(value)
      if (!name || !Number.isFinite(milliseconds)) throw new Error('--offset 需要 <输入名>=<毫秒>')
      options.offsets.set(name, milliseconds)
    } else options.inputs.push(argument)
  }
  if (options.inputs.length === 0) throw new Error('至少需要一个诊断导出输入')
  return options
}

function jsonlFromDirectory(root) {
  const found = []
  const walk = (directory) => {
    for (const entry of readdirSync(directory)) {
      const path = join(directory, entry)
      if (statSync(path).isDirectory()) walk(path)
      else if (/engine.*\.jsonl$/.test(entry)) found.push(readFileSync(path, 'utf8'))
    }
  }
  walk(root)
  return found
}

function jsonlFromZip(path) {
  const listing = execFileSync('unzip', ['-Z1', path], { encoding: 'utf8' }).split('\n')
  return listing
    .filter((name) => /(^|\/)engine[^/]*\.jsonl$/.test(name))
    .map((name) => execFileSync('unzip', ['-p', path, name], { encoding: 'utf8', maxBuffer: 1 << 28 }))
}

function readInput(path) {
  if (!existsSync(path)) throw new Error(`输入不存在：${path}`)
  return statSync(path).isDirectory() ? jsonlFromDirectory(path) : jsonlFromZip(path)
}

function main() {
  const options = parseArguments(process.argv.slice(2))
  const rows = []
  for (const input of options.inputs) {
    const label = basename(input).replace(/\.zip$/, '')
    const offset = options.offsets.get(label) ?? 0
    for (const text of readInput(input)) {
      for (const line of text.split('\n')) {
        if (!line.trim()) continue
        let record
        try {
          record = JSON.parse(line)
        } catch {
          continue
        }
        if (!record.trace_id || (options.trace && record.trace_id !== options.trace)) continue
        const time = Date.parse(record.timestamp)
        if (Number.isNaN(time)) continue
        rows.push({ label, at: time + offset, record })
      }
    }
  }
  rows.sort((left, right) => left.at - right.at)
  for (const { label, at, record } of rows) {
    const chain = record['error.root'] ? ` root=${JSON.stringify(record['error.root'])}` : ''
    const kind = record.source === 'engine_module' ? 'module' : 'contract'
    const message = record.message ?? record.fields?.['event.name'] ?? ''
    console.log(`${new Date(at).toISOString().slice(11, 23)} ${label} ${kind} ${record.level ?? ''} ${record.trace_id} ${message}${chain}`)
  }
  if (options.offsets.size === 0 && options.inputs.length > 1) {
    console.error('提示：未提供 --offset，跨设备顺序按各自原始时钟排列，不能据此推断因果。')
  }
}

try {
  main()
} catch (error) {
  console.error(error.message)
  process.exit(1)
}
