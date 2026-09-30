#!/usr/bin/env node

import assert from 'node:assert/strict'
import test from 'node:test'

import { compareToBaseline, countByFile, nextBaseline, violationSites } from './check-log-macro-ratchet.mjs'

function message(code, file, line, column, primary = true) {
  return JSON.stringify({
    reason: 'compiler-message',
    message: { code: { code }, spans: [{ is_primary: primary, file_name: file, line_start: line, column_start: column }] },
  })
}

test('只统计仓库内 disallowed_macros 的唯一位置', () => {
  const sites = violationSites([
    message('clippy::disallowed_macros', 'crates/a/src/lib.rs', 10, 5),
    message('clippy::disallowed_macros', 'crates/a/src/lib.rs', 10, 5),
    message('clippy::disallowed_macros', 'crates/a/src/lib.rs', 20, 5),
    message('clippy::disallowed_macros', '/Users/x/.cargo/registry/tracing/src/macros.rs', 1, 1),
    message('clippy::needless_return', 'crates/a/src/lib.rs', 30, 5),
    message('clippy::disallowed_macros', 'crates/b/src/lib.rs', 1, 1, false),
    'not json',
  ])
  assert.deepEqual(countByFile(sites), { 'crates/a/src/lib.rs': 2 })
})

test('数量增加与下降都需要处理', () => {
  const baseline = { 'a.rs': 3, 'b.rs': 2 }
  assert.deepEqual(compareToBaseline(baseline, { 'a.rs': 3, 'b.rs': 2 }), { increases: [], decreases: [] })
  assert.deepEqual(compareToBaseline(baseline, { 'a.rs': 4, 'b.rs': 2 }).increases, [{ file: 'a.rs', before: 3, after: 4 }])
  assert.deepEqual(compareToBaseline(baseline, { 'a.rs': 3 }).decreases, [{ file: 'b.rs', before: 2, after: 0 }])
  assert.deepEqual(compareToBaseline(baseline, { 'a.rs': 3, 'b.rs': 2, 'c.rs': 1 }).increases, [{ file: 'c.rs', before: 0, after: 1 }])
})

test('更新基线默认拒绝上调', () => {
  assert.throws(() => nextBaseline({ 'a.rs': 1 }, { 'a.rs': 2 }), /拒绝上调基线/)
  assert.deepEqual(nextBaseline({ 'a.rs': 2 }, { 'a.rs': 1 }), { version: 1, total: 1, files: { 'a.rs': 1 } })
  assert.equal(nextBaseline({ 'a.rs': 1 }, { 'a.rs': 2 }, { allowIncrease: true }).total, 2)
})
