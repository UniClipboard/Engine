import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'

const runner = resolve(import.meta.dirname, 'with-isolated-source.mjs')

test('isolates ancestor Cargo patches without changing user settings; cleans success and failure', () => {
  const root = mkdtempSync(join(tmpdir(), 'engine-release-test-'))
  const source = join(root, 'parent', 'source')
  const scratch = join(root, 'scratch')
  try {
    mkdirSync(source, { recursive: true })
    mkdirSync(scratch)
    mkdirSync(join(source, 'src'))
    writeFileSync(join(source, 'Cargo.toml'), '[package]\nname="release-fixture"\nversion="0.1.0"\nedition="2021"\n')
    writeFileSync(join(source, 'src/lib.rs'), '')
    const cargo = args => spawnSync('cargo', args, { cwd: source, encoding: 'utf8' })
    assert.equal(cargo(['generate-lockfile', '--offline']).status, 0)
    const configDir = join(root, 'parent', '.cargo')
    mkdirSync(configDir)
    const config = '[patch.crates-io]\nmissing = { path = "../missing" }\n'
    writeFileSync(join(configDir, 'config.toml'), config)
    const args = ['metadata', '--locked', '--offline', '--format-version', '1']
    assert.notEqual(cargo(args).status, 0, 'ancestor patch must reproduce the failure')
    const git = args => execFileSync('git', args, { cwd: source, stdio: 'pipe' })
    git(['init', '-q'])
    git(['add', '.'])
    git(['-c', 'user.name=Release Test', '-c', 'user.email=release@example.invalid', 'commit', '-qm', 'fixture'])
    const head = git(['rev-parse', 'HEAD']).toString().trim()
    const probe = `
      const {execFileSync} = require('node:child_process');
      const assert = require('node:assert/strict');
      assert.equal(process.env.HOME, ${JSON.stringify(process.env.HOME)});
      assert.equal(execFileSync('git', ['rev-parse','HEAD']).toString().trim(), '${head}');
      assert.equal(execFileSync('rustup', ['show','home']).toString(), ${JSON.stringify(execFileSync('rustup', ['show', 'home']).toString())});
      execFileSync('cargo', ${JSON.stringify(args)}, {stdio:'pipe'});
    `
    const result = spawnSync(process.execPath, [runner, scratch, process.execPath, '-e', probe], { cwd: source, encoding: 'utf8' })
    assert.equal(result.status, 0, result.stderr)
    assert.equal(readFileSync(join(configDir, 'config.toml'), 'utf8'), config)
    assert.deepEqual(readdirSync(scratch), [])
    const failure = spawnSync(process.execPath, [runner, scratch, process.execPath, '-e', 'process.exit(7)'], { cwd: source, encoding: 'utf8' })
    assert.equal(failure.status, 7, failure.stderr)
    assert.deepEqual(readdirSync(scratch), [])
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
