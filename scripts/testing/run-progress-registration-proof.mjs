#!/usr/bin/env node
// 串行构建两个当前测试二进制，显式关联实际接收fixture与真实Iroh登记，禁止猜测旧产物路径。
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createReadStream, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
const root = resolve(import.meta.dirname, '../..')
const buildScope = process.env.UC_TEST_BUILD_SCOPE || 'group'
if (!['group', 'workspace'].includes(buildScope)) throw new Error('UC_TEST_BUILD_SCOPE must be group or workspace')
const report = resolve(root, process.env.UC_TEST_ARTIFACTS_DIR ?? 'target/test-artifacts/ci', 'progress-registration')
mkdirSync(report, { recursive: true })
rmSync(resolve(report, 'report.json'), { force: true })
const capacityReport = resolve(report, 'snapshot-capacity.json')
rmSync(capacityReport, { force: true })
const prefix = 'network::iroh::connectivity::stream::authority_probe::'
const required = [
  'current_receive_authority_binds_over_real_iroh_and_old_descriptors_cannot_roll_back',
  'actual_receiver_restart_new_connection_retires_old_producer_and_pending_stream',
  'fresh_challenge_reads_current_attempt_and_late_bound_is_checked_again',
  'explicit_legacy_is_unsupported_but_corrupt_current_frame_keeps_exchange_failure',
  'live_receive_attempt_reports_its_failed_materialization_through_the_complete_action',
  'snapshot_probe::actual_maximum_v3_plan_survives_sixteen_real_snapshot_pages',
  'query_provider_probe::actual_failed_receive_results_cross_sixteen_real_inbound_query_pages',
  'legacy_probe::actual_failed_receive_results_form_one_complete_legacy_summary_without_ack',
].map(name => prefix + name)
function binary(packageName, targetName) {
  // CI 沿用前后验收的工作区构建范围，避免单包特性解析使整条链路再次编译。
  const targets = buildScope === 'workspace' ? ['--workspace', '--all-targets'] : ['-p', packageName, '--lib']
  const built = spawnSync('just', ['cargo', 'test', ...targets, '--no-run', '--locked', '--message-format=json'], {
    cwd: root, encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, stdio: ['ignore', 'pipe', 'inherit'],
  })
  if (built.error) throw built.error
  if (built.status !== 0) {
    for (const line of built.stdout.split('\n').filter(Boolean)) {
      try { const row = JSON.parse(line); if (row.message?.rendered) process.stderr.write(row.message.rendered) }
      catch { process.stderr.write(line + '\n') }
    }
    process.exit(built.status ?? 1)
  }
  const artifacts = built.stdout.split('\n').filter(Boolean).map(line => JSON.parse(line)).filter(row =>
    row.reason === 'compiler-artifact' && row.target.name === targetName && row.profile.test && row.executable)
  if (artifacts.length !== 1) throw new Error(`expected one current ${packageName} test binary`)
  return artifacts[0].executable
}
const application = binary('uc-application', 'uc_application')
const network = binary('uc-infra-p2p', 'uc_infra_p2p')
const listed = spawnSync(network, [prefix, '--ignored', '--list'], { cwd: root, encoding: 'utf8' })
if (listed.error) throw listed.error
if (listed.status !== 0 || required.some(name => !listed.stdout.split('\n').includes(name + ': test'))) {
  throw new Error('mandatory current-authority proof cases are missing from the current binary')
}
const run = spawnSync(network, [prefix, '--ignored', '--nocapture'], {
  cwd: root, env: { ...process.env, UC_APPLICATION_TEST_BINARY: application, UC_PROGRESS_SNAPSHOT_REPORT: capacityReport }, stdio: 'inherit',
})
async function sha(file) {
  const hash = createHash('sha256')
  for await (const chunk of createReadStream(file)) hash.update(chunk)
  return hash.digest('hex')
}
let capacity = null
if (run.status === 0) {
  capacity = JSON.parse(readFileSync(capacityReport, 'utf8'))
  if (capacity.plan_items !== 1024 || capacity.snapshot_items !== 1024 || capacity.pages !== 16 ||
      capacity.staged_capacity_items !== 1024 || capacity.page_item_limit !== 64 || capacity.frame_limit_bytes !== 65536 ||
      !Number.isSafeInteger(capacity.largest_frame_bytes) || capacity.largest_frame_bytes <= 0 || capacity.largest_frame_bytes > capacity.frame_limit_bytes ||
      !Number.isSafeInteger(capacity.staged_item_storage_bytes) || capacity.staged_item_storage_bytes <= 0 || capacity.plan_authority !== 'actual_application_cas_v3' ||
      capacity.state_payload !== 'synthetic' || capacity.connection_profile !== 'transfer_progress') throw new Error('maximum original-plan snapshot capacity proof is missing or invalid')
}
const provenance = spawnSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' })
const state = spawnSync('git', ['status', '--porcelain'], { cwd: root, encoding: 'utf8' })
if (provenance.status !== 0 || state.status !== 0) throw new Error('cannot record proof source provenance')
writeFileSync(resolve(report, 'report.json'), JSON.stringify({
  source_commit: provenance.stdout.trim(), source_state: state.stdout.trim() ? 'modified' : 'clean',
  application_binary_sha256: await sha(application), network_binary_sha256: await sha(network),
  required_cases: required, snapshot_capacity: capacity, exit_code: run.status, signal: run.signal,
}, null, 2) + '\n')
if (run.error) throw run.error
process.exit(run.status ?? 1)
