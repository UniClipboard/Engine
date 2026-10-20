import { once } from 'node:events'
import { setTimeout as delay } from 'node:timers/promises'

// connection-recovery-network.mjs 中等待双方重新在线的预期超时文案；只有这一种失败可以被记录为“未在期限内恢复”。
export const ONLINE_DEADLINE_MESSAGE = 'automatic connection exceeded its deadline'

/** 返回本轮真正建立 Relay 传输的发生时间；TCP 探测连接不能证明协议就绪。 */
export function relayTransportReadyAt(tcpEstablished, observations, restartedAt) {
  if (!tcpEstablished) return undefined
  const statuses = observations
    .filter(row => row.fields?.['event.name'] === 'relay.status.observed')
    .map(row => ({ at: Date.parse(row.timestamp), connected: row.fields.connected_count > 0 }))
    .filter(row => Number.isFinite(row.at) && row.at >= restartedAt)
    .sort((left, right) => left.at - right.at)
  const latest = statuses.at(-1)
  return latest?.connected ? latest.at : undefined
}

/**
 * 在期限内等待恢复，返回耗时毫秒；仅当失败正是预期的期限超时才返回 null，
 * 宿主退出、命令错误、中断等其他失败一律继续抛出，避免把环境故障记成“没有恢复”。
 */
export async function measureRecoveryMs(waitOnline, startedAt, now = () => performance.now()) {
  try {
    await waitOnline()
  } catch (error) {
    if (error?.message === ONLINE_DEADLINE_MESSAGE) return null
    throw error
  }
  return Math.round(now() - startedAt)
}

/**
 * 关闭本地 relay 进程：已提前退出则明确报错（不再永久等待 exit 事件），
 * 正常关闭用 SIGINT 并限时等待，超时改用 SIGKILL 并报错。
 */
export async function terminateRelay(relay, { timeoutMs = 5000 } = {}) {
  if (relay.exitCode !== null || relay.signalCode !== null) {
    throw new Error(`local relay exited before shutdown (exit code ${relay.exitCode}, signal ${relay.signalCode})`)
  }
  const exited = once(relay, 'exit')
  relay.kill('SIGINT')
  const outcome = await Promise.race([exited.then(() => 'exited'), delay(timeoutMs).then(() => 'timeout')])
  if (outcome === 'timeout') {
    relay.kill('SIGKILL')
    await exited
    throw new Error(`local relay did not shut down within ${timeoutMs} ms`)
  }
  if (relay.exitCode !== 0) throw new Error('local relay failed to shut down')
}
