#!/usr/bin/env node
// 校验随Engine传递的密码来源清单；变更必须同时有显式来源及局部差异记录。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { readFileSync, readdirSync } from 'node:fs'
import { join, relative, resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const expectedChanges = {
  'opaque-ke': ['src/lib.rs', 'src/key_exchange/group/elliptic_curve.rs', 'src/key_exchange/group/mod.rs', 'src/key_exchange/sigma_i/ecdsa.rs', 'src/key_exchange/sigma_i/mod.rs'],
  voprf: ['src/ciphersuite.rs', 'src/group/elliptic_curve.rs', 'src/group/mod.rs', 'src/group/ristretto.rs'],
}
const sha = file => createHash('sha256').update(readFileSync(file)).digest('hex')
function filesAt(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap(entry =>
    entry.isDirectory() ? filesAt(join(dir, entry.name)) : [join(dir, entry.name)])
}
for (const [name, changes] of Object.entries(expectedChanges)) {
  const dir = join(root, 'third_party', name)
  const record = JSON.parse(readFileSync(join(dir, 'UPSTREAM.json'), 'utf8'))
  assert.equal(record.crate, name)
  assert.match(record.registry_checksum, /^[0-9a-f]{64}$/)
  assert.deepEqual(Object.keys(record.modified_files).sort(), changes.sort())
  const actual = filesAt(dir).map(file => relative(dir, file).replaceAll('\\', '/'))
    .filter(path => path.startsWith('src/') || path === 'Cargo.toml' || path.startsWith('LICENSE') || path.startsWith('RFC9380-LICENSE')).sort()
  assert.deepEqual(actual, Object.keys(record.local_files).sort(), `${name}: source inventory changed`)
  for (const [path, hash] of Object.entries(record.local_files)) assert.equal(sha(join(dir, path)), hash, `${name}: ${path}`)
  for (const [path, hash] of Object.entries(record.original_files)) {
    if (Object.hasOwn(record.modified_files, path)) {
      assert.equal(record.local_files[path] ?? null, record.modified_files[path])
    } else {
      assert.equal(record.local_files[path], hash, `${name}: undocumented upstream source change`)
    }
  }
  const manifest = readFileSync(join(dir, 'Cargo.toml'), 'utf8')
  assert.ok(!manifest.includes('elliptic-curve') && !manifest.includes('crypto-bigint'))
}
// 逆转仅有的模块路径／无信息错误类型适配，证明消息展开算法与固定原版逐字节一致。
const voprf = join(root, 'third_party/voprf')
const expansion = JSON.parse(readFileSync(join(voprf, 'UPSTREAM.json'), 'utf8')).expand_message_origin
for (const [upstreamPath, expectedHash] of Object.entries(expansion.files)) {
  const filename = upstreamPath === 'expand_msg.rs' ? 'mod.rs' : upstreamPath.split('/').at(-1)
  let text = readFileSync(join(voprf, 'src/group/expand_msg', filename), 'utf8')
  if (filename === 'mod.rs') {
    text = text.replace('pub(super) use xmd::ExpandMsgXmd;\n', '')
      .replace('#[derive(Debug)]\npub(super) struct Error;\ntype Result<T> = core::result::Result<T, Error>;', 'use crate::{Error, Result};')
  } else {
    text = text.replace('use super::{Error, Result};', 'use crate::{Error, Result};')
  }
  assert.equal(createHash('sha256').update(text).digest('hex'), expectedHash, `RFC9380 algorithm changed: ${filename}`)
}
process.stdout.write('Pinned OPAQUE/VOPRF sources, licenses and documented differences verified\n')
