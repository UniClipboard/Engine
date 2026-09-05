import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const HERE = dirname(fileURLToPath(import.meta.url))
const CONFIGS = ['collector.yaml', 'collector.posthog.yaml']

function read(name) {
  return readFileSync(join(HERE, name), 'utf8')
}

function block(source, start, end) {
  const from = source.indexOf(start)
  const to = source.indexOf(end, from + start.length)
  assert.notEqual(from, -1, `missing block start: ${start}`)
  assert.notEqual(to, -1, `missing block end: ${end}`)
  return source.slice(from, to).trimEnd()
}

const RESOURCE_RULES = [
  'resource.schema_url != ""',
  'Len(resource.attributes) != 9',
  'resource.dropped_attributes_count != 0',
  'resource.attributes["service.namespace"] != "uniclipboard"',
  'resource.attributes["service.name"] != "uc-engine"',
  'IsString(resource.attributes["service.version"]) == false',
  'Len(resource.attributes["service.version"]) > 32',
  'IsMatch(resource.attributes["service.version"], "^[0-9]{1,5}[.][0-9]{1,5}[.][0-9]{1,5}(-(alpha|beta|rc)([.][0-9]{1,5})?)?$") == false',
  'IsString(resource.attributes["service.instance.id"]) == false',
  'IsMatch(resource.attributes["service.instance.id"], "^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$") == false',
  'IsMatch(resource.attributes["deployment.environment.name"], "^(development|test|staging|production)$") == false',
  'IsMatch(resource.attributes["os.type"], "^(ios|android|macos|windows|linux|ohos|other)$") == false',
  'IsMatch(resource.attributes["host.arch"], "^(arm64|arm|x86|x86_64|riscv64|s390x|powerpc|powerpc64|wasm32|other)$") == false',
  'IsMatch(resource.attributes["uc.app.channel"], "^(development|test|alpha|beta|stable|production)$") == false',
  'IsInt(resource.attributes["uc.telemetry.schema.version"]) == false',
  'resource.attributes["uc.telemetry.schema.version"] != 1',
]

const SCOPE_RULES = [
  'scope.schema_url != ""',
  'scope.version != ""',
  'Len(scope.attributes) != 0',
  'scope.dropped_attributes_count != 0',
]

