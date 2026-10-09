#!/usr/bin/env node

import assert from 'node:assert/strict'
import test from 'node:test'

import { violationSites } from './check-direct-log-macros.mjs'

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
  assert.deepEqual([...sites.values()], [
    { file: 'crates/a/src/lib.rs', line: 10 },
    { file: 'crates/a/src/lib.rs', line: 20 },
  ])
})
