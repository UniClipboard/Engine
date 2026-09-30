import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import test from 'node:test'

const checker = new URL('./check-module-log-errors.mjs', import.meta.url).pathname

function run(files) {
  const root = mkdtempSync(join(tmpdir(), 'module-log-errors-'))
  const source = join(root, 'crates', 'uc-fixture', 'src')
  mkdirSync(source, { recursive: true })
  for (const [name, text] of Object.entries(files)) writeFileSync(join(source, name), text)
  return spawnSync(process.execPath, [checker, '--root', root], { encoding: 'utf8' })
}

test('registered error types with fixed text pass', () => {
  const result = run({
    'lib.rs': `
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    #[error("refused {reason}")]
    Fixed { reason: &'static str },
}
uc_observability_contract::log_safe_errors!(fn probes => [Refused]);
`,
  })
  assert.equal(result.status, 0, result.stderr)
})

test('registered error type that interpolates free text is rejected', () => {
  const result = run({
    'lib.rs': `
#[derive(Debug, thiserror::Error)]
pub enum Leaky {
    #[error("cannot open {path}")]
    Open { path: std::path::PathBuf },
}
uc_observability_contract::log_safe_errors!(fn probes => [Leaky]);
`,
  })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /Leaky/)
})

test('unregistered types are not audited', () => {
  const result = run({
    'lib.rs': `
#[derive(Debug, thiserror::Error)]
pub enum Other {
    #[error("bad {0}")]
    Bad(String),
}
`,
  })
  assert.equal(result.status, 0, result.stderr)
})

test('registered types that cannot be found are reported', () => {
  const result = run({ 'lib.rs': `uc_observability_contract::log_safe_errors!(fn probes => [Missing]);\n` })
  assert.equal(result.status, 1)
  assert.match(result.stderr, /Missing/)
})
