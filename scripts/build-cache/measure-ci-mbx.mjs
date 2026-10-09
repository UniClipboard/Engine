#!/usr/bin/env node
// 两个独立空 target 共用动作缓存，验证恢复后仍实际执行调用方命令；不删除既有 target。
import { spawnSync } from 'node:child_process'
import { mkdirSync, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'

const split = process.argv.indexOf('--')
const outIndex = process.argv.indexOf('--out')
if (split < 0 || outIndex < 0 || !process.argv[outIndex + 1]) throw new Error('usage: measure-ci-mbx.mjs --out <new-directory> -- <command> [args...]')
const out = resolve(process.argv[outIndex + 1])
if (existsSync(out)) throw new Error('evidence directory must be new')
mkdirSync(out, { recursive: true })
const command = process.argv.slice(split + 1)
if (!command.length) throw new Error('build command is required')
const targetRoot = join(process.cwd(), 'target', 'mbx-reuse-' + Date.now())
const results = []
for (const phase of ['populate', 'restore']) {
  const stats = join(out, phase + '.json')
  const env = { ...process.env, CARGO_TARGET_DIR: join(targetRoot, phase), MBX_CACHE_DIR: join(out, 'cache'), MBX_STATS_REPORT: stats }
  const start = process.hrtime.bigint()
  const result = spawnSync(command[0], command.slice(1), { env, stdio: 'inherit' })
  const seconds = Number(process.hrtime.bigint() - start) / 1e9
  if (result.status !== 0) process.exit(result.status ?? 1)
  const report = JSON.parse(readFileSync(stats, 'utf8'))
  results.push({ phase, seconds, hits: report.hits, misses: report.misses, downloaded_bytes: report.downloaded_bytes, uploaded_bytes: report.uploaded_bytes, remote_failures: report.remote_failures, bypasses: report.bypasses })
  writeFileSync(join(out, 'summary.json'), JSON.stringify({ command, targetRoot, results }, null, 2) + '\n')
  if (report.remote_failures || report.background_upload_failures) throw new Error('MBX remote cache errors; evidence retained')
  if (phase === 'restore' && !(report.hits > 0 && report.restored_output_files > 0)) throw new Error('independent target did not restore cached outputs')
}
console.log(JSON.stringify({ command, targetRoot, results }, null, 2))
