#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { basename, join, relative, resolve } from 'node:path'
import process from 'node:process'

const [releaseDirectoryArg, deviceMatrixArg] = process.argv.slice(2)
if (!releaseDirectoryArg || !deviceMatrixArg) {
  throw new Error('usage: build-release-manifest.mjs <release-directory> <device-matrix.json>')
}

const repositoryRoot = process.cwd()
const releaseDirectory = resolve(releaseDirectoryArg)
const deviceMatrix = JSON.parse(readFileSync(resolve(deviceMatrixArg), 'utf8'))

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function filesUnder(root) {
  const files = []
  const pending = [root]
  while (pending.length > 0) {
    const current = pending.pop()
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name)
      if (entry.isDirectory()) pending.push(path)
      else if (entry.name !== 'release-manifest.json') files.push(path)
    }
  }
  return files.sort()
}

function platformFor(name) {
  if (/xcframework|uniffi\.swift/i.test(name)) return 'ios'
  if (/\.aar$|\.pom$|uniffi\.kt|runtime-dependencies/i.test(name)) return 'android'
  if (/\.har|\.hap|ohos|index\.d\.ts/i.test(name)) return 'harmonyos'
  if (/debug-symbols/i.test(name)) return 'multi-platform'
  return 'source'
}

function architecturesFor(name) {
  if (/xcframework/i.test(name)) return ['arm64-device', 'arm64-simulator', 'x86_64-simulator']
  if (/\.aar$/i.test(name)) return ['arm64-v8a', 'x86_64']
  if (/\.har|\.hap|ohos/i.test(name)) return ['arm64-v8a']
  return []
}

for (const [platform, record] of Object.entries(deviceMatrix)) {
  if (!['passed', 'failed', 'skipped'].includes(record.status)) {
    throw new Error(`device matrix ${platform} has invalid status ${record.status}`)
  }
  if (record.status === 'skipped' && !record.reason) {
    throw new Error(`device matrix ${platform} must explain why it was skipped`)
  }
}

const version = readFileSync(join(releaseDirectory, 'core-version.txt'), 'utf8').trim()
const commit = readFileSync(join(releaseDirectory, 'source-commit.txt'), 'utf8').trim()
const lockPath = join(releaseDirectory, 'Cargo.lock')
const rustToolchain = readFileSync(join(repositoryRoot, 'rust-toolchain.toml'), 'utf8')
  .match(/channel\s*=\s*"([^"]+)"/)?.[1]
const migrations = readdirSync(join(repositoryRoot, 'crates/uc-infra/migrations'))
  .filter(name => /^\d/.test(name))
  .sort()
const metadata = JSON.parse(
  execFileSync('cargo', ['metadata', '--locked', '--format-version', '1'], {
    cwd: repositoryRoot,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  })
)
const packageVersion = name => metadata.packages.find(candidate => candidate.name === name)?.version

const artifacts = filesUnder(releaseDirectory).map(path => {
  const name = relative(releaseDirectory, path)
  return {
    name,
    platform: platformFor(name),
    architectures: architecturesFor(name),
    sha256: sha256(path),
    size: statSync(path).size,
  }
})

const manifest = {
  schemaVersion: 1,
  release: {
    version,
    commit,
    rustToolchain,
    cargoLockSha256: sha256(lockPath),
  },
  generators: {
    uniffi: packageVersion('uniffi'),
    napi: packageVersion('napi'),
    napiBuild: packageVersion('napi-build'),
    napiDerive: packageVersion('napi-derive'),
    kotlin: '2.1.20',
    swift: `UniFFI ${packageVersion('uniffi')}`,
    arkts: `napi-rs ${packageVersion('napi')}`,
  },
  compatibility: {
    p2pProtocols: [
      'pairing/1',
      'presence/0',
      'clipboard/0',
      'active-clipboard/0',
      'active-clipboard-pull/0',
      'transfer-progress/0',
      'iroh-blobs',
    ],
    database: 'Diesel SQLite embedded migrations',
    latestMigration: migrations.at(-1),
    minimumSystems: {
      ios: '16.4',
      androidApi: 24,
      harmonyosApi: 24,
    },
  },
  deviceMatrix,
  artifacts,
}

writeFileSync(
  join(releaseDirectory, 'release-manifest.json'),
  `${JSON.stringify(manifest, null, 2)}\n`
)
process.stdout.write(`Wrote ${basename(releaseDirectory)}/release-manifest.json with ${artifacts.length} artifacts\n`)
