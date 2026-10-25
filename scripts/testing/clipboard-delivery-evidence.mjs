// 只为隔离网络验收保留固定分类；不复制业务标识、内容或错误正文。
import assert from 'node:assert/strict'

const failurePrefixes = [
  ['peer rejected', 'peer_rejected'],
  ['stream io', 'stream_io'],
  ['internal', 'internal'],
  ['local policy rejected payload before dispatch', 'local_policy_rejected'],
  ['peer version is incompatible with confirmed clipboard delivery', 'incompatible_version'],
  ['target device offline or unreachable', 'unreachable'],
]

export function summarizeDeliveryReport(report) {
  const counts = Object.fromEntries(
    ['total_accepted', 'total_offline', 'total_errored', 'total_pending', 'total_duplicate']
      .filter(key => Number.isSafeInteger(report[key]) && report[key] >= 0)
      .map(key => [key, report[key]])
  )
  const causes = (report.per_target ?? []).map(target => {
    const outcome = target.outcome ?? {}
    const message = typeof outcome.message === 'string' ? outcome.message : ''
    return failurePrefixes.find(([prefix]) => message.startsWith(prefix))?.[1]
      ?? (['accepted', 'duplicate', 'error'].includes(outcome.kind) ? outcome.kind : 'unknown')
  })
  return { ...counts, causes }
}

// 发送返回与精确读回分开记录；拒绝和读回失败保留原断言与原期限。
export async function verifyDelivery(evidence, { kind, from, to, scenario }, send, readback) {
  const attempt = { kind, from, to, scenario, stage: 'dispatch' }
  evidence.push(attempt)
  let started = performance.now()
  try {
    let report
    try { report = await send() }
    finally { attempt.dispatch_ms = Math.round(performance.now() - started) }
    Object.assign(attempt, summarizeDeliveryReport(report))
    assert.equal(report.total_accepted, 1,
      `${from} to ${to}: ${kind} not accepted (offline=${attempt.total_offline}, failed=${attempt.total_errored}, pending=${attempt.total_pending}, duplicate=${attempt.total_duplicate}, causes=${attempt.causes.join(',')})`)
    attempt.stage = 'readback'
    started = performance.now()
    try { await readback() }
    finally { attempt.readback_ms = Math.round(performance.now() - started) }
    attempt.stage = 'verified'
  } catch (error) {
    attempt.failed = true
    throw error
  }
}
