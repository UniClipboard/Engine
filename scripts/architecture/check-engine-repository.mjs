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

function rustSourcesUnder(relativeRoot) {
  const root = join(REPOSITORY_ROOT, relativeRoot)
  const sources = []
  const pending = [root]
  while (pending.length > 0) {
    const directory = pending.pop()
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) pending.push(path)
      else if (entry.name.endsWith('.rs')) {
        sources.push({
          path: relative(REPOSITORY_ROOT, path),
          source: readFileSync(path, 'utf8'),
        })
      }
    }
  }
  return sources
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

function checkApplicationDependencyInventory(sources) {
  const problems = []
  for (const field of [
    'pub current_profile:',
    'pub blob_cipher:',
    'pub portable_current_space_identity:',
  ]) {
    if (sources.applicationDeps.includes(field)) {
      addProblem(
        problems,
        'application dependency inventory',
        `retired wiring-only field remains in Application dependency bundles: ${field}`
      )
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

function checkProfileStorageGenerationOwnership(sources) {
  const problems = []
  const v3TransitionPath =
    'crates/uc-infra/src/security/v3_admission_space_transition.rs'
  const forbiddenV3Dependencies = [
    'ProfileStorageUpgrade',
    'profile_storage_upgrade',
    'LegacyPayloadProtection',
    'BlobCipherAdapter',
    'EncryptedBlobStore',
    'source_cipher',
    'target_cipher',
    'payload_rewrap',
    'rewrap_finalized_source',
    'ProfileContentKeyVault',
    'ClipboardDispatch',
    'EntryDelivery',
    'Outbox',
  ]
  for (const marker of forbiddenV3Dependencies) {
    if (sources.v3AdmissionTransition.includes(marker)) {
      addProblem(
        problems,
        'profile storage generation ownership',
        `${v3TransitionPath} depends on forbidden payload-upgrade knowledge: ${marker}`
      )
    }
  }

  const forbiddenCoordinatorDetails = [
    'UpgradePhaseV1',
    'TargetGenerationStager',
    'PrimaryPayloadConverter',
    'DerivedPayloadConverter',
    'profile_storage_upgrade::',
  ]
  for (const [owner, source] of [
    ['uc-application', sources.application],
    ['uc-engine', sources.engineRuntime],
  ]) {
    for (const marker of forbiddenCoordinatorDetails) {
      if (source.includes(marker)) {
        addProblem(
          problems,
          'profile storage generation ownership',
          `${owner} combines private profile-upgrade steps through ${marker}`
        )
      }
    }
  }

  if (!sources.engineWiring.includes('.ensure_v3()')) {
    addProblem(
      problems,
      'profile storage generation ownership',
      'uc-engine must enter profile storage upgrade through the complete ensure_v3 operation'
    )
  }
  for (const marker of ['ProfileContentKeyVault', 'profile_content_key_vault']) {
    if (sources.network.includes(marker)) {
      addProblem(
        problems,
        'profile storage generation ownership',
        `network adapters depend on historical profile vault authority through ${marker}`
      )
    }
  }
  return problems
}

function checkCurrentPeerScopeOwnership() {
  const problems = []
  const scopedConsumers = [
    'crates/uc-application/src/facade/roster/facade.rs',
    'crates/uc-application/src/clipboard/sync/dispatch_entry/target_selector.rs',
    'crates/uc-application/src/clipboard/sync/active_state/fanout.rs',
    'crates/uc-application/src/clipboard/sync/resend_entry.rs',
  ]
  for (const path of scopedConsumers) {
    const source = read(path)
    if (
      !source.includes('CurrentSpaceMemberScopePort') ||
      !/\.snapshot\(\)\s*\.await/.test(source)
    ) {
      addProblem(
        problems,
        'current peer scope',
        `${path} must consume one complete CurrentSpaceMemberScopePort snapshot`
      )
    }
  }
  const corePorts = read('crates/uc-core/src/membership/ports.rs')
  if (corePorts.includes('DeviceVisibilityGatePort')) {
    addProblem(problems, 'current peer scope', 'the superseded device visibility gate was restored')
  }

  const currentScope = read('crates/uc-application/src/space/membership/ledger/repository.rs')
  if (
    !currentScope.includes('VersionedMembershipHistory::decode_persisted_v2') ||
    !currentScope.includes('impl CurrentSpaceMemberScopePort for MembershipLedger') ||
    currentScope.includes('CurrentWorkspacePeerScopePort') ||
    currentScope.includes('MemberRepositoryPort')
  ) {
    addProblem(
      problems,
      'current peer scope',
      'missing V2 membership history must fail closed without a legacy membership fallback'
    )
  }

  const allowedPendingReadmissionConsumers = new Set()
  const applicationRoot = join(REPOSITORY_ROOT, 'crates/uc-application/src')
  const pendingDirectories = [applicationRoot]
  while (pendingDirectories.length > 0) {
    const directory = pendingDirectories.pop()
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) {
        pendingDirectories.push(path)
      } else if (entry.name.endsWith('.rs')) {
        const relativePath = relative(REPOSITORY_ROOT, path)
        if (
          readFileSync(path, 'utf8').includes('pending_readmission_members') &&
          !allowedPendingReadmissionConsumers.has(relativePath)
        ) {
          addProblem(
            problems,
            'current peer scope',
            `${relativePath} must not use legacy readmission candidates for ordinary work`
          )
        }
      }
    }
  }
  return problems
}

function checkMembershipConfirmationWatermarkOwnership(sources) {
  const problems = []
  const positiveAssignment = /\.confirmed_position\s*=\s*(?!None\b)[A-Za-z_]/g
  const applicationAssignments = sources.application.match(positiveAssignment) ?? []
  const authenticatedExchangeOwners = [
    read('crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs'),
    read('crates/uc-application/src/space/membership/handle_history_message/use_case.rs'),
  ].join('\n')
  const ownerAssignments = authenticatedExchangeOwners.match(positiveAssignment) ?? []
  if (applicationAssignments.length !== 2 || ownerAssignments.length !== 2) {
    addProblem(
      problems,
      'membership confirmation watermark',
      'positive confirmed_position assignment must remain exclusive to authenticated ACK/suffix owners'
    )
  }

  const sponsorActivation = read('crates/uc-infra/src/space/admission/sponsor/complete.rs')
  if (
    !sponsorActivation.includes('confirmed_position: None') ||
    /confirmed_position\s*:\s*Some\b/.test(sponsorActivation)
  ) {
    addProblem(
      problems,
      'membership confirmation watermark',
      'Sponsor activation must create propagation debt instead of inferring peer confirmation'
    )
  }
  return problems
}

function checkRetiredLegacyPairingRecovery() {
  const problems = []
  const retiredPaths = [
    'crates/uc-application/src/space/convergence/membership/legacy_upgrade.rs',
    'crates/uc-application/src/space/convergence/membership/legacy_upgrade_tests.rs',
    'crates/uc-core/src/membership/upgrade.rs',
    'crates/uc-core/tests/legacy_upgrade.rs',
    'crates/uc-infra/src/network/iroh/legacy_upgrade_adapter.rs',
  ]
  for (const path of retiredPaths) {
    if (existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'retired legacy pairing recovery', `retired path remains: ${path}`)
    }
  }

  const forbiddenMarkers = [
    'AutomaticLegacyUpgrade',
    'LEGACY_UPGRADE_ALPN',
    'uniclipboard/legacy-upgrade/2',
    'QueryLegacyBootstrap',
  ]
  const sourceRoots = ['crates', 'bindings', 'tests/hosts']
  for (const sourceRoot of sourceRoots) {
    const pendingDirectories = [join(REPOSITORY_ROOT, sourceRoot)]
    while (pendingDirectories.length > 0) {
      const directory = pendingDirectories.pop()
      for (const entry of readdirSync(directory, { withFileTypes: true })) {
        const path = join(directory, entry.name)
        if (entry.isDirectory()) {
          pendingDirectories.push(path)
        } else if (entry.name.endsWith('.rs')) {
          const source = readFileSync(path, 'utf8')
          for (const marker of forbiddenMarkers) {
            if (source.includes(marker)) {
              addProblem(
                problems,
                'retired legacy pairing recovery',
                `${relative(REPOSITORY_ROOT, path)} contains retired marker: ${marker}`
              )
            }
          }
        }
      }
    }
  }

  const eventContract = read('crates/uc-engine/src/contract/event.rs')
  const hasRePairingVariant = /RePairingRequired\s*\{\s*scope:\s*RePairingScope\s*,?\s*\}/s.test(eventContract)
  const hasAllDevicesScope = /enum\s+RePairingScope\s*\{[^}]*\bAllDevices\b[^}]*\}/s.test(eventContract)
  const hasKindMapping = /Self::RePairingRequired\s*\{\s*\.\.\s*\}\s*=>\s*"re_pairing_required"/s.test(eventContract)
  if (!hasRePairingVariant || !hasAllDevicesScope || !hasKindMapping) {
    addProblem(problems, 'retired legacy pairing recovery', 'all-device re-pairing event is missing')
  }
  return problems
}