const SPAN_RULES = [
  'scope.name != "uc-observability-runtime"',
  'span.attributes["target"] != "uc.telemetry"',
  'span.attributes["uc.flow.id"] == nil and Len(span.attributes) != 4',
  'span.attributes["uc.flow.id"] != nil and Len(span.attributes) != 5',
  'IsString(span.attributes["uc.domain"]) == false',
  'IsMatch(span.attributes["uc.domain"], "^(clipboard|space_admission|space_membership|storage|runtime)$") == false',
  'IsString(span.attributes["uc.operation"]) == false',
  'IsMatch(span.attributes["uc.operation"], "^(clipboard_dispatch|clipboard_receive|clipboard_address_resolve|clipboard_connect|space_admission|membership_history_sync|membership_group_update|network_transport|profile_storage_upgrade|session_lifecycle)$") == false',
  'IsString(span.attributes["uc.role"]) == false',
  'IsMatch(span.attributes["uc.role"], "^(local|client|server|joiner|sponsor|member)$") == false',
  'span.attributes["uc.domain"] != "space_admission" and span.name != span.attributes["uc.operation"]',
  'span.attributes["uc.domain"] == "space_admission" and ((span.attributes["uc.role"] == "local" and span.name != "pairing.lifecycle") or (span.attributes["uc.role"] == "joiner" and span.attributes["uc.operation"] == "space_admission" and IsMatch(span.name, "^pairing[.](authenticate|reconnect)$") == false) or (span.attributes["uc.role"] == "joiner" and span.attributes["uc.operation"] == "network_transport" and IsMatch(span.name, "^pairing[.](send_request|(request_join|confirm_prepared|confirm_applied|settle|cancel)[.]send)$") == false) or (span.attributes["uc.role"] == "sponsor" and span.attributes["uc.operation"] == "network_transport" and span.name != "pairing.receive_request") or (span.attributes["uc.role"] == "sponsor" and span.attributes["uc.operation"] == "space_admission" and IsMatch(span.name, "^pairing[.](process_request|(request_join|confirm_prepared|confirm_applied|settle|cancel)[.]process)$") == false))',
  'span.attributes["uc.flow.id"] != nil and IsString(span.attributes["uc.flow.id"]) == false',
  'span.attributes["uc.flow.id"] != nil and IsMatch(span.attributes["uc.flow.id"], "^[0-9a-f]{32}$") == false',
  'span.attributes["uc.flow.id"] != nil and (span.attributes["uc.domain"] != "space_admission" or ((span.attributes["uc.role"] != "joiner" or IsMatch(span.attributes["uc.operation"], "^(space_admission|network_transport)$") == false or span.kind != SPAN_KIND_CLIENT) and (span.attributes["uc.role"] != "local" or span.attributes["uc.operation"] != "space_admission" or span.kind != SPAN_KIND_INTERNAL or IsEmpty(span.parent_span_id) == false)))',
  'span.trace_state != ""',
  'Len(span.events) != 0',
  'Len(span.links) != 0',
  'span.status.message != ""',
  'span.status.code != STATUS_CODE_UNSET and span.status.code != STATUS_CODE_OK and span.status.code != STATUS_CODE_ERROR',
  'span.dropped_attributes_count != 0',
  'span.dropped_events_count != 0',
  'span.dropped_links_count != 0',
  'IsEmpty(span.trace_id)',
  'IsEmpty(span.span_id)',
]

const LOG_RULES = [
  'scope.name != "uc.telemetry"',
  'log.body != nil',
  'log.event_name != "uc.diagnostic"',
  'log.attributes["event.name"] != "uc.operation.completed"',
  'log.attributes["uc.flow.id"] != nil',
  'log.attributes["error.type"] == nil and Len(log.attributes) != 6',
  'log.attributes["error.type"] != nil and Len(log.attributes) != 7',
  'IsString(log.attributes["uc.domain"]) == false',
  'IsMatch(log.attributes["uc.domain"], "^(clipboard|space_admission|space_membership|storage|runtime)$") == false',
  'IsString(log.attributes["uc.operation"]) == false',
  'IsMatch(log.attributes["uc.operation"], "^(clipboard_dispatch|clipboard_receive|clipboard_address_resolve|clipboard_connect|space_admission|membership_history_sync|membership_group_update|network_transport|profile_storage_upgrade|session_lifecycle)$") == false',
  'IsString(log.attributes["uc.role"]) == false',
  'IsMatch(log.attributes["uc.role"], "^(local|client|server|joiner|sponsor|member)$") == false',
  'IsString(log.attributes["uc.outcome"]) == false',
  'IsMatch(log.attributes["uc.outcome"], "^(ok|error|deferred|rejected|cancelled)$") == false',
  'IsInt(log.attributes["duration_ms"]) == false',
  'log.attributes["duration_ms"] < 0',
  'log.attributes["uc.outcome"] == "error" and log.attributes["error.type"] == nil',
  'log.attributes["uc.outcome"] != "error" and log.attributes["error.type"] != nil',
  'log.attributes["error.type"] != nil and IsString(log.attributes["error.type"]) == false',
  'log.attributes["error.type"] != nil and IsMatch(log.attributes["error.type"], "^(authentication_failed|address_unavailable|connect_failed|stream_failed|decode_failed|timeout|channel_closed|storage|security|corrupt|source_changed|manifest|join_failed|shutdown_timeout|unavailable|peer_rejected|peer_incompatible|local_policy_exceeded|internal)$") == false',
  'IsEmpty(log.trace_id) != IsEmpty(log.span_id)',
  'IsMatch(log.severity_text, "^(INFO|ERROR)$") == false',
  '(log.attributes["uc.outcome"] == "error" and (log.severity_number != SEVERITY_NUMBER_ERROR or log.severity_text != "ERROR"))',
  '(log.attributes["uc.outcome"] != "error" and (log.severity_number != SEVERITY_NUMBER_INFO or log.severity_text != "INFO"))',
  'log.dropped_attributes_count != 0',
]

