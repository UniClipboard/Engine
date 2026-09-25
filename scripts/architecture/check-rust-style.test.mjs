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

test('拒绝把下层错误字符串化后重新包装成 anyhow', () => {
  const result = check(`
fn run() -> anyhow::Result<()> {
    load().map_err(|e| anyhow::anyhow!(e.to_string()))?;
    save().map_err(anyhow::Error::msg)?;
    parse().map_err(|error| anyhow::Error::msg(error))?;
    Ok(())
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:3 .*anyhow::Error::new/)
  assert.match(result.stderr, /fixture\.rs:5 /)
})

test('拒绝把下层错误拼进 anyhow 文本', () => {
  const result = check(`
fn run() -> anyhow::Result<()> {
    load().map_err(|e| anyhow!("load failed: {e}"))?;
    save().map_err(|err| anyhow::anyhow!("save failed: {}", err))?;
    let error = parse().unwrap_err();
    bail!(
        "parse failed: {error:?}"
    );
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:3 .*context/)
  assert.match(result.stderr, /fixture\.rs:4 /)
  assert.match(result.stderr, /fixture\.rs:7 /)
})

test('接受固定动作文本与 context', () => {
  const result = check(`
fn run() -> anyhow::Result<()> {
    load().context("load clipboard entry")?;
    save().map_err(anyhow::Error::new)?;
    let count = 3;
    anyhow::ensure!(count > 0, "count {count} must be positive");
    Err(anyhow!("unsupported version {}", version))
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('拒绝错误变体只保存下层错误文本', () => {
  const result = check(`
fn run() -> Result<(), StoreError> {
    load().map_err(|e| StoreError::Storage(e.to_string()))?;
    save().map_err(|error| StoreError::Io(format!("write failed: {error}")))?;
    read().map_err(|error| StoreError::Decode {
        reason: error.to_string(),
    })?;
    open().map_err(|err| {
        StoreError::Open(
            err.to_string(),
        )
    })?;
    close().map_err(|e| e.to_string())?;
    Ok(())
}
`)
  assert.equal(result.status, 1)
  for (const line of [3, 4, 6, 10, 13]) assert.match(result.stderr, new RegExp(`fixture\\.rs:${line} .*#\\[source\\]`))
})

test('拒绝无理由丢弃下层错误', () => {
  const result = check(`
fn run() -> Result<(), StoreError> {
    load().map_err(|_| StoreError::Storage)?;
    save().map_err(|_error| StoreError::Storage)?;
    Ok(())
}
`)
  assert.equal(result.status, 1)
  assert.match(result.stderr, /fixture\.rs:3 .*中文注释/)
  assert.match(result.stderr, /fixture\.rs:4 /)
})

test('接受写明中文理由的丢弃来源例外', () => {
  const result = check(`
fn run(bytes: &[u8]) -> Result<[u8; 32], KeyError> {
    // TryFromSliceError 只表示长度不符，目标分类已完整表达
    let key = bytes.try_into().map_err(|_| KeyError::Length)?;
    let guard = lock.lock().map_err(|_| KeyError::Poisoned)?; // 锁中毒持有 guard，不能保存
    Ok(key)
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('拒绝日志输出错误正文', () => {
  const result = check(`
fn run() {
    if let Err(err) = load() {
        warn!(error = %err, "load failed");
    }
    if let Err(error) = save() {
        tracing::error!(
            entry = 1,
            error = ?error,
            "save failed"
        );
    }
    if let Err(error) = sync() {
        warn!(%error, "sync failed");
    }
    if let Err(e) = flush() {
        debug!("flush failed: {e:#}");
    }
}
`)
  assert.equal(result.status, 1)
  for (const line of [4, 9, 14, 17]) assert.match(result.stderr, new RegExp(`fixture\\.rs:${line} .*error_kind`))
})

test('接受固定分类的日志字段', () => {
  const result = check(`
fn run() {
    if let Err(err) = load() {
        warn!(error_kind = "load", io_error_kind = io_error_kind(&err), "load failed");
    }
    warn!(source = %source_label, reason = ?reason, error_kind = ?callback_error, "skipped");
}
`)
  assert.equal(result.status, 0, result.stderr)
})

test('测试代码不检查错误来源写法', () => {
  const result = check(`
fn run() {}

#[cfg(test)]
mod tests {
    fn helper() -> anyhow::Result<()> {
        load().map_err(|e| anyhow::anyhow!(e.to_string()))?;
        load().map_err(|_| StoreError::Storage)?;
        Ok(())
    }
}
`)
  assert.equal(result.status, 0, result.stderr)
})
