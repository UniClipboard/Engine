#!/usr/bin/env node

import { execFileSync, spawnSync } from 'node:child_process'
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = realpathSync(resolve(SCRIPT_DIR, '../..'))

const EXPECTED_PACKAGES = [
  'openmls-validation',
  'uc-application',
  'uc-content-hash',
  'uc-core',
  'uc-engine',
  'uc-engine-uniffi',
  'uc-infra',
  'uc-mobile',
  'uc-mobile-lan',
  'uc-mobile-proto',
  'uc-mobile-probe-core',
  'uc-observability-contract',
  'uc-ohos-napi',
]

const INTERNAL_PACKAGES = new Set([
  'uc-application',
  'uc-content-hash',
  'uc-core',
  'uc-infra',
  'uc-mobile',
  'uc-mobile-lan',
  'uc-mobile-proto',
  'uc-observability-contract',
])

const BINDING_PACKAGES = ['uc-engine-uniffi', 'uc-ohos-napi']
const P2P_CONSUMERS = [...BINDING_PACKAGES, 'uc-mobile-probe-core']
const DESKTOP_OWNED_PACKAGES = new Set([
  'uc-app-paths',
  'uc-bootstrap',
  'uc-cli',
  'uc-daemon',
  'uc-daemon-client',
  'uc-daemon-contract',
  'uc-daemon-local',
  'uc-daemon-process',
  'uc-desktop',
  'uc-observability',
  'uc-platform',
  'uc-tauri',
  'uc-webserver',
])

function read(relativePath) {
  return readFileSync(join(REPOSITORY_ROOT, relativePath), 'utf8')
}

function readSourceTree(relativeRoot) {
  const root = join(REPOSITORY_ROOT, relativeRoot)
  const sources = []
  const pending = [root]
  while (pending.length > 0) {
    const current = pending.pop()
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name)
      if (entry.isDirectory()) pending.push(path)
      else if (/\.(rs|toml|ets|ts|java|swift)$/.test(entry.name)) {
        sources.push(readFileSync(path, 'utf8'))
      }
    }
  }
  return sources.join('\n')
}

function parseJson(input, source) {
  try {
    return JSON.parse(input)
  } catch (error) {
    throw new Error(`invalid JSON from ${source}`, { cause: error })
  }
}

function cargoMetadata() {
  const output = execFileSync(
    'cargo',
    ['metadata', '--no-deps', '--format-version', '1', '--locked'],
    { cwd: REPOSITORY_ROOT, encoding: 'utf8' }
  )
  return parseJson(output, 'cargo metadata')
}

function packageByName(metadata, name) {
  const found = metadata.packages.find(candidate => candidate.name === name)
  if (!found) throw new Error(`workspace package is missing: ${name}`)
  return found
}

function normalDependencies(packageMetadata) {
  return packageMetadata.dependencies.filter(dependency => dependency.kind === null)
}

function normalDependency(packageMetadata, dependencyName) {
  return normalDependencies(packageMetadata).find(dependency => dependency.name === dependencyName)
}

function featureItems(packageMetadata, featureName) {
  return packageMetadata.features[featureName] ?? []
}

function addProblem(problems, check, message) {
  problems.push(`${check}: ${message}`)
}

function pathIsInsideRepository(path) {
  const resolved = resolve(path)
  const offset = relative(REPOSITORY_ROOT, resolved)
  return offset !== '..' && !offset.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && !isAbsolute(offset)
}

function checkWorkspaceShape(metadata) {
  const problems = []
  const actual = metadata.workspace_members
    .map(id => metadata.packages.find(candidate => candidate.id === id)?.name)
    .filter(Boolean)
    .sort()
  const expected = [...EXPECTED_PACKAGES].sort()
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    addProblem(problems, 'workspace shape', `expected ${expected.join(', ')}; found ${actual.join(', ')}`)
  }
  for (const forbidden of ['apps', 'src-tauri']) {
    if (existsSync(join(REPOSITORY_ROOT, forbidden))) {
      addProblem(problems, 'workspace shape', `desktop-owned directory is present: ${forbidden}`)
    }
  }
  return problems
}