function checkRetiredPairingTransport(sources) {
  const problems = []
  const retiredPaths = [
    'crates/uc-core/src/pairing/session_message.rs',
    'crates/uc-core/src/ports/pairing/mod.rs',
    'crates/uc-core/src/ports/pairing/events.rs',
    'crates/uc-core/src/ports/pairing/session.rs',
    'crates/uc-infra/src/pairing/session.rs',
    'crates/uc-infra/src/pairing/wire.rs',
  ]
  for (const path of retiredPaths) {
    if (existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'retired pairing transport', `retired path remains: ${path}`)
    }
  }

  for (const marker of [
    'PairingSessionPort',
    'PairingEventPort',
    'PairingSessionMessage',
    'PAIRING_ALPN',
    '/uniclipboard/pairing/1',
    '/uniclipboard/pairing/2',
  ]) {
    if (sources.runtime.includes(marker)) {
      addProblem(problems, 'retired pairing transport', `retired runtime marker remains: ${marker}`)
    }
  }
  return problems
}

function checkApplicationMembershipCutover() {
  const problems = []
  const requiredEntries = [
    'crates/uc-application/src/space/application.rs',
    'crates/uc-application/src/space/facade/mod.rs',
    'crates/uc-application/src/facade/space_setup/mod.rs',
    'crates/uc-application/src/space/admission/mod.rs',
    'crates/uc-application/src/space/admission/protocol/mod.rs',
    'crates/uc-application/src/space/lifecycle/mod.rs',
    'crates/uc-application/src/space/lifecycle/session/mod.rs',
    'crates/uc-application/src/space/membership/mod.rs',
    'crates/uc-application/src/space/membership/ledger/mod.rs',
    'crates/uc-application/src/space/membership/query_device_trust/mod.rs',
    'crates/uc-application/src/space/membership/remove_space_member/mod.rs',
    'crates/uc-application/src/space/membership/decide_device_trust_change/mod.rs',
    'crates/uc-application/src/space/membership/synchronize_history/mod.rs',
    'crates/uc-application/src/space/membership/handle_history_message/mod.rs',
    'crates/uc-application/src/space/membership/maintenance/runtime.rs',
    'crates/uc-application/src/space/connectivity/recovery/mod.rs',
  ]
  for (const path of requiredEntries) {
    if (!existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'application membership cutover', `missing target entry: ${path}`)
    }
  }

  const retiredPaths = [
    'crates/uc-application/src/space/assembly.rs',
    'crates/uc-application/src/space/current_membership_scope.rs',
    'crates/uc-application/src/space/membership_state_coordinator.rs',
    'crates/uc-application/src/space/membership_state_coordinator_tests.rs',
    'crates/uc-application/src/space/membership_state',
    'crates/uc-application/src/space/membership_convergence',
    'crates/uc-application/src/space/membership_runtime',
    'crates/uc-application/src/space/recover_pending_membership_effects',
    'crates/uc-application/src/space/decide_membership_removal_legacy',
    'crates/uc-application/src/space/query_workspace_membership_diagnostics',
    'crates/uc-application/src/space/runtime.rs',
    'crates/uc-application/src/facade/space_join',
    'crates/uc-application/src/facade/space_membership',
    'crates/uc-application/src/facade/space_setup/commands.rs',
    'crates/uc-application/src/facade/space_setup/deps.rs',
    'crates/uc-application/src/facade/space_setup/errors.rs',
    'crates/uc-application/src/facade/space_setup/facade.rs',
    'crates/uc-application/src/space/current_member_signing',
    'crates/uc-application/src/space/current_space',
    'crates/uc-application/src/space/decide_device_trust_change',
    'crates/uc-application/src/space/handle_membership_history_message',
    'crates/uc-application/src/space/initialize_space',
    'crates/uc-application/src/space/lock_space_session',
    'crates/uc-application/src/space/maintain_space_membership',
    'crates/uc-application/src/space/membership_ledger',
    'crates/uc-application/src/space/query_device_trust',
    'crates/uc-application/src/space/query_membership_admission',
    'crates/uc-application/src/space/query_space_access_state',
    'crates/uc-application/src/space/query_space_setup_state',
    'crates/uc-application/src/space/re_pairing',
    'crates/uc-application/src/space/rebuild_space',
    'crates/uc-application/src/space/recover_space_session',
    'crates/uc-application/src/space/remove_space_member',
    'crates/uc-application/src/space/reset_space',
    'crates/uc-application/src/space/session',
    'crates/uc-application/src/space/synchronize_membership_history',
    'crates/uc-application/src/space/unlock_space',
    'crates/uc-application/src/space/upgrade_space',
    'crates/uc-application/src/space/admission/handle_space_admission_message',
    'crates/uc-application/src/space/admission/outbox.rs',
    'crates/uc-application/src/space/admission/cancel_space_join/use_case.rs',
    'crates/uc-application/src/space/admission/complete_pending_space_transition/use_case.rs',
    'crates/uc-application/src/space/admission/query_pending_space_transition/use_case.rs',
    'crates/uc-application/src/space/membership/ledger/join_record.rs',
    'crates/uc-core/src/membership/space_join_record.rs',
  ]
  for (const path of retiredPaths) {
    if (existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'application membership cutover', `retired path remains: ${path}`)
    }
  }

  const applicationSources = readSourceTree('crates/uc-application/src')
  const forbiddenMarkers = [
    'MembershipStateCoordinator',
    'MembershipConvergence',
    'SpaceModules',
    'SpaceMembershipState',
    'WorkspaceSnapshot',
    'ContentExchangeGatePort',
    'CurrentWorkspacePeerScopePort',
    'SpaceJoinRecordStorePort',
    'SpaceJoinRecord',
    'AdmissionOutboxDeliveryPort',
    'LegacySynchronizeMembershipHistoryUseCase',
  ]
  for (const marker of forbiddenMarkers) {
    if (applicationSources.includes(marker)) {
      addProblem(
        problems,
        'application membership cutover',
        `retired application marker remains: ${marker}`
      )
    }
  }

  const facadeSurface = read('crates/uc-application/src/facade/mod.rs')
  for (const marker of [
    'MemberRosterFacade',
    'SpaceMembershipMaintenanceRuntime',
    'QueryDeviceTrustUseCase',
    'RemoveSpaceMemberUseCase',
    'DecideDeviceTrustChangeUseCase',
    'MaintainSpaceMembershipUseCase',
  ]) {
    if (facadeSurface.includes(marker)) {
      addProblem(
        problems,
        'application membership cutover',
        `internal Space implementation is publicly re-exported: ${marker}`
      )
    }
  }

  const sensitiveTracingField = /(device(?:_id)?|peer|from_device|target|address|path|file(?:name)?)\s*=\s*%/
  if (sensitiveTracingField.test(applicationSources)) {
    addProblem(
      problems,
      'application membership cutover',
      'application tracing contains a raw device, address, filename, or path field'
    )
  }
  return problems
}

