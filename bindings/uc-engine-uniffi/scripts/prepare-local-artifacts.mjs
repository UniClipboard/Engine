#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import process from 'node:process'

const repositoryRoot = process.cwd()
const bindingScripts = join(
  repositoryRoot,
  'bindings/uc-engine-uniffi/scripts'
)
const targetDir = resolve(
  process.env.UC_ENGINE_UNIFFI_TARGET_DIR ??
    process.env.CARGO_TARGET_DIR ??
    join(repositoryRoot, 'target')
)
const localRoot = join(repositoryRoot, '.artifacts/local')
const stagingDistRoot = join(targetDir, 'uc-engine-uniffi-local-dist')
const platforms = ['ios', 'ios-sim', 'android']

function runWith(slice, distRoot) {
  execFileSync(
    '/usr/bin/env',
    ['bash', join(bindingScripts, 'build-ios-xcframework.sh')],
    {
      cwd: repositoryRoot,
      stdio: 'inherit',
      env: {
        ...process.env,
        UC_ENGINE_UNIFFI_SLICE: slice,
        UC_ENGINE_UNIFFI_DIST_DIR: distRoot,
      },
    }
  )
}

function sha256Of(file) {
  return createHash('sha256').update(readFileSync(file)).digest('hex')
}

function copyPlatformDist(platform, sourceRoot, distSubdirectory) {
  const source = join(sourceRoot, distSubdirectory)
  if (!existsSync(source)) {
    throw new Error(`platform artifacts are missing: ${source}`)
  }
  const destination = join(localRoot, platform)
  mkdirSync(destination, { recursive: true })
  cpSync(source, destination, { recursive: true })
}

const iosDeviceDist = join(stagingDistRoot, 'device')
const iosSimulatorDist = join(stagingDistRoot, 'simulator')
const androidDist = join(stagingDistRoot, 'android')

console.log('==> Build iOS device XCFramework')
runWith('device', iosDeviceDist)
console.log('==> Build iOS simulator XCFramework')
runWith('simulator', iosSimulatorDist)

console.log('==> Build Android AAR')
execFileSync(
  '/usr/bin/env',
  ['bash', join(bindingScripts, 'build-android-aar.sh')],
  {
    cwd: repositoryRoot,
    stdio: 'inherit',
    env: { ...process.env, UC_ENGINE_UNIFFI_DIST_DIR: androidDist },
  }
)

rmSync(localRoot, { recursive: true, force: true })
mkdirSync(localRoot, { recursive: true })
copyPlatformDist('ios', iosDeviceDist, 'ios')
copyPlatformDist('ios-sim', iosSimulatorDist, 'ios')
copyPlatformDist('android', androidDist, 'android')

const provenance = platforms.map(platform => ({
  platform,
  version: readFileSync(join(localRoot, platform, 'version.txt'), 'utf8').trim(),
  commit: readFileSync(join(localRoot, platform, 'source-commit.txt'), 'utf8').trim(),
}))
if (new Set(provenance.map(item => item.version)).size !== 1) {
  throw new Error('platform artifacts do not share one Engine version')
}
if (new Set(provenance.map(item => item.commit)).size !== 1) {
  throw new Error('platform artifacts do not share one source commit')
}

const primaryArtifacts = [
  { platform: 'ios', file: 'ios/UniClipboardEngine.xcframework.zip' },
  { platform: 'ios-sim', file: 'ios-sim/UniClipboardEngine.xcframework.zip' },
  { platform: 'android', file: 'android/UniClipboardEngine.aar' },
]
const artifacts = primaryArtifacts.map(artifact => {
  const path = join(localRoot, artifact.file)
  if (!existsSync(path)) throw new Error(`local artifact is missing: ${path}`)
  return { platform: artifact.platform, file: artifact.file, sha256: sha256Of(path) }
})

const prepared = {
  schemaVersion: 1,
  engineVersion: provenance[0].version,
  sourceCommit: provenance[0].commit,
  preparedAt: new Date().toISOString(),
  artifacts,
}
writeFileSync(
  join(localRoot, 'local-prepared.json'),
  `${JSON.stringify(prepared, null, 2)}\n`
)

process.stdout.write(`Local artifacts prepared at ${localRoot}\n`)