function checkOpenMlsValidation(metadata) {
  const problems = []
  const validation = packageByName(metadata, 'openmls-validation')
  const executableTarget = validation.targets.find(
    target => target.name === 'revocation' && target.kind.includes('test')
  )
  if (!executableTarget) {
    addProblem(
      problems,
      'OpenMLS validation',
      'openmls-validation must contain the executable revocation test target'
    )
  }
  return problems
}

function runOpenMlsValidation() {
  const result = spawnSync(
    'cargo',
    ['test', '-p', 'openmls-validation', '--test', 'revocation', '--locked'],
    { cwd: REPOSITORY_ROOT, encoding: 'utf8' }
  )
  const output = `${result.stdout ?? ''}${result.stderr ?? ''}`
  const passed = output.match(/test result: ok\. ([1-9]\d*) passed/)
  if (result.status !== 0 || !passed) {
    process.stderr.write(output)
    throw new Error('OpenMLS executable validation target did not pass')
  }
  process.stdout.write(`OK OpenMLS executable validation passed: ${passed[1]} tests\n`)
}

function checkLocalDependencies(metadata) {
  const problems = []
  for (const packageMetadata of metadata.packages) {
    for (const dependency of packageMetadata.dependencies) {
      if (DESKTOP_OWNED_PACKAGES.has(dependency.name)) {
        addProblem(
          problems,
          'dependency firewall',
          `${packageMetadata.name} depends on desktop-owned package ${dependency.name}`
        )
      }
      if (!dependency.path) continue
      if (!pathIsInsideRepository(dependency.path)) {
        addProblem(
          problems,
          'dependency firewall',
          `${packageMetadata.name} has a repository-external local dependency: ${dependency.name}`
        )
      } else if (!EXPECTED_PACKAGES.includes(dependency.name)) {
        addProblem(
          problems,
          'dependency firewall',
          `${packageMetadata.name} has a local dependency outside the owned package set: ${dependency.name}`
        )
      }
    }
  }
  return problems
}

function checkPublicSurface(metadata, sources) {
  const problems = []
  for (const packageMetadata of metadata.packages) {
    if (!Array.isArray(packageMetadata.publish) || packageMetadata.publish.length !== 0) {
      addProblem(problems, 'public surface', `${packageMetadata.name} must set publish = false`)
    }
  }

  for (const bindingName of BINDING_PACKAGES) {
    const localDependencies = normalDependencies(packageByName(metadata, bindingName))
      .filter(dependency => dependency.path)
      .map(dependency => dependency.name)
      .sort()
    if (JSON.stringify(localDependencies) !== JSON.stringify(['uc-engine'])) {
      addProblem(
        problems,
        'public surface',
        `${bindingName} local dependencies must be exactly uc-engine; found ${localDependencies.join(', ')}`
      )
    }
  }

  for (const packageMetadata of metadata.packages) {
    if (packageMetadata.name === 'uc-engine' || BINDING_PACKAGES.includes(packageMetadata.name)) {
      continue
    }
    for (const dependency of normalDependencies(packageMetadata)) {
      if (packageMetadata.name === 'uc-mobile-probe-core' && INTERNAL_PACKAGES.has(dependency.name)) {
        addProblem(
          problems,
          'public surface',
          `acceptance host directly depends on internal package ${dependency.name}`
        )
      }
    }
  }

  for (const token of ['pub use engine::{Engine, EventStream};', 'pub use contract::*;']) {
    if (!sources.engine.includes(token)) {
      addProblem(problems, 'public surface', `uc-engine stable interface is missing ${token}`)
    }
  }
  return problems
}

function checkBindingProvenance(metadata, sources) {
  const problems = []
  const engineVersion = packageByName(metadata, 'uc-engine').version
  for (const bindingName of BINDING_PACKAGES) {
    const bindingVersion = packageByName(metadata, bindingName).version
    if (bindingVersion !== engineVersion) {
      addProblem(
        problems,
        'binding provenance',
        `${bindingName} version ${bindingVersion} differs from uc-engine ${engineVersion}`
      )
    }
  }

  for (const [name, source] of [['UniFFI', sources.uniffi], ['HarmonyOS', sources.ohos]]) {
    if (!source.includes('format!("v{}", env!("CARGO_PKG_VERSION"))')) {
      addProblem(problems, 'binding provenance', `${name} version is not derived from Cargo`)
    }
  }

  const requiredTokens = ['version.txt', 'source-commit.txt', 'rev-parse HEAD', 'checksum']
  for (const [name, script] of [
    ['iOS', sources.iosPackaging],
    ['Android', sources.androidPackaging],
    ['HarmonyOS', sources.ohosPackaging],
  ]) {
    for (const token of requiredTokens) {
      if (!script.includes(token)) {
        addProblem(problems, 'binding provenance', `${name} packaging is missing ${token}`)
      }
    }
  }
  for (const token of ['assembleHar', 'UniClipboardEngine.har']) {
    if (!sources.ohosPackaging.includes(token)) {
      addProblem(problems, 'binding provenance', `HarmonyOS packaging is missing ${token}`)
    }
  }
  return problems
}