function checkSpaceModuleInterface() {
  const problems = []
  const spaceRoot = 'crates/uc-application/src/space'
  const moduleSource = read(`${spaceRoot}/mod.rs`)
  const allowedRootEntries = new Set([
    'AGENTS.md',
    'admission',
    'adapters.rs',
    'application.rs',
    'application_tests.rs',
    'connectivity',
    'facade',
    'lifecycle',
    'membership',
    'mod.rs',
  ])
  for (const entry of readdirSync(join(REPOSITORY_ROOT, spaceRoot))) {
    if (!allowedRootEntries.has(entry)) {
      addProblem(
        problems,
        'space module interface',
        `${spaceRoot}/${entry} is outside the approved responsibility areas`
      )
    }
  }
  const publicChildModule = /^\s*pub(?:\(crate\))?\s+mod\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*;/gm
  for (const match of moduleSource.matchAll(publicChildModule)) {
    addProblem(
      problems,
      'space module interface',
      `${spaceRoot}/mod.rs exposes child module ${match[1]}; export approved items instead`
    )
  }

  const responsibilityRoots = ['admission', 'connectivity', 'facade', 'lifecycle', 'membership']
  for (const responsibility of responsibilityRoots) {
    const responsibilityModule = read(`${spaceRoot}/${responsibility}/mod.rs`)
    for (const match of responsibilityModule.matchAll(publicChildModule)) {
      addProblem(
        problems,
        'space module interface',
        `${spaceRoot}/${responsibility}/mod.rs exposes child module ${match[1]}`
      )
    }
  }

  for (const { path, source } of rustSourcesUnder('crates/uc-application/src')) {
    if (!path.startsWith(`${spaceRoot}/`) && /\bcrate::space::[a-z_][a-zA-Z0-9_]*::/.test(source)) {
      addProblem(
        problems,
        'space module interface',
        `${path} reaches through the space module interface`
      )
    }
    if (path.startsWith(`${spaceRoot}/`) && source.includes('crate::facade::space_setup::')) {
      addProblem(
        problems,
        'space module interface',
        `${path} depends on the public space facade re-export`
      )
    }
    for (const responsibility of responsibilityRoots) {
      const responsibilityRoot = `${spaceRoot}/${responsibility}/`
      const deepReference = new RegExp(
        `\\bcrate::space::${responsibility}::[a-z_][a-zA-Z0-9_]*::`
      )
      if (!path.startsWith(responsibilityRoot) && deepReference.test(source)) {
        addProblem(
          problems,
          'space module interface',
          `${path} reaches through the ${responsibility} responsibility interface`
        )
      }
    }
  }
  return problems
}

