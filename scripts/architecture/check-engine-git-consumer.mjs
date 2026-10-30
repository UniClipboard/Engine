#!/usr/bin/env node
// 从已提交的 Engine Git 来源构建独立宿主，防止根补丁掩盖下游依赖断链。
import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

const root = resolve(import.meta.dirname, '../..')
const revision = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim()
const consumer = mkdtempSync(join(tmpdir(), 'uc-engine-git-consumer-'))
const cargo = (args, capture = false) => execFileSync('just', ['cargo', ...args, '--manifest-path', join(consumer, 'Cargo.toml')], {
  cwd: root,
  encoding: 'utf8',
  stdio: capture ? ['ignore', 'pipe', 'inherit'] : 'inherit',
  maxBuffer: 64 * 1024 * 1024,
})
try {
  mkdirSync(join(consumer, 'src'))
  writeFileSync(join(consumer, 'src/lib.rs'), 'pub use uc_engine::*;\n')
  writeFileSync(join(consumer, 'Cargo.toml'), `[package]\nname = "engine-git-consumer"\nversion = "0.0.0"\nedition = "2021"\n[dependencies]\nuc-engine = { git = "${pathToFileURL(root).href}", rev = "${revision}" }\n[workspace]\n`)
  copyFileSync(join(root, 'Cargo.lock'), join(consumer, 'Cargo.lock'))
  const metadata = JSON.parse(cargo(['metadata', '--format-version', '1'], true))
  const engine = metadata.packages.find(pkg => pkg.name === 'uc-engine')
  for (const name of ['iroh', 'iroh-base', 'iroh-dns', 'iroh-relay', 'iroh-blobs', 'iroh-tickets', 'iroh-util', 'opaque-ke', 'voprf']) {
    const packages = metadata.packages.filter(pkg => pkg.name === name)
    if (packages.length !== 1 || packages[0].source !== engine.source) {
      throw new Error(`${name}: expected exactly one package from the Engine Git dependency`)
    }
  }
  for (const [name, rev] of [
    ['n0-dns-resolver', 'b15fdb6abd50714fd551b4fd93a9b5728457442d'],
    ['simple-dns', 'efdb13b9473131f7172b790b4e8497825aff2890'],
  ]) {
    const packages = metadata.packages.filter(pkg => pkg.name === name)
    if (packages.length !== 1 || !packages[0].source?.endsWith(`#${rev}`)) {
      throw new Error(`${name}: corrected DNS dependency did not propagate`)
    }
  }
  cargo(['check', '--locked', '--target-dir', join(root, 'target')])
  process.stdout.write('Independent Engine Git consumer passed without patches\n')
} finally {
  rmSync(consumer, { recursive: true, force: true })
}
