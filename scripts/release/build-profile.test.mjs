import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'

const stageRelease = resolve(import.meta.dirname, 'stage-engine-release.mjs')

function run(t, profiles) {
  const fixture = mkdtempSync(join(tmpdir(), 'uc-release-profile-'))
  t.after(() => rmSync(fixture, { recursive: true, force: true }))
  const dist = join(fixture, 'dist')
  for (const platform of ['ios', 'android', 'ohos']) {
    const directory = join(dist, platform)
    mkdirSync(directory, { recursive: true })
    writeFileSync(join(directory, 'version.txt'), 'v1.0.0\n')
    writeFileSync(join(directory, 'source-commit.txt'), `${'a'.repeat(40)}\n`)
    if (profiles[platform] !== undefined) {
      writeFileSync(join(directory, 'build-profile.txt'), `${profiles[platform]}\n`)
    }
  }
  const deviceMatrix = join(fixture, 'device-matrix.json')
  writeFileSync(deviceMatrix, '{}\n')
  return spawnSync(process.execPath, [stageRelease, dist, deviceMatrix], {
    cwd: fixture,
    encoding: 'utf8',
  })
}

for (const platform of ['ios', 'android']) {
  test(`正式发布拒绝 ${platform} 开发版产物`, (t) => {
    const result = run(t, { ios: 'release', android: 'release', [platform]: 'dev' })
    assert.equal(result.status, 1)
    assert.match(result.stderr, new RegExp(`${platform} build profile must be release`))
  })

  test(`正式发布拒绝缺少 ${platform} 构建类型的产物`, (t) => {
    const profiles = { ios: 'release', android: 'release' }
    delete profiles[platform]
    const result = run(t, profiles)
    assert.equal(result.status, 1)
    assert.match(result.stderr, new RegExp(`${platform} build profile is missing`))
  })
}

test('正式版产物通过构建类型检查', (t) => {
  const result = run(t, { ios: 'release', android: 'release' })
  assert.equal(result.status, 1)
  assert.doesNotMatch(result.stderr, /build profile/)
  assert.match(result.stderr, /platform artifact is missing/)
})
