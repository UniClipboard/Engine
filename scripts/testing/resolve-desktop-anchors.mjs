#!/usr/bin/env node
// 从 Desktop 公开发布读取各版本锁定的 Engine rev，生成升级兼容矩阵的锚点清单与期望登记。
//
// 用法：
//   node scripts/testing/resolve-desktop-anchors.mjs            # 核对入库清单与 GitHub 当前发布一致
//   node scripts/testing/resolve-desktop-anchors.mjs --write    # 重新生成清单并补齐期望登记
//
// 只收录 GitHub 上非草稿的 Desktop v1.x 发布；同一 rev 被多个发布使用时只算一个锚点。

import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const MATRIX_DIR = join(REPOSITORY_ROOT, 'tests/upgrade-matrix')
const ANCHORS_PATH = join(MATRIX_DIR, 'anchors.json')
const LEGACY_ANCHORS_PATH = join(MATRIX_DIR, 'legacy-anchors.json')
const EXPECTATIONS_PATH = join(MATRIX_DIR, 'expectations.json')
const DESKTOP_REPOSITORY = 'UniClipboard/UniClipboard'
const ENGINE_GIT_URL = 'https://github.com/UniClipboard/Engine.git'
const RELEASE_TAG = /^v1\.\d+\.\d+(-(alpha|beta|rc)\.\d+)?$/
const ENGINE_PIN = /^uc-engine\s*=\s*\{[^}]*git\s*=\s*"([^"]+)"[^}]*rev\s*=\s*"([0-9a-f]{40})"/m
const DIMENSIONS = ['d1', 'd2', 'd3', 'd4']

function gh(args) {
  return execFileSync('gh', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 64 << 20 })
}

function compareVersions(left, right) {
  const parse = tag => {
    const [core, pre] = tag.slice(1).split('-')
    const [kind, number] = pre ? pre.split('.') : ['~', '0']
    return [...core.split('.').map(Number), kind, Number(number)]
  }
  const a = parse(left)
  const b = parse(right)
  for (let index = 0; index < a.length; index++) {
    if (a[index] < b[index]) return -1
    if (a[index] > b[index]) return 1
  }
  return 0
}

function publishedReleases() {
  const pages = JSON.parse(gh(['api', '--paginate', '--slurp', `repos/${DESKTOP_REPOSITORY}/releases?per_page=100`]))
  return pages
    .flat()
    .filter(release => !release.draft && RELEASE_TAG.test(release.tag_name))
    .map(release => ({ tag: release.tag_name, published_at: release.published_at }))
    .sort((left, right) => compareVersions(left.tag, right.tag))
}

function releasePin(tag) {
  const manifest = gh([
    'api',
    '-H',
    'Accept: application/vnd.github.raw',
    `repos/${DESKTOP_REPOSITORY}/contents/Cargo.toml?ref=${encodeURIComponent(tag)}`,
  ])
  const match = ENGINE_PIN.exec(manifest)
  if (!match) throw new Error(`Desktop ${tag} does not pin uc-engine to a git rev`)
  if (match[1] !== ENGINE_GIT_URL) throw new Error(`Desktop ${tag} pins uc-engine from an unexpected source`)
  const commit = gh(['api', `repos/${DESKTOP_REPOSITORY}/commits/${encodeURIComponent(tag)}`, '--jq', '.sha']).trim()
  return { engineRev: match[2], desktopCommit: commit }
}

function resolveAnchors() {
  const anchors = []
  for (const release of publishedReleases()) {
    const { engineRev, desktopCommit } = releasePin(release.tag)
    const entry = { tag: release.tag, commit: desktopCommit, published_at: release.published_at }
    const existing = anchors.find(anchor => anchor.engine_rev === engineRev)
    if (existing) {
      existing.desktop_releases.push(entry)
      continue
    }
    anchors.push({
      id: `a${String(anchors.length + 1).padStart(2, '0')}`,
      engine_rev: engineRev,
      desktop_releases: [entry],
    })
  }
  return {
    schema_version: 1,
    source: {
      desktop_repository: DESKTOP_REPOSITORY,
      manifest: 'Cargo.toml',
      dependency: 'uc-engine',
      release_filter: 'non-draft GitHub releases whose tag matches v1.x.y[-(alpha|beta|rc).N]',
      generator: 'scripts/testing/resolve-desktop-anchors.mjs',
    },
    anchors,
  }
}

