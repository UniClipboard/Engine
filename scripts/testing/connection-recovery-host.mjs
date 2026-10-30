import { createInterface } from 'node:readline'

// 请求、进程退出和管道错误共用一个结算入口，避免清理时的二次错误中断证据落盘。
export function connectHostCommands(child, timeoutMs = 130_000) {
  const pending = []
  let failure
  const fail = error => {
    failure ??= error
    for (const request of pending.splice(0)) {
      clearTimeout(request.timer)
      request.reject(failure)
    }
  }
  child.stdin.on('error', error => fail(new Error('test host input failed', { cause: error })))
  child.on('error', error => fail(new Error('test host process failed', { cause: error })))
  child.on('exit', () => fail(new Error('test host exited before replying')))
  createInterface({ input: child.stdout }).on('line', line => {
    let response
    try { response = JSON.parse(line) } catch { return }
    if (response.uc_connectivity !== 1) return
    const request = pending.shift()
    if (!request) return
    clearTimeout(request.timer)
    request.resolve(response)
  })
  return {
    pending,
    raw(request) {
      if (failure) return Promise.reject(failure)
      if (child.exitCode !== null || child.signalCode !== null || child.stdin.destroyed) {
        return Promise.reject(new Error('test host is not running'))
      }
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          fail(new Error('test host command timed out'))
          child.kill('SIGTERM')
        }, timeoutMs)
        pending.push({ resolve, reject, timer })
        try { child.stdin.write(`${JSON.stringify(request)}\n`) }
        catch (error) { fail(new Error('test host input failed', { cause: error })) }
      })
    },
  }
}