function checkSpaceAdmissionProtocolOwnership() {
  const problems = []
  const protocolRoot = 'crates/uc-application/src/space/admission/protocol'
  const coreStateRoot = 'crates/uc-core/src/membership/space_admission/state'
  const protocol = read(`${protocolRoot}/protocol.rs`)
  const moduleSurface = read(`${protocolRoot}/mod.rs`)
  const requiredRoleEntries = [
    'joiner/mod.rs',
    'joiner/start_join/execute.rs',
    'joiner/handle_candidate/execute.rs',
    'joiner/handle_commit/execute.rs',
    'joiner/handle_complete/execute.rs',
    'joiner/activate_complete/execute.rs',
    'joiner/handle_settled/execute.rs',
    'sponsor/mod.rs',
    'sponsor/handle_authenticated_message/execute.rs',
    'sponsor/handle_join_request/execute.rs',
    'sponsor/handle_prepared/execute.rs',
    'sponsor/handle_applied/execute.rs',
    'sponsor/handle_complete_ack/execute.rs',
    'sponsor/state/mod.rs',
    'sponsor/state/error.rs',
    'sponsor/state/model.rs',
    'sponsor/state/ports.rs',
    'recovery/mod.rs',
    'recovery/recover_pending/execute.rs',
  ]
  for (const entry of requiredRoleEntries) {
    if (!existsSync(join(REPOSITORY_ROOT, protocolRoot, entry))) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `missing role-owned entry: ${protocolRoot}/${entry}`
      )
    }
  }

  const retiredSplitEntries = [
    'joiner.rs',
    'sponsor.rs',
    'recovery.rs',
    'start_join',
    'handle_authenticated_message',
    'recover_pending',
  ]
  for (const entry of retiredSplitEntries) {
    if (existsSync(join(REPOSITORY_ROOT, protocolRoot, entry))) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `split role entry must be removed: ${protocolRoot}/${entry}`
      )
    }
  }

  const roleSource = entry => {
    const path = join(REPOSITORY_ROOT, protocolRoot, entry)
    return existsSync(path) ? read(`${protocolRoot}/${entry}`) : ''
  }
  const joinerStart = roleSource('joiner/start_join/execute.rs')
  const joinerCandidate = roleSource('joiner/handle_candidate/execute.rs')
  const joinerCommit = roleSource('joiner/handle_commit/execute.rs')
  const joinerComplete = roleSource('joiner/handle_complete/execute.rs')
  const joinerActivation = roleSource('joiner/activate_complete/execute.rs')
  const joinerSettled = roleSource('joiner/handle_settled/execute.rs')
  const recovery = roleSource('recovery/recover_pending/execute.rs')
  const sponsorMessage = roleSource('sponsor/handle_join_request/execute.rs')
  const sponsorPrepared = roleSource('sponsor/handle_prepared/execute.rs')
  const sponsorApplied = roleSource('sponsor/handle_applied/execute.rs')
  const sponsorCompleteAck = roleSource('sponsor/handle_complete_ack/execute.rs')

  for (const child of ['joiner', 'sponsor', 'recovery']) {
    if (!moduleSurface.includes(`mod ${child};`)) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `protocol must contain a private ${child} responsibility module`
      )
    }
  }

  for (const field of [
    'joiner: JoinerAdmissionService',
    'sponsor: SponsorAdmissionService',
    'recovery: AdmissionRecoveryService',
    'execution_lock: tokio::sync::Mutex<()>',
  ]) {
    if (!protocol.includes(field)) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `SpaceAdmissionProtocol is missing ${field}`
      )
    }
  }

  for (const leakedDependency of [
    'SettingsPort',
    'JoinerStartMaterialPort',
    'JoinerStartStatePort',
    'PendingAdmissionRecoveryStatePort',
    'SpaceAdmissionTransportPort',
    'WakeSpaceMembershipMaintenancePort',
    'SponsorAdmissionStatePort',
    'PrepareSponsorCandidatePort',
    'PrepareSponsorCommitPort',
    'PrepareSponsorCompletePort',
    'PrepareSponsorSettledPort',
    'PrepareJoinerCandidatePort',
    'PrepareJoinerAppliedPort',
    'PrepareJoinerActivationPort',
    'JoinerActivationStatePort',
    'ExecuteJoinerActivationPort',
  ]) {
    if (protocol.includes(leakedDependency)) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `SpaceAdmissionProtocol directly owns ${leakedDependency}`
      )
    }
  }

  for (const [source, owner, action] of [
    [joinerStart, 'JoinerAdmissionService', 'start'],
    [joinerCandidate, 'JoinerAdmissionService', 'handle_candidate'],
    [joinerCommit, 'JoinerAdmissionService', 'handle_commit'],
    [joinerComplete, 'JoinerAdmissionService', 'handle_complete'],
    [joinerActivation, 'JoinerAdmissionService', 'recover_activation'],
    [joinerSettled, 'JoinerAdmissionService', 'handle_settled'],
    [sponsorMessage, 'SponsorAdmissionService', 'handle_join_request'],
    [sponsorPrepared, 'SponsorAdmissionService', 'handle_prepared'],
    [sponsorApplied, 'SponsorAdmissionService', 'handle_applied'],
    [sponsorCompleteAck, 'SponsorAdmissionService', 'handle_complete_ack'],
    [recovery, 'AdmissionRecoveryService', 'recover_pending'],
  ]) {
    if (!source.includes(`impl ${owner}`) || !source.includes(`async fn ${action}(`)) {
      addProblem(
        problems,
        'space admission protocol ownership',
        `${owner} must own ${action}`
      )
    }
  }

  for (const { path, source } of rustSourcesUnder('crates/uc-application/src')) {
    if (path.endsWith('/tests.rs') || path.endsWith('/test_support.rs')) continue
    if (/\bSpaceAdmissionAggregate\b/.test(source)) {
      addProblem(
        problems,
        'space admission role capabilities',
        `${path} exposes the complete admission record to Application`
      )
    }
    if (/\bAdmissionRecordPersistence\b/.test(source)) {
      addProblem(
        problems,
        'space admission role capabilities',
        `${path} exposes admission persistence capability to Application`
      )
    }
  }

  const capabilityPath = `${coreStateRoot}/capability.rs`
  if (!existsSync(join(REPOSITORY_ROOT, capabilityPath))) {
    addProblem(
      problems,
      'space admission role capabilities',
      `missing Core role capability module: ${capabilityPath}`
    )
  } else {
    const capability = read(capabilityPath)
    for (const role of ['JoinerAdmission', 'SponsorAdmission']) {
      if (!capability.includes(`pub struct ${role}`)) {
        addProblem(
          problems,
          'space admission role capabilities',
          `${capabilityPath} is missing ${role}`
        )
      }
    }
  }

  for (const role of ['joiner', 'sponsor', 'helper', 'terminal']) {
    const transitionPath = `${coreStateRoot}/transition/${role}.rs`
    const transition = read(transitionPath)
    if (/\n\s*pub fn /.test(transition)) {
      addProblem(
        problems,
        'space admission role capabilities',
        `${transitionPath} still publishes raw Aggregate transitions`
      )
    }
  }

  return problems
}

