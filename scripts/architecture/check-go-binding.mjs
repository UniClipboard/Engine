// Go binding 的静态约束：生成器来源固定、模板补丁与声明一致、许可证随生成包、生成文件不含手写入口。
// 重新生成并比较零差异由 go-binding 工作流执行（需要构建生成器与原生库）。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const goDir = join(root, 'bindings/go')
const sha256 = (path) => createHash('sha256').update(readFileSync(path)).digest('hex')

const pin = Object.fromEntries(
  readFileSync(join(goDir, 'generator/PIN.env'), 'utf8')
    .split('\n')
    .filter((line) => line && !line.startsWith('#'))
    .map((line) => line.split(/=(.*)/s).slice(0, 2)),
)
assert.match(pin.UNIFFI_BINDGEN_GO_REVISION, /^[0-9a-f]{40}$/, 'generator revision must be a full commit')
assert.match(pin.UNIFFI_BINDGEN_GO_LOCK_SHA256, /^[0-9a-f]{64}$/)
assert.equal(
  sha256(join(goDir, 'generator', pin.UNIFFI_BINDGEN_GO_PATCH)),
  pin.UNIFFI_BINDGEN_GO_PATCH_SHA256,
  'template patch differs from PIN.env',
)

const generated = join(goDir, 'uc_engine_uniffi')
assert.ok(existsSync(join(generated, 'uc_engine_uniffi.go')) && existsSync(join(generated, 'uc_engine_uniffi.h')))
assert.match(readFileSync(join(generated, 'LICENSE-uniffi-bindgen-go'), 'utf8'), /Mozilla Public License Version 2\.0/)
// 生成目录只允许两个生成文件、手写 link.go 与许可证；其余入口说明有人把手写代码混入生成包。
assert.deepEqual(
  readdirSync(generated).sort(),
  ['LICENSE-uniffi-bindgen-go', 'link.go', 'uc_engine_uniffi.go', 'uc_engine_uniffi.h'],
)

// go 指令不得高于 Desktop 声明的 1.26，避免把消费者工具链强行抬高。
const goMod = readFileSync(join(goDir, 'go.mod'), 'utf8')
const directive = /^go (\d+)\.(\d+)/m.exec(goMod)
assert.ok(directive && Number(directive[1]) === 1 && Number(directive[2]) <= 26, 'go directive must be <= 1.26')

// 门面不得绕过稳定入口：只允许依赖生成包与 native 校验。
for (const file of readdirSync(join(goDir, 'engine')).filter((name) => name.endsWith('.go'))) {
  const text = readFileSync(join(goDir, 'engine', file), 'utf8')
  for (const [, path] of text.matchAll(/"(github\.com\/UniClipboard\/[^"]+)"/g)) {
    assert.ok(
      path === 'github.com/UniClipboard/Engine/bindings/go/uc_engine_uniffi' ||
        path === 'github.com/UniClipboard/Engine/bindings/go/native',
      `engine/${file} imports ${path}`,
    )
  }
}
console.log('Go binding static checks passed')
