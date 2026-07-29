#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'
import process from 'node:process'

const [releaseDirectoryArg] = process.argv.slice(2)
if (!releaseDirectoryArg) throw new Error('usage: verify-release-bundle.mjs <release-directory>')
const releaseDirectory = resolve(releaseDirectoryArg)

function parseJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch (error) {
    throw new Error(`invalid JSON file: ${path}`, { cause: error })
  }
}

const manifest = parseJson(join(releaseDirectory, 'release-manifest.json'))

const required = [
  'Cargo.lock',
  'LICENSE',
  'UniClipboardEngine-source.tar.gz',
  'dependency-licenses.json',
  'debug-symbols.tar.gz',
  'UniClipboardEngine.xcframework.zip',
  'UniClipboardEngine.xcframework.checksum.txt',
  'uc_engine_uniffi.swift',
  'UniClipboardEngine.aar',
  'UniClipboardEngine.aar.checksum.txt',
  'uc_engine_uniffi.kt',
  'UniClipboardEngine.pom',
  'runtime-dependencies.txt',
  'UniClipboardEngine.har',
  'UniClipboardEngine.har.checksum.txt',
  'UniClipboardEngineProbe.hap',
  'UniClipboardEngineProbe.checksum.txt',
  'libuc_ohos_napi.so',
  'uc-ohos-napi.checksum.txt',
  'index.d.ts',
  'version.txt',
  'source-commit.txt',
]

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

function listFiles(root) {
  const files = []
  const pending = [root]
  while (pending.length > 0) {
    const current = pending.pop()
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name)
      if (entry.isDirectory()) pending.push(path)
      else if (entry.name !== 'release-manifest.json') files.push(relative(root, path))
    }
  }
  return files.sort()
}

if (!/^v\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(manifest.release.version)) {
  throw new Error(`invalid Engine version: ${manifest.release.version}`)
}
if (!/^[0-9a-f]{40}$/.test(manifest.release.commit)) {
  throw new Error(`invalid source commit: ${manifest.release.commit}`)
}
for (const name of required) {
  if (!listFiles(releaseDirectory).includes(name)) throw new Error(`release asset is missing: ${name}`)
}

const declared = manifest.artifacts.map(artifact => artifact.name).sort()
const actual = listFiles(releaseDirectory)
if (JSON.stringify(declared) !== JSON.stringify(actual)) {
  throw new Error('release manifest file list does not match the release directory')
}

for (const artifact of manifest.artifacts) {
  const path = join(releaseDirectory, artifact.name)
  if (statSync(path).size !== artifact.size) throw new Error(`size mismatch: ${artifact.name}`)
  if (sha256(path) !== artifact.sha256) throw new Error(`sha256 mismatch: ${artifact.name}`)
}
if (sha256(join(releaseDirectory, 'Cargo.lock')) !== manifest.release.cargoLockSha256) {
  throw new Error('Cargo.lock checksum does not match the release record')
}
if (readFileSync(join(releaseDirectory, 'version.txt'), 'utf8').trim() !== manifest.release.version) {
  throw new Error('version.txt does not match the release record')
}
if (readFileSync(join(releaseDirectory, 'source-commit.txt'), 'utf8').trim() !== manifest.release.commit) {
  throw new Error('source-commit.txt does not match the release record')
}
for (const [platform, record] of Object.entries(manifest.deviceMatrix)) {
  if (!['passed', 'failed', 'skipped'].includes(record.status)) {
    throw new Error(`invalid device status for ${platform}: ${record.status}`)
  }
  if (record.status === 'skipped' && !record.reason) {
    throw new Error(`skipped device status must include a reason: ${platform}`)
  }
}

const debugListing = execFileSync(
  'tar',
  ['-tzf', join(releaseDirectory, 'debug-symbols.tar.gz')],
  { encoding: 'utf8' }
)
for (const platform of ['ios/', 'android/', 'ohos/']) {
  if (!debugListing.includes(`debug-symbols/${platform}`)) {
    throw new Error(`debug archive is missing ${platform}`)
  }
}

process.stdout.write(`Verified ${manifest.artifacts.length} release artifacts\n`)
