import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

const script = new URL('./merge-diagnostic-timeline.mjs', import.meta.url).pathname

function bundle(root, name, records) {
  const directory = join(root, name, 'logs', 'engine')
  mkdirSync(directory, { recursive: true })
  writeFileSync(join(directory, 'engine.2026-09-29.jsonl'), records.map((r) => JSON.stringify(r)).join('\n') + '\n')
  return join(root, name)
}

test('merges two bundles into one trace timeline with an explicit clock offset', () => {
  const root = mkdtempSync(join(tmpdir(), 'timeline-'))
  const phone = bundle(root, 'phone', [
    { timestamp: '2026-09-29T12:00:01.000000Z', trace_id: 'aa', source: 'engine_module', level: 'WARN', message: 'refused', 'error.root': 'root' },
    { timestamp: '2026-09-29T12:00:09.000000Z', trace_id: 'bb', message: 'other' },
  ])
  const desktop = bundle(root, 'desktop', [
    { timestamp: '2026-09-29T12:00:00.500000Z', trace_id: 'aa', message: 'request sent' },
  ])
  const run = spawnSync('node', [script, '--trace', 'aa', '--offset', 'desktop=+600', phone, desktop], { encoding: 'utf8' })
  assert.equal(run.status, 0, run.stderr)
  const lines = run.stdout.trim().split('\n')
  assert.equal(lines.length, 2)
  assert.match(lines[0], /^12:00:01\.000 phone.*refused.*root/)
  assert.match(lines[1], /^12:00:01\.100 desktop.*request sent/)
})

test('rejects a run without inputs', () => {
  const run = spawnSync('node', [script], { encoding: 'utf8' })
  assert.notEqual(run.status, 0)
})