// 矩阵只覆盖当前源码：每个已发布锚点到 head 的单元，加一条单设备经过全部可运行锚点到 head 的完整链。已发布锚点
// 之间的组合不随当前源码变化，不再展开；单元数随锚点数线性增长。
// 早于 Engine 的 Desktop 发布只以静态资料快照参与 D1，由人工维护，不从 GitHub 发布生成。
export function readLegacyAnchorIds() {
  return (readJson(LEGACY_ANCHORS_PATH)?.anchors ?? []).map(anchor => anchor.id)
}

export function expandCells(anchorIds, legacyIds = []) {
  const cells = []
  for (const from of legacyIds) {
    cells.push(`d1-${from}-head`)
    cells.push(`d1-${from}-head-commit-interrupted`)
  }
  for (const from of anchorIds) cells.push(`d1-${from}-head`)
  cells.push('d1-chain')
  for (const from of anchorIds) cells.push(`d2-${from}-head`)
  for (const from of anchorIds) {
    cells.push(`d3-${from}-head-old-inviter`)
    cells.push(`d3-${from}-head-new-inviter`)
  }
  for (const from of anchorIds) cells.push(`d4-${from}-head`)
  return cells
}

function expectationsFor(anchors, previous) {
  const known = new Map((previous?.cells ?? []).map(cell => [cell.cell, cell]))
  const cells = expandCells(
    anchors.anchors.map(anchor => anchor.id),
    readLegacyAnchorIds(),
  ).map(cell => {
    const recorded = known.get(cell)
    // 保留人工登记（非“通过”或带排除点与原因的条目）；其余单元初始期望为“通过”。
    return recorded && Object.keys(recorded).some(key => key !== 'cell' && key !== 'expected') ? recorded : recorded?.expected === 'pass' || !recorded ? { cell, expected: 'pass' } : recorded
  })
  return { schema_version: 1, cells }
}

function verifyRevisions(anchors) {
  const missing = []
  for (const anchor of anchors.anchors) {
    try {
      execFileSync('git', ['-C', REPOSITORY_ROOT, 'cat-file', '-e', `${anchor.engine_rev}^{commit}`], { stdio: 'ignore' })
    } catch {
      missing.push(`${anchor.id} ${anchor.engine_rev}`)
    }
  }
  return missing
}

function readJson(path) {
  return existsSync(path) ? JSON.parse(readFileSync(path, 'utf8')) : undefined
}

function serialize(value) {
  return `${JSON.stringify(value, null, 2)}\n`
}

function main() {
  const write = process.argv.includes('--write')
  const anchors = resolveAnchors()
  const expectations = expectationsFor(anchors, readJson(EXPECTATIONS_PATH))
  const missing = verifyRevisions(anchors)
  if (missing.length) {
    process.stderr.write(`Engine revisions not present locally (fetch them first): ${missing.join(', ')}\n`)
    process.exitCode = 1
  }
  const counts = Object.fromEntries(
    DIMENSIONS.map(dimension => [dimension, expectations.cells.filter(cell => cell.cell.startsWith(`${dimension}-`)).length]),
  )
  process.stdout.write(
    `anchors: ${anchors.anchors.length} (+ head); releases: ${anchors.anchors.reduce((sum, anchor) => sum + anchor.desktop_releases.length, 0)}; ` +
      `cells: ${expectations.cells.length} ${JSON.stringify(counts)}\n`,
  )
  if (write) {
    writeFileSync(ANCHORS_PATH, serialize(anchors))
    writeFileSync(EXPECTATIONS_PATH, serialize(expectations))
    return
  }
  const drift = []
  if (serialize(anchors) !== (existsSync(ANCHORS_PATH) ? readFileSync(ANCHORS_PATH, 'utf8') : '')) drift.push('anchors.json')
  if (serialize(expectations) !== (existsSync(EXPECTATIONS_PATH) ? readFileSync(EXPECTATIONS_PATH, 'utf8') : '')) {
    drift.push('expectations.json')
  }
  if (drift.length) {
    process.stderr.write(`committed ${drift.join(' and ')} differ from published Desktop releases; rerun with --write\n`)
    process.exitCode = 1
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main()