function checkSpaceMembershipMaintenanceOwnership() {
  const problems = []
  const runtimePath = 'crates/uc-application/src/space/membership/maintenance/runtime.rs'
  const portsPath = 'crates/uc-application/src/space/membership/maintenance/ports.rs'
  const retiredWakePath =
    'crates/uc-application/src/space/membership/remove_space_member/ports.rs'
  const runtime = read(runtimePath)
  const ports = read(portsPath)

  for (const required of [
    'SpaceMembershipMaintenanceRuntime',
    'SpaceMembershipMaintenanceActivity',
    'SpaceMembershipMaintenanceRuntimeError',
  ]) {
    if (!runtime.includes(required)) {
      addProblem(
        problems,
        'space membership maintenance ownership',
        `${runtimePath} is missing ${required}`
      )
    }
  }

  const spaceSources = readSourceTree('crates/uc-application/src/space')
  for (const retired of [
    'SpaceMembershipRuntime',
    'SpaceMembershipActivity',
    'SpaceMembershipRuntimeError',
  ]) {
    if (spaceSources.includes(retired)) {
      addProblem(
        problems,
        'space membership maintenance ownership',
        `retired broad runtime name remains: ${retired}`
      )
    }
  }

  if (!ports.includes('pub trait WakeSpaceMembershipMaintenancePort')) {
    addProblem(
      problems,
      'space membership maintenance ownership',
      `${portsPath} must own WakeSpaceMembershipMaintenancePort`
    )
  }
  if (existsSync(join(REPOSITORY_ROOT, retiredWakePath))) {
    addProblem(
      problems,
      'space membership maintenance ownership',
      `retired wake-port path remains: ${retiredWakePath}`
    )
  }

  return problems
}

function checkSpaceAdmissionPersistenceOwnership() {
  const problems = []
  const stateRoot = 'crates/uc-core/src/membership/space_admission/state'
  const persistenceRoot = `${stateRoot}/persistence`
  const requiredEntries = [
    'mod.rs',
    'aggregate.rs',
    'initial.rs',
    'invitation.rs',
    'joiner.rs',
    'sponsor.rs',
    'terminal.rs',
    'message.rs',
    'value.rs',
  ]

  if (existsSync(join(REPOSITORY_ROOT, `${stateRoot}/persistence.rs`))) {
    addProblem(
      problems,
      'space admission persistence ownership',
      `${stateRoot}/persistence.rs must be replaced by role-owned persistence files`
    )
  }
  for (const entry of requiredEntries) {
    if (!existsSync(join(REPOSITORY_ROOT, persistenceRoot, entry))) {
      addProblem(
        problems,
        'space admission persistence ownership',
        `missing persistence responsibility file: ${persistenceRoot}/${entry}`
      )
    }
  }
  const joinerState = read(`${stateRoot}/joiner.rs`)
  const initialPersistence = read(`${persistenceRoot}/initial.rs`)
  if (!joinerState.includes('private_state: AdmissionJoinerPrivateState')) {
    addProblem(
      problems,
      'space admission persistence ownership',
      'Joiner Initiated must own its opaque private state'
    )
  }
  if (!initialPersistence.includes('private_state: state.private_state.as_bytes().to_vec()')) {
    addProblem(
      problems,
      'space admission persistence ownership',
      'Joiner private state must be included in encrypted admission persistence'
    )
  }

  return problems
}