function checkLanIsolation(metadata, sources) {
  const problems = []
  const engine = packageByName(metadata, 'uc-engine')
  const application = packageByName(metadata, 'uc-application')
  const infra = packageByName(metadata, 'uc-infra')

  for (const packageMetadata of [engine, application, infra]) {
    if (featureItems(packageMetadata, 'default').includes('lan-compat')) {
      addProblem(problems, 'compatibility gate', `${packageMetadata.name} enables lan-compat by default`)
    }
  }
  for (const required of [
    'dep:uc-mobile-lan',
    'dep:uc-mobile-proto',
    'uc-infra/lan-compat',
  ]) {
    if (!featureItems(engine, 'lan-compat').includes(required)) {
      addProblem(problems, 'compatibility gate', `uc-engine/lan-compat is missing ${required}`)
    }
  }
  if (normalDependency(application, 'uc-mobile-proto')) {
    addProblem(problems, 'compatibility gate', 'uc-application must not depend on uc-mobile-proto (moved to uc-mobile-lan)')
  }
  if (!normalDependency(infra, 'network-interface')?.optional) {
    addProblem(problems, 'compatibility gate', 'uc-infra must keep network-interface optional')
  }
  for (const consumerName of P2P_CONSUMERS) {
    const dependency = normalDependency(packageByName(metadata, consumerName), 'uc-engine')
    if (dependency?.features.includes('lan-compat')) {
      addProblem(problems, 'compatibility gate', `${consumerName} enables uc-engine/lan-compat`)
    }
  }
  if (/fallback_to_lan|auto(?:matic)?_lan_fallback|p2p_failed_to_lan/i.test(sources.runtime)) {
    addProblem(problems, 'compatibility gate', 'source contains an automatic P2P-to-LAN fallback')
  }
  return problems
}

function checkPlaintextScanner() {
  const problems = []
  const work = mkdtempSync(join(tmpdir(), 'uc-engine-repository-check-'))
  const probe = join(work, 'probe.txt')
  const root = join(work, 'storage')
  const scanner = join(REPOSITORY_ROOT, 'scripts/security/scan-plaintext-probe.sh')
  const value = 'cross-repository-plaintext-probe-20260724'
  try {
    mkdirSync(root)
    writeFileSync(probe, value)
    writeFileSync(join(root, 'encrypted.db'), 'ciphertext-only')
    const clean = spawnSync('bash', [scanner, probe, root], { encoding: 'utf8' })
    if (clean.status !== 0) {
      addProblem(problems, 'persistence gate', 'plaintext scanner rejected a clean fixture')
    }

    writeFileSync(join(root, 'leak.db'), value)
    const leaking = spawnSync('bash', [scanner, probe, root], { encoding: 'utf8' })
    if (leaking.status === 0) {
      addProblem(problems, 'persistence gate', 'plaintext scanner accepted a leaking fixture')
    }
    if (`${leaking.stdout}${leaking.stderr}`.includes(value)) {
      addProblem(problems, 'persistence gate', 'plaintext scanner printed the probe value')
    }
  } finally {
    rmSync(work, { recursive: true, force: true })
  }

  const rules = read('AGENTS.md')
  if (!rules.includes('文件内容本体') || !rules.includes('严禁明文落库')) {
    addProblem(problems, 'persistence gate', 'repository rules weakened encrypted persistence')
  }
  return problems
}