const CONSISTENCY_RULES = [
  '(span.attributes["uc.domain"] == "clipboard" and IsMatch(span.attributes["uc.operation"], "^clipboard_") == false)',
  '(span.attributes["uc.domain"] == "space_admission" and IsMatch(span.attributes["uc.operation"], "^(space_admission|network_transport)$") == false)',
  '(span.attributes["uc.domain"] == "space_membership" and IsMatch(span.attributes["uc.operation"], "^membership_") == false)',
  '(span.attributes["uc.domain"] == "storage" and span.attributes["uc.operation"] != "profile_storage_upgrade")',
  '(span.attributes["uc.domain"] == "runtime" and span.attributes["uc.operation"] != "session_lifecycle")',
  '(log.attributes["uc.domain"] == "clipboard" and IsMatch(log.attributes["uc.operation"], "^clipboard_") == false)',
  '(log.attributes["uc.domain"] == "space_admission" and IsMatch(log.attributes["uc.operation"], "^(space_admission|network_transport)$") == false)',
  '(log.attributes["uc.domain"] == "space_membership" and IsMatch(log.attributes["uc.operation"], "^membership_") == false)',
  '(log.attributes["uc.domain"] == "storage" and log.attributes["uc.operation"] != "profile_storage_upgrade")',
  '(log.attributes["uc.domain"] == "runtime" and log.attributes["uc.operation"] != "session_lifecycle")',
  '(span.attributes["uc.operation"] == "clipboard_dispatch" and (span.attributes["uc.role"] != "client" or span.kind != SPAN_KIND_CLIENT))',
  '(span.attributes["uc.operation"] == "clipboard_receive" and (span.attributes["uc.role"] != "server" or span.kind != SPAN_KIND_SERVER))',
  '(IsMatch(span.attributes["uc.operation"], "^clipboard_(address_resolve|connect)$") and (span.attributes["uc.role"] != "client" or span.kind != SPAN_KIND_INTERNAL))',
  '(span.attributes["uc.operation"] == "space_admission" and ((span.attributes["uc.role"] == "joiner" and span.kind != SPAN_KIND_CLIENT) or (span.attributes["uc.role"] == "local" and span.kind != SPAN_KIND_INTERNAL) or (span.attributes["uc.role"] == "local" and span.kind == SPAN_KIND_INTERNAL and (span.attributes["uc.flow.id"] == nil or IsEmpty(span.parent_span_id) == false)) or (span.attributes["uc.role"] == "sponsor" and span.kind != SPAN_KIND_INTERNAL) or IsMatch(span.attributes["uc.role"], "^(local|joiner|sponsor)$") == false))',
  '(span.attributes["uc.operation"] == "network_transport" and ((span.attributes["uc.role"] == "joiner" and span.kind != SPAN_KIND_CLIENT) or (span.attributes["uc.role"] == "sponsor" and span.kind != SPAN_KIND_SERVER) or IsMatch(span.attributes["uc.role"], "^(joiner|sponsor)$") == false))',
  '(IsMatch(span.attributes["uc.operation"], "^membership_") and (span.attributes["uc.role"] != "member" or IsMatch(span.kind.string, "^(Client|Server)$") == false))',
  '(span.attributes["uc.operation"] == "profile_storage_upgrade" and (span.attributes["uc.role"] != "local" or span.kind != SPAN_KIND_INTERNAL))',
  '(span.attributes["uc.operation"] == "session_lifecycle" and (span.attributes["uc.role"] != "local" or span.kind != SPAN_KIND_INTERNAL))',
  '(log.attributes["uc.operation"] == "clipboard_dispatch" and log.attributes["uc.role"] != "client")',
  '(log.attributes["uc.operation"] == "clipboard_receive" and log.attributes["uc.role"] != "server")',
  '(IsMatch(log.attributes["uc.operation"], "^clipboard_(address_resolve|connect)$") and log.attributes["uc.role"] != "client")',
  '(log.attributes["uc.operation"] == "space_admission" and IsMatch(log.attributes["uc.role"], "^(local|joiner|sponsor)$") == false)',
  '(log.attributes["uc.operation"] == "network_transport" and IsMatch(log.attributes["uc.role"], "^(joiner|sponsor)$") == false)',
  '(IsMatch(log.attributes["uc.operation"], "^membership_") and IsMatch(log.attributes["uc.role"], "^(local|member)$") == false)',
  '(IsMatch(log.attributes["uc.operation"], "^(profile_storage_upgrade|session_lifecycle)$") and log.attributes["uc.role"] != "local")',
]

