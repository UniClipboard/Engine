// 固定矩阵的证据门禁：缺格、错误版本、假终态或缺少实际路径均不能通过。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, writeFileSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

export function verifyProgressCell(result, { head, oldRevision, side, mode }) {
  const legacy = oldRevision !== undefined
  assert.equal(result.failed, false)
  assert.equal(result.cleaned, true)
  assert.equal(result.plaintext_clean, true)
  assert.equal(result.progress_baseline, true)
  const records = new Map(result.records.map(row => [row.id, row]))
  assert.equal(records.size, result.records.length, 'duplicate scenario evidence')
  const row = records.get(legacy ? 'P00-prepaired-upgrade' : 'E02-file-transfer')
  assert.equal(row?.outcome, 'passed')
  const proof = row.proof
  assert.equal(proof.actual_handshake_observed, true)
  assert.equal(proof.negotiated_protocol, 'blob')
  assert(proof.paths.length > 0 && proof.paths.every(path => path.selected_path === mode))
  assert.equal(proof.hosts.length, 2)
  const current = proof.hosts.filter(host => host.anchor === 'progress-current')
  assert.equal(current.length, legacy ? 1 : 2)
  assert(current.every(host => host.engine_rev === head && host.capabilities.includes('transfer-progress')))
  if (legacy) {
    assert.equal(proof.hosts[side].anchor, 'progress-current')
    assert.equal(proof.hosts[1 - side].engine_rev, oldRevision)
    assert(!proof.hosts[1 - side].capabilities.includes('transfer-progress'))
    assert.equal(records.get('P00-prepaired-upgrade-old-pairing')?.outcome, 'passed')
    assert.equal(proof.original_received_files_readable, true)
    assert.equal(proof.exact_bidirectional_files, true)
    assert.equal(proof.independent_original_copies, true)
    assert.equal(proof.retained.length, 2)
    assert(proof.retained.every(node => ['identity', 'transport_identity', 'members', 'content'].every(key => node[key] === true)))
  } else {
    assert.equal(proof.exact_bytes_verified, true)
    assert.equal(proof.direction_count, 2)
  }
  if (mode === 'relay') {
    const blocked = result.faults.filter(fault => fault.action === 'direct_paths_blocked')
    for (const node of ['A', 'B']) assert(blocked.some(fault => fault.node === node && fault.ipv4_drop_packets > 0 && fault.ipv6_drop_packets > 0))
  }
  const transfers = result.progress_transfers
  assert.equal(transfers.length, 2)
  assert.deepEqual(new Set(transfers.map(value => `${value.sender}:${value.receiver}`)), new Set(['A:B', 'B:A']))
  for (const transfer of transfers) {
    const expectedSender = !legacy || transfer.sender === (side === 0 ? 'A' : 'B')
    assert.equal(transfer.new_sender, expectedSender)
    assert.equal(transfer.new_receiver, !legacy || !expectedSender)
    assert.equal(transfer.negotiated_protocol, legacy ? 'transfer_progress_v1' : 'transfer_progress_v2')
    assert.equal(transfer.actual_handshake_observed, true)
    assert.equal(transfer.actual_path_observed, true)
    assert.equal(transfer.selected_path, mode)
    const observedAt = transfer.new_sender ? transfer.sender : transfer.receiver
    const facts = result.nodes.find(node => node.label === observedAt)?.network_facts ?? []
    assert(facts.some(fact => fact['event.name'] === 'connection.protocol.negotiated'
      && fact.protocol === transfer.negotiated_protocol && fact.initial_path === mode), 'raw handshake/path observation is missing')
    if (legacy) assert(!facts.some(fact => fact.protocol === 'transfer_progress_v2'))
    if (transfer.new_receiver) assert.equal(transfer.local_receiver_completed, true)
    if (transfer.new_sender) {
      assert.deepEqual(transfer.source, legacy
        ? { state: 'unconfirmed', freshness: 'unconfirmed', completed: 0, unconfirmed: 1, total: 1, reconciliation: 'unsupported' }
        : { state: 'completed', freshness: 'settled', completed: 1, unconfirmed: 0, total: 1, reconciliation: 'settled' })
    } else {
      assert.equal(transfer.frozen_sender_query, 'not_required')
      assert.equal(transfer.no_fabricated_outbound, true)
      assert.equal(transfer.source, undefined)
    }
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = resolve(process.argv[2])
  const sources = JSON.parse(readFileSync(join(root, 'sources.json')))
  const labels = readFileSync(join(root, 'completed-labels.txt'), 'utf8').trim().split('\n')
  const expected = ['new-new-direct', 'new-new-relay', ...['main', 'history'].flatMap(anchor => ['direct', 'relay'].flatMap(mode => [0, 1].map(side => `${anchor}-${mode}-side${side}`)))]
  assert.equal(labels.length, 10)
  assert.deepEqual(new Set(labels), new Set(expected))
  const cases = []
  for (const label of labels) {
    const directory = join(root, label)
    const files = readdirSync(directory).filter(name => name.endsWith('.json'))
    assert.equal(files.length, 1)
    const bytes = readFileSync(join(directory, files[0]))
    const result = JSON.parse(bytes)
    const legacy = !label.startsWith('new-new-')
    const anchor = label.startsWith('main-') ? 'main' : 'history'
    verifyProgressCell(result, { head: sources.head, mode: label.includes('-relay') ? 'relay' : 'direct',
      ...(legacy ? { oldRevision: sources.anchors[anchor], side: Number(label.at(-1)) } : {}) })
    assert(result.binaries.some(binary => binary.sha256 === sources.current_binary_sha256))
    cases.push({ case: label, passed: true, progress: result.progress_transfers,
      evidence_sha256: createHash('sha256').update(bytes).digest('hex'), binaries: result.binaries })
  }
  const baseline = JSON.parse(readFileSync(join(root, 'old-old/summary.json')))
  assert.equal(baseline.full_matrix, true)
  assert.equal(baseline.cases.length, 4)
  assert.deepEqual(new Set(baseline.cases.map(value => value.case)), new Set(['main-direct', 'main-relay', 'history-direct', 'history-relay']))
  assert(baseline.cases.every(value => value.passed === true))
  assert.deepEqual(new Set(baseline.cases.map(value => value.engine_revision)), new Set(Object.values(sources.anchors)))
  writeFileSync(join(root, 'summary.json'), JSON.stringify({ scope: 'production-transfer-progress-network', sources,
    new_and_mixed_cells: cases, fixed_old_old_cells: baseline.cases, completed_cells: 14, skipped_cells: [] }, null, 2) + '\n')
  process.stdout.write('14 fixed-version cells: exact files, progress ALPN/path and conservative legacy views passed\n')
}