function checkCurrentPeerScopeOwnership() {
  const problems = []
  const scopedConsumers = [
    'crates/uc-application/src/facade/roster/facade.rs',
    'crates/uc-application/src/space/convergence/reachability.rs',
    'crates/uc-application/src/space/convergence/membership_connectivity.rs',
    'crates/uc-application/src/space/convergence/legacy_upgrade.rs',
    'crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs',
    'crates/uc-application/src/clipboard/sync/active_state/fanout.rs',
    'crates/uc-application/src/clipboard/sync/resend_entry.rs',
  ]
  for (const path of scopedConsumers) {
    const source = read(path)
    if (
      !source.includes('CurrentWorkspacePeerScopePort') ||
      !/\.snapshot\(\)\s*\.await/.test(source)
    ) {
      addProblem(
        problems,
        'current peer scope',
        `${path} must consume one complete CurrentWorkspacePeerScopePort snapshot`
      )
    }
  }
  const corePorts = read('crates/uc-core/src/membership/ports.rs')
  if (corePorts.includes('DeviceVisibilityGatePort')) {
    addProblem(problems, 'current peer scope', 'the superseded device visibility gate was restored')
  }
  return problems
}

function repositorySources() {
  return {
    engine: read('crates/uc-engine/src/lib.rs'),
    uniffi: read('bindings/uc-engine-uniffi/src/lib.rs'),
    ohos: read('bindings/uc-ohos-napi/src/lib.rs'),
    iosPackaging: read('bindings/uc-engine-uniffi/scripts/build-ios-xcframework.sh'),
    androidPackaging: read('bindings/uc-engine-uniffi/scripts/build-android-aar.sh'),
    ohosPackaging: read('tests/hosts/ohos/build-emulator.sh'),
    runtime: [
      readSourceTree('crates/uc-engine'),
      readSourceTree('bindings'),
      readSourceTree('compatibility'),
    ].join('\n'),
  }
}

function collectProblems(metadata, sources, { includePlaintext = true } = {}) {
  return [
    ...checkWorkspaceShape(metadata),
    ...checkOpenMlsValidation(metadata),
    ...checkLocalDependencies(metadata),
    ...checkPublicSurface(metadata, sources),
    ...checkBindingProvenance(metadata, sources),
    ...checkLanIsolation(metadata, sources),
    ...checkCurrentPeerScopeOwnership(),
    ...(includePlaintext ? checkPlaintextScanner() : []),
  ]
}

function clone(value) {
  return structuredClone(value)
}

function expectRejected(name, mutate, metadata, sources) {
  const changedMetadata = clone(metadata)
  const changedSources = { ...sources }
  mutate(changedMetadata, changedSources)
  const problems = collectProblems(changedMetadata, changedSources, { includePlaintext: false })
  if (problems.length === 0) throw new Error(`negative fixture was not rejected: ${name}`)
  process.stdout.write(`OK negative fixture rejected: ${name}\n`)
}

function runNegativeFixtures(metadata, sources) {
  expectRejected('repository-external local dependency', changed => {
    packageByName(changed, 'uc-engine').dependencies.push({
      name: 'uc-platform',
      kind: null,
      path: join(tmpdir(), 'uc-platform'),
      features: [],
      optional: false,
    })
  }, metadata, sources)
  expectRejected('binding version mismatch', changed => {
    packageByName(changed, 'uc-ohos-napi').version = '999.0.0'
  }, metadata, sources)
  expectRejected('automatic LAN fallback', (_changed, changedSources) => {
    changedSources.runtime += '\nfn fallback_to_lan() {}\n'
  }, metadata, sources)
  expectRejected('missing OpenMLS validation target', changed => {
    const validation = packageByName(changed, 'openmls-validation')
    validation.targets = validation.targets.filter(target => !target.kind.includes('test'))
  }, metadata, sources)
}

function main() {
  if (realpathSync(process.cwd()) !== REPOSITORY_ROOT) {
    throw new Error(`run from repository root: ${REPOSITORY_ROOT}`)
  }
  const metadata = cargoMetadata()
  const sources = repositorySources()
  const problems = collectProblems(metadata, sources)
  if (problems.length > 0) {
    for (const problem of problems) process.stderr.write(`ERROR ${problem}\n`)
    process.exitCode = 1
    return
  }
  runOpenMlsValidation()
  runNegativeFixtures(metadata, sources)
  process.stdout.write('Engine repository preflight passed\n')
}

main()
