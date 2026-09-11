#!/usr/bin/env node

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const CHECKER = join(SCRIPT_DIR, 'check-rust-style.mjs')

function check(source) {
  const directory = mkdtempSync(join(tmpdir(), 'uc-rust-style-'))
  const fixture = join(directory, 'fixture.rs')
  writeFileSync(fixture, source)
  const result = spawnSync(process.execPath, [CHECKER, '--file', fixture], {
    encoding: 'utf8',
  })
  rmSync(directory, { recursive: true, force: true })
  return result
}

test('接受集中引入和公开入口测试', () => {
  const result = check(`
use crate::{Error, Result};

fn run() -> Result<(), Error> { Ok(()) }

#[cfg(test)]
mod tests {
    #[test]
    fn public_contract() { crate::run(); }
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('拒绝正文和签名中新增长路径', () => {
  const result = check(`
fn run(value: crate::Value) -> crate::Result<()> {
    crate::service::execute(value)
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:2/)
  assert.match(result.stderr, /fixture\.rs:3/)
})

test('接受写明具体理由的局部例外', () => {
  const result = check(`
fn run() {
    // rust-style: allow-qualified-path -- 宏要求从 crate 根解析名称
    crate::generated_macro_entry!();
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('空泛或缺失理由不能绕过检查', () => {
  const result = check(`
fn run() {
    // rust-style: allow-qualified-path --
    crate::service::execute();
}
`)
  assert.equal(result.status, 1)
})

test('生命周期参数不能遮住后面的违规路径', () => {
  const result = check(`
fn borrow<'a>(value: &'a crate::Value) -> &'a str {
    value.as_str()
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:2/)
})

test('字符串和注释中的示例不算生产引用', () => {
  const result = check(`
const EXAMPLE: &str = "crate::service::execute";
// crate::service::execute();
/*
crate::service::execute();
*/
`)
  assert.equal(result.status, 0, result.stderr)
})
