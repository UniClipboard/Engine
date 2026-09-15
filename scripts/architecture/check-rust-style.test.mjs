#!/usr/bin/env node

import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { changedFunctionLinesFromDiff } from './check-rust-style.mjs'

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

test('拒绝仓库内部方法只补固定参数后转调', () => {
  const result = check(`
impl Maintenance {
    pub(super) async fn execute(&self) -> StepOutcome {
        self.execute_for_trigger(&MaintenanceTrigger::Periodic).await
    }

    async fn execute_for_trigger(&self, trigger: &MaintenanceTrigger) -> StepOutcome {
        run(trigger).await
    }
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:3/)
  assert.match(result.stderr, /execute 只转调 execute_for_trigger/)
  assert.doesNotMatch(result.stderr, /allow-qualified-path/)
})

test('拒绝私有方法原样转调并传播失败', () => {
  const result = check(`
impl Worker {
    async fn run(&self, input: Input) -> Result<Output> {
        self.run_inner(input).await?
    }
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /run 只转调 run_inner/)
})

test('接受真正公开的稳定入口转调', () => {
  const result = check(`
impl Engine {
    pub async fn start(&self) -> Result<()> {
        self.start_inner(Default::default()).await
    }
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('接受包含实际处理的仓库内部方法', () => {
  const result = check(`
impl Maintenance {
    pub(super) async fn execute(&self, trigger: &MaintenanceTrigger) -> StepOutcome {
        let outcome = self.execute_step(trigger).await;
        self.record_outcome(&outcome);
        outcome
    }
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('接受先调用本机方法再继续处理的内部方法', () => {
  const result = check(`
impl Repository {
    async fn reload(&self) -> Result<Snapshot> {
        self.clear_cache()?;
        let record = self.loader.load().await?;
        self.cache(record)
    }
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('只删除函数内容时仍把该函数列入检查', () => {
  const changes = changedFunctionLinesFromDiff(`
diff --git a/crates/example.rs b/crates/example.rs
--- a/crates/example.rs
+++ b/crates/example.rs
@@ -12,2 +12,0 @@ impl Worker {
-        validate();
-        record();
`)

  assert.deepEqual(changes, [{ path: 'crates/example.rs', line: 12 }])
})
