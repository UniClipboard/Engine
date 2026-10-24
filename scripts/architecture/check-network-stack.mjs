#!/usr/bin/env node
// 网络栈源码与日志调用点审查绑定；升级 gitlink 时须同步审查此提交。
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const vendor = 'third_party/iroh'
const reviewedCommit = '4f388e14ddf44b34b17ef870d56d41053fd95301'
const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim()
if (!existsSync(resolve(root, vendor, 'iroh/Cargo.toml'))) {
  throw new Error('网络栈 submodule 未初始化：运行 git submodule update --init --recursive')
}
const staged = git('ls-files', '--stage', '--', vendor).split(/\s+/)
if (staged[0] !== '160000' || staged[1] !== reviewedCommit || git('-C', vendor, 'rev-parse', 'HEAD') !== reviewedCommit) {
  throw new Error('网络栈 gitlink / checkout 与已审查提交不一致；升级时须复核网络日志调用点')
}
if (git('-C', vendor, 'status', '--porcelain')) {
  throw new Error('网络栈 submodule 存在未提交修改')
}
const manifest = readFileSync(resolve(root, 'Cargo.toml'), 'utf8')
if (/^\[patch\./m.test(manifest)) {
  throw new Error('Engine 不得依赖不会向宿主传递的根补丁')
}
process.stdout.write('Network stack submodule provenance passed\n')