function checkDualInvitationEntry() {
  const problems = []
  const invitationPortPath = 'crates/uc-core/src/ports/pairing_invitation.rs'
  const invitationPort = read(invitationPortPath)
  for (const required of [
    'pub invitation_id: InvitationId',
    'pub full_invitation: FullInvitation',
  ]) {
    if (!invitationPort.includes(required)) {
      addProblem(
        problems,
        'dual invitation entry',
        `${invitationPortPath} is missing ${required}`
      )
    }
  }

  const codecPath = 'crates/uc-infra/src/space/admission/full_invitation.rs'
  const codec = read(codecPath)
  for (const required of [
    'FULL_INVITATION_PREFIX',
    'decode_invitation_entry',
    'InvitationId::from_bytes',
  ]) {
    if (!codec.includes(required)) {
      addProblem(problems, 'dual invitation entry', `${codecPath} is missing ${required}`)
    }
  }

  const joinerStatePath =
    'crates/uc-core/src/membership/space_admission/state/joiner.rs'
  const joinerTransitionPath =
    'crates/uc-core/src/membership/space_admission/state/transition/joiner.rs'
  const joinerState = read(joinerStatePath)
  const joinerTransition = read(joinerTransitionPath)
  for (const required of [
    'SpaceAdmissionInvitationResolutionState',
    'Ready {',
    'Started',
    'ResolvedInvitation',
  ]) {
    if (!joinerState.includes(required)) {
      addProblem(problems, 'dual invitation entry', `${joinerStatePath} is missing ${required}`)
    }
  }
  for (const required of [
    'mark_invitation_resolution_started',
    'save_resolved_invitation',
    'reject_started_invitation_resolution',
  ]) {
    if (!joinerTransition.includes(required)) {
      addProblem(
        problems,
        'dual invitation entry',
        `${joinerTransitionPath} is missing ${required}`
      )
    }
  }

  const resolutionPath =
    'crates/uc-application/src/space/admission/protocol/joiner/resolve_invitation/execute.rs'
  const resolution = read(resolutionPath)
  const startedCommit = resolution.indexOf('commit_recovery(token, transition).await')
  const resolveOnce = resolution.indexOf('resolve_once(&short_code).await')
  if (startedCommit < 0 || resolveOnce < 0 || startedCommit > resolveOnce) {
    addProblem(
      problems,
      'dual invitation entry',
      `${resolutionPath} must commit Started before resolving the short code`
    )
  }

  const uniffiRuntimePath = 'bindings/uc-engine-uniffi/src/runtime.rs'
  const uniffiRuntime = read(uniffiRuntimePath)
  for (const required of [
    '.field("invitation_code", &"[REDACTED]")',
    '.field("full_invitation", &"[REDACTED]")',
  ]) {
    if (!uniffiRuntime.includes(required)) {
      addProblem(problems, 'dual invitation entry', `${uniffiRuntimePath} is missing ${required}`)
    }
  }

  for (const path of [
    'crates/uc-infra/src/rendezvous/invitation_adapter.rs',
    'crates/uc-infra/src/pairing/invitation_resolver.rs',
  ]) {
    if (/code\s*=\s*%code\.as_str\(\)/.test(read(path))) {
      addProblem(problems, 'dual invitation entry', `${path} logs a full invitation code`)
    }
  }

  return problems
}

function checkInfraSpaceAdmissionOwnership() {
  const problems = []
  const admissionRoot = 'crates/uc-infra/src/space/admission'
  const requiredEntries = [
    'mod.rs',
    'full_invitation.rs',
    'security/mod.rs',
    'security/transition.rs',
    'repository/mod.rs',
    'repository/persisted.rs',
    'repository/codec.rs',
    'repository/token.rs',
    'joiner/mod.rs',
    'joiner/activation_state.rs',
    'joiner/invitation_start.rs',
    'joiner/start_state.rs',
    'joiner/source_snapshot.rs',
    'recovery/mod.rs',
    'recovery/pending_state.rs',
    'sponsor/mod.rs',
    'sponsor/base_snapshot.rs',
    'sponsor/state.rs',
  ]
  const retiredEntries = [
    'crates/uc-infra/src/db/repositories/space_join_record_store.rs',
    'crates/uc-infra/src/network/iroh/admission_completion_recovery_adapter.rs',
    'crates/uc-infra/src/pairing/admission_outbox_delivery.rs',
  ]

  for (const entry of requiredEntries) {
    if (!existsSync(join(REPOSITORY_ROOT, admissionRoot, entry))) {
      addProblem(
        problems,
        'infra space admission ownership',
        `missing role-owned admission implementation: ${admissionRoot}/${entry}`
      )
    }
  }
  for (const path of retiredEntries) {
    if (existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'infra space admission ownership', `retired admission path remains: ${path}`)
    }
  }
  if (
    existsSync(join(REPOSITORY_ROOT, admissionRoot)) &&
    readSourceTree(admissionRoot).includes('SpaceJoinRecord')
  ) {
    addProblem(
      problems,
      'infra space admission ownership',
      'new admission implementation depends on retired SpaceJoinRecord types'
    )
  }

  return problems
}

