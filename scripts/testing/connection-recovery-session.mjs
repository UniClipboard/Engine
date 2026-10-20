import assert from 'node:assert/strict'

/** 沿用 Engine 启动恢复的会话；仅在仍锁定时执行一次显式解锁，不重试或吞掉错误。 */
export async function ensureSessionReady(host) {
  const restored = (await host.call('observe')).encryption?.session_ready
  assert.equal(typeof restored, 'boolean', 'session readiness query failed')
  if (!restored) {
    await host.call('unlock')
    assert.equal((await host.call('observe')).encryption?.session_ready, true, 'session did not become ready')
  }
  return restored
}