test('local and PostHog collectors share one privacy boundary', () => {
  const [local, posthog] = CONFIGS.map(read)
  assert.equal(
    block(local, '  filter/diagnostics:', '  transform/allowlist:'),
    block(posthog, '  filter/diagnostics:', '  transform/allowlist:'),
  )
  assert.equal(
    block(local, '  transform/allowlist:', '\nexporters:'),
    block(posthog, '  transform/allowlist:', '  tail_sampling:'),
  )
})

test('collector rejects every unapproved telemetry carrier and value', () => {
  for (const name of CONFIGS) {
    const source = read(name)
    assert.match(source, /trace_conditions:/, `${name} must use Collector 0.160 trace conditions`)
    assert.match(source, /log_conditions:/, `${name} must use Collector 0.160 log conditions`)
    for (const rule of [...RESOURCE_RULES, ...SCOPE_RULES, ...SPAN_RULES, ...LOG_RULES, ...CONSISTENCY_RULES]) {
      assert.ok(source.includes(rule), `${name} is missing privacy rule: ${rule}`)
    }
  }
})

test('allowlist transformation cannot retain obsolete or open-ended fields', () => {
  for (const name of CONFIGS) {
    const source = read(name)
    const transform = block(source, '  transform/allowlist:', name === 'collector.yaml' ? '\nexporters:' : '  tail_sampling:')
    assert.ok(!transform.includes('set(log.body, nil)'))
    assert.ok(transform.includes('keep_keys(span.attributes, ["target", "uc.flow.id", "uc.domain", "uc.operation", "uc.role"])'))
    assert.ok(transform.includes('keep_keys(log.attributes, ["event.name", "uc.domain", "uc.operation", "uc.role", "uc.outcome", "error.type", "duration_ms"])'))
    assert.ok(!transform.includes('uc.message.kind'))
    assert.ok(!transform.includes('item.count'))
    assert.ok(!transform.includes('peer.count'))
  }
})

test('production traces keep every error and sample successful traffic', () => {
  const source = read('collector.posthog.yaml')
  assert.match(source, /tail_sampling:/)
  assert.match(source, /type: status_code/)
  assert.match(source, /status_codes: \[ERROR\]/)
  assert.match(source, /type: probabilistic/)
  assert.match(source, /sampling_percentage: 10/)
  assert.match(
    source,
    /processors: \[filter\/diagnostics, transform\/allowlist, tail_sampling, batch\]/,
  )
  assert.match(
    source,
    /logs:\n\s+receivers: \[otlp\]\n\s+processors: \[filter\/diagnostics, transform\/allowlist, batch\]/,
  )
})

test('repository preflight runs the collector privacy contract', () => {
  const preflight = readFileSync(join(HERE, '../../../scripts/architecture/check-engine-repository.mjs'), 'utf8')
  assert.match(preflight, /collector-privacy\.test\.mjs/)
})