function checkInfraSpaceSecurityOwnership() {
  const problems = []
  const securityRoot = 'crates/uc-infra/src/space/security'
  const requiredEntries = [
    'mod.rs',
    'access.rs',
    'history_signature.rs',
    'key_material.rs',
    'membership_update.rs',
    'mls_group.rs',
    'peer_admission.rs',
    'scope_identifier.rs',
    'session.rs',
    'session_rebind.rs',
  ]
  const retiredEntries = [
    'crates/uc-infra/src/security/adapters/space_session_rebind.rs',
    'crates/uc-infra/src/security/admission_security_transition.rs',
    'crates/uc-infra/src/security/historical_signature_adapter.rs',
    'crates/uc-infra/src/security/key_material.rs',
    'crates/uc-infra/src/security/membership_security_update_adapter.rs',
    'crates/uc-infra/src/security/mls_group.rs',
    'crates/uc-infra/src/security/peer_admission_adapter.rs',
    'crates/uc-infra/src/security/scope_identifier.rs',
    'crates/uc-infra/src/security/session.rs',
    'crates/uc-infra/src/security/space_access_adapter.rs',
  ]

  for (const entry of requiredEntries) {
    if (!existsSync(join(REPOSITORY_ROOT, securityRoot, entry))) {
      addProblem(
        problems,
        'infra space security ownership',
        `missing Space security implementation: ${securityRoot}/${entry}`
      )
    }
  }
  for (const path of retiredEntries) {
    if (existsSync(join(REPOSITORY_ROOT, path))) {
      addProblem(problems, 'infra space security ownership', `retired security path remains: ${path}`)
    }
  }

  return problems
}

function checkRetiredLegacySpaceTransition(sources) {
  const problems = []
  const legacyPath = 'crates/uc-infra/src/security/admission_space_transition.rs'
  if (sources.legacySpaceTransitionPathPresent) {
    addProblem(
      problems,
      'retired legacy Space transition',
      `retired transition module remains: ${legacyPath}`
    )
  }
  for (const marker of [
    'mod admission_space_transition',
    'pub use admission_space_transition',
    'space_generation_directory',
  ]) {
    if (sources.infraSecurityModule.includes(marker)) {
      addProblem(
        problems,
        'retired legacy Space transition',
        `security module exposes retired transition knowledge through ${marker}`
      )
    }
  }
  for (const marker of [
    'DurableAdmissionSpaceTransition',
    'rewrap_finalized_source',
    'source-backup-v1',
  ]) {
    if (sources.infraSecurityRuntime.includes(marker)) {
      addProblem(
        problems,
        'retired legacy Space transition',
        `Infra security source restores retired transition behavior through ${marker}`
      )
    }
  }
  for (const marker of ['space_generation_directory', 'space-generations', 'target.sqlite']) {
    if (sources.runtimeStorage.includes(marker)) {
      addProblem(
        problems,
        'retired legacy Space transition',
        `Engine runtime storage reopens a retired V2 generation through ${marker}`
      )
    }
  }
  for (const required of ['ActiveRuntimeManifest::V2(_)', 'StorageUpgradeRequired']) {
    if (!sources.runtimeStorage.includes(required)) {
      addProblem(
        problems,
        'retired legacy Space transition',
        `Engine runtime storage is missing the V2 fail-closed marker ${required}`
      )
    }
  }
  return problems
}

function checkObservabilityAssemblyInterface(sources) {
  const problems = []
  const runtimeAdapters = sources.spaceAdapters.match(
    /pub struct SpaceRuntimeAdapters\s*\{(?<body>[^}]*)\}/s
  )
  if (!runtimeAdapters?.groups?.body) {
    addProblem(
      problems,
      'observability assembly interface',
      'Application does not own SpaceRuntimeAdapters'
    )
  } else {
    const fields = [...runtimeAdapters.groups.body.matchAll(/pub\s+(\w+)\s*:/g)]
      .map(match => match[1])
      .sort()
    if (JSON.stringify(fields) !== JSON.stringify(['admission', 'membership'])) {
      addProblem(
        problems,
        'observability assembly interface',
        `SpaceRuntimeAdapters must contain only admission and membership; found ${fields.join(', ')}`
      )
    }
  }

  for (const marker of ['AdmissionPortImplementations', 'ObservedAdmissionPorts']) {
    if (sources.engineObservability.includes(marker)) {
      addProblem(
        problems,
        'observability assembly interface',
        `retired mirror bundle remains: ${marker}`
      )
    }
  }

  for (const domain of ['admission', 'membership']) {
    const reexport = `pub(crate) use ${domain}::observe_${domain};`
    const domainReexports = sources.observabilityModule.match(
      new RegExp(`pub\\(crate\\)\\s+use\\s+${domain}::`, 'g')
    ) ?? []
    if (
      sources.observabilityModule.split(reexport).length - 1 !== 1 ||
      domainReexports.length !== 1
    ) {
      addProblem(
        problems,
        'observability assembly interface',
        `observability module must expose exactly one ${domain} entry`
      )
    }
    const call = `observability::observe_${domain}(`
    if (sources.syncEngine.split(call).length - 1 !== 1) {
      addProblem(
        problems,
        'observability assembly interface',
        `sync_engine must call ${call} exactly once`
      )
    }
  }

  if (/pub\(crate\)\s+struct\s+(?:Observed\w+|\w+ObservationPolicy)/.test(sources.engineObservability)) {
    addProblem(
      problems,
      'observability assembly interface',
      'concrete decorator or observation policy is crate-visible'
    )
  }
  if (/Instant::now\(\)|target:\s*"(?:admission|membership)\.performance"/.test(sources.spaceApplication)) {
    addProblem(
      problems,
      'observability assembly interface',
      'Space Application assembly contains cross-layer performance observation'
    )
  }

  return problems
}

