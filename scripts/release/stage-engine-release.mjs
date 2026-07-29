#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'

const [distArg, deviceMatrixArg] = process.argv.slice(2)
if (!distArg || !deviceMatrixArg) {
  throw new Error('usage: stage-engine-release.mjs <platform-dist-root> <device-matrix.json>')
}

const repositoryRoot = process.cwd()

function parseJson(input, source) {
  try {
    return JSON.parse(input)
  } catch (error) {
    throw new Error(`invalid JSON from ${source}`, { cause: error })
  }
}

const distRoot = resolve(distArg)
const releaseDirectory = join(distRoot, 'release-assets')
const platformDirectories = ['ios', 'android', 'ohos']

const provenance = platformDirectories.map(platform => ({
  platform,
  version: readFileSync(join(distRoot, platform, 'version.txt'), 'utf8').trim(),
  commit: readFileSync(join(distRoot, platform, 'source-commit.txt'), 'utf8').trim(),
}))
if (new Set(provenance.map(item => item.version)).size !== 1) {
  throw new Error('platform artifacts do not share one Engine version')
}
if (new Set(provenance.map(item => item.commit)).size !== 1) {
  throw new Error('platform artifacts do not share one source commit')
}

rmSync(releaseDirectory, { recursive: true, force: true })
mkdirSync(releaseDirectory, { recursive: true })

const copies = [
  ['ios/UniClipboardEngine.xcframework.zip', 'UniClipboardEngine.xcframework.zip'],
  ['ios/UniClipboardEngine.checksum.txt', 'UniClipboardEngine.xcframework.checksum.txt'],
  ['ios/uc_engine_uniffi.swift', 'uc_engine_uniffi.swift'],
  ['android/UniClipboardEngine.aar', 'UniClipboardEngine.aar'],
  ['android/UniClipboardEngine.checksum.txt', 'UniClipboardEngine.aar.checksum.txt'],
  ['android/UniClipboardEngine.pom', 'UniClipboardEngine.pom'],
  ['android/runtime-dependencies.txt', 'runtime-dependencies.txt'],
  ['android/uc_engine_uniffi.kt', 'uc_engine_uniffi.kt'],
  ['ohos/UniClipboardEngine.har', 'UniClipboardEngine.har'],
  ['ohos/UniClipboardEngine.har.checksum.txt', 'UniClipboardEngine.har.checksum.txt'],
  ['ohos/UniClipboardEngineProbe.hap', 'UniClipboardEngineProbe.hap'],
  ['ohos/UniClipboardEngineProbe.checksum.txt', 'UniClipboardEngineProbe.checksum.txt'],
  ['ohos/libuc_ohos_napi.so', 'libuc_ohos_napi.so'],
  ['ohos/uc-ohos-napi.checksum.txt', 'uc-ohos-napi.checksum.txt'],
  ['ohos/index.d.ts', 'index.d.ts'],
]
for (const [source, destination] of copies) {
  const sourcePath = join(distRoot, source)
  if (!existsSync(sourcePath)) throw new Error(`platform artifact is missing: ${source}`)
  cpSync(sourcePath, join(releaseDirectory, destination))
}

const version = provenance[0].version
const commit = provenance[0].commit
const head = execFileSync('git', ['rev-parse', 'HEAD'], {
  cwd: repositoryRoot,
  encoding: 'utf8',
}).trim()
if (head !== commit) {
  throw new Error(`platform source commit ${commit} does not match checked-out commit ${head}`)
}
const trackedChanges = execFileSync('git', ['status', '--porcelain', '--untracked-files=no'], {
  cwd: repositoryRoot,
  encoding: 'utf8',
}).trim()
if (trackedChanges) {
  throw new Error('tracked files must be clean before staging release source assets')
}
writeFileSync(join(releaseDirectory, 'version.txt'), `${version}\n`)
writeFileSync(join(releaseDirectory, 'source-commit.txt'), `${commit}\n`)
cpSync(join(repositoryRoot, 'Cargo.lock'), join(releaseDirectory, 'Cargo.lock'))
cpSync(join(repositoryRoot, 'LICENSE'), join(releaseDirectory, 'LICENSE'))

execFileSync(
  'git',
  [
    'archive',
    '--format=tar.gz',
    `--prefix=UniClipboardEngine-${version.replace(/^v/, '')}/`,
    `--output=${join(releaseDirectory, 'UniClipboardEngine-source.tar.gz')}`,
    commit,
  ],
  { cwd: repositoryRoot, stdio: 'inherit' }
)

const metadata = parseJson(
  execFileSync('cargo', ['metadata', '--locked', '--format-version', '1'], {
    cwd: repositoryRoot,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  }),
  'cargo metadata'
)
const engineVersion = metadata.packages.find(candidate => candidate.name === 'uc-engine')?.version
if (version !== `v${engineVersion}`) {
  throw new Error(`platform version ${version} does not match uc-engine v${engineVersion}`)
}
const licenses = metadata.packages
  .map(packageMetadata => ({
    name: packageMetadata.name,
    version: packageMetadata.version,
    license: packageMetadata.license ?? 'UNKNOWN',
    source: packageMetadata.source ?? 'workspace',
  }))
  .sort((left, right) => `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`))
writeFileSync(
  join(releaseDirectory, 'dependency-licenses.json'),
  `${JSON.stringify({ schemaVersion: 1, packages: licenses }, null, 2)}\n`
)

const debugDirectory = join(distRoot, 'debug-symbols')
if (!existsSync(debugDirectory)) throw new Error('debug symbols are missing')
execFileSync(
  'tar',
  ['-czf', join(releaseDirectory, 'debug-symbols.tar.gz'), '-C', dirname(debugDirectory), 'debug-symbols'],
  { stdio: 'inherit' }
)

execFileSync(
  process.execPath,
  [join(repositoryRoot, 'scripts/release/build-release-manifest.mjs'), releaseDirectory, resolve(deviceMatrixArg)],
  { cwd: repositoryRoot, stdio: 'inherit' }
)
execFileSync(
  process.execPath,
  [join(repositoryRoot, 'scripts/release/verify-release-bundle.mjs'), releaseDirectory],
  { cwd: repositoryRoot, stdio: 'inherit' }
)

process.stdout.write(`${releaseDirectory}\n`)