function repositorySources() {
  return {
    legacySpaceTransitionPathPresent: existsSync(
      join(REPOSITORY_ROOT, 'crates/uc-infra/src/security/admission_space_transition.rs')
    ),
    infraSecurityModule: read('crates/uc-infra/src/security/mod.rs'),
    infraSecurityRuntime: readSourceTree('crates/uc-infra/src/security'),
    runtimeStorage: read('crates/uc-engine/src/assembly/runtime_storage.rs'),
    observabilityModule: read('crates/uc-engine/src/assembly/observability/mod.rs'),
    engineObservability: readSourceTree('crates/uc-engine/src/assembly/observability'),
    syncEngine: read('crates/uc-engine/src/assembly/sync_engine.rs'),
    engine: read('crates/uc-engine/src/lib.rs'),
    engineRuntime: readSourceTree('crates/uc-engine/src'),
    engineWiring: read('crates/uc-engine/src/assembly/wire/mod.rs'),
    applicationDeps: read('crates/uc-application/src/deps.rs'),
    application: readSourceTree('crates/uc-application/src'),
    spaceAdapters: read('crates/uc-application/src/space/adapters.rs'),
    spaceApplication: read('crates/uc-application/src/space/application.rs'),
    network: readSourceTree('crates/uc-infra/src/network'),
    v3AdmissionTransition: read(
      'crates/uc-infra/src/security/v3_admission_space_transition.rs'
    ),
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
    ...checkApplicationDependencyInventory(sources),
    ...checkBindingProvenance(metadata, sources),
    ...checkLanIsolation(metadata, sources),
    ...checkProfileStorageGenerationOwnership(sources),
    ...checkCurrentPeerScopeOwnership(),
    ...checkMembershipConfirmationWatermarkOwnership(sources),
    ...checkApplicationMembershipCutover(),
    ...checkSpaceModuleInterface(),
    ...checkSpaceAdmissionProtocolOwnership(),
    ...checkSpaceAdmissionPersistenceOwnership(),
    ...checkInfraSpaceAdmissionOwnership(),
    ...checkInfraSpaceSecurityOwnership(),
    ...checkRetiredLegacySpaceTransition(sources),
    ...checkObservabilityAssemblyInterface(sources),
    ...checkDualInvitationEntry(),
    ...checkSpaceMembershipMaintenanceOwnership(),
    ...checkRetiredLegacyPairingRecovery(),
    ...checkRetiredPairingTransport(sources),
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
  expectRejected('retired Application dependency inventory', (_changed, changedSources) => {
    changedSources.applicationDeps += '\npub current_profile: RetiredWiringOnlyPort,\n'
  }, metadata, sources)
  expectRejected('retired pairing transport', (_changed, changedSources) => {
    changedSources.runtime += '\nconst PAIRING_ALPN: &[u8] = b"/uniclipboard/pairing/2";\n'
  }, metadata, sources)
  expectRejected('missing OpenMLS validation target', changed => {
    const validation = packageByName(changed, 'openmls-validation')
    validation.targets = validation.targets.filter(target => !target.kind.includes('test'))
  }, metadata, sources)
  expectRejected('V3 CrossSpace payload rewrap dependency', (_changed, changedSources) => {
    changedSources.v3AdmissionTransition += '\nuse crate::security::ProfileStorageUpgrade;\nfn payload_rewrap() {}\n'
  }, metadata, sources)
  expectRejected('Engine profile-upgrade phase orchestration', (_changed, changedSources) => {
    changedSources.engineRuntime += '\nfn combine_upgrade_steps(_: TargetGenerationStager) {}\n'
  }, metadata, sources)
  expectRejected('historical vault used as network authority', (_changed, changedSources) => {
    changedSources.network += '\nfn authorize_from_history(_: ProfileContentKeyVault) {}\n'
  }, metadata, sources)
  expectRejected('forged membership confirmation watermark', (_changed, changedSources) => {
    changedSources.application +=
      '\nfn infer_peer_confirmation(peer: &mut PeerReconciliationRecord, position: BaseMembershipHistoryPosition) { peer.confirmed_position = Some(position); }\n'
  }, metadata, sources)
  expectRejected('observability mirror bundle', (_changed, changedSources) => {
    changedSources.engineObservability += '\nstruct ObservedAdmissionPorts;\n'
  }, metadata, sources)
  expectRejected('second admission observation entry', (_changed, changedSources) => {
    changedSources.observabilityModule +=
      '\npub(crate) use admission::observe_admission_again;\n'
  }, metadata, sources)
  expectRejected('public observation decorator', (_changed, changedSources) => {
    changedSources.engineObservability += '\npub(crate) struct ObservedMembershipLeak;\n'
  }, metadata, sources)
  expectRejected('retired legacy Space transition module', (_changed, changedSources) => {
    changedSources.legacySpaceTransitionPathPresent = true
  }, metadata, sources)
  expectRejected('retired legacy Space transition export', (_changed, changedSources) => {
    changedSources.infraSecurityModule += `
mod admission_space_transition;
pub use admission_space_transition::{space_generation_directory, DurableAdmissionSpaceTransition};
`
  }, metadata, sources)
  expectRejected('retired legacy Space transition implementation', (_changed, changedSources) => {
    changedSources.infraSecurityRuntime += `
pub struct DurableAdmissionSpaceTransition;
`
  }, metadata, sources)
  expectRejected('Engine V2 runtime storage bypass', (_changed, changedSources) => {
    changedSources.runtimeStorage += `
fn open_v2_generation(directory: &Path) {
  let database = directory.join("space-generations").join("target.sqlite");
}
`
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
