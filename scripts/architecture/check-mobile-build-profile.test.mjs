import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'

const root = resolve(import.meta.dirname, '../..')

function writeExecutable(path, contents) {
  writeFileSync(path, contents)
  chmodSync(path, 0o755)
}

// 执行真实打包脚本，以固定工具替身覆盖完整流程，避免测试依赖 SDK 或下载。
function run(
  t,
  platform,
  profile,
  separateSource = false,
  complete = false,
  extraEnv = {},
  bindingSource = 'binding',
) {
  const fixture = mkdtempSync(join(tmpdir(), 'uc-mobile-profile-'))
  t.after(() => rmSync(fixture, { recursive: true, force: true }))
  const scripts = join(fixture, 'bindings/uc-engine-uniffi/scripts')
  const bin = join(fixture, 'bin')
  mkdirSync(scripts, { recursive: true })
  mkdirSync(bin)
  const name = platform === 'ios' ? 'build-ios-xcframework.sh' : 'build-android-aar.sh'
  copyFileSync(join(root, 'bindings/uc-engine-uniffi/scripts', name), join(scripts, name))
  writeExecutable(join(bin, 'uname'), '#!/bin/sh\necho Darwin\n')
  writeExecutable(join(bin, 'cargo'), `#!/bin/bash
set -eu
test "$PWD" = "$PROFILE_TEST_SOURCE" || exit 78
printf '%s\\n' "$*" >> "$PROFILE_TEST_LOG"
if [[ "$1" == metadata ]]; then
  printf '{"packages":[{"name":"rustls-platform-verifier-android","manifest_path":"%s/rustls/Cargo.toml","version":"1.0.0"}]}\n' "$PROFILE_TEST_SOURCE"
  exit 0
fi
if [[ "$1" == pkgid ]]; then echo 'fixture#1.2.3'; exit 0; fi
if [[ "$1" == ndk ]]; then
  if [[ "$PROFILE_TEST_COMPLETE" != 1 ]]; then exit 77; fi
  mkdir -p "$CARGO_TARGET_DIR/aarch64-linux-android/debug" "$CARGO_TARGET_DIR/x86_64-linux-android/debug"
  touch "$CARGO_TARGET_DIR/aarch64-linux-android/debug/libuc_engine_uniffi.so"
  touch "$CARGO_TARGET_DIR/x86_64-linux-android/debug/libuc_engine_uniffi.so"
  exit 0
fi
if [[ "$*" == *"--target"* ]]; then
  if [[ "$PROFILE_TEST_COMPLETE" != 1 ]]; then exit 77; fi
  target=''
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == --target ]]; then target="$2"; break; fi
    shift
  done
  mkdir -p "$CARGO_TARGET_DIR/$target/debug"
  touch "$CARGO_TARGET_DIR/$target/debug/libuc_engine_uniffi.a"
  exit 0
fi
if [[ "$1" == run ]]; then
  while [[ "$1" != --out-dir ]]; do shift; done
  mkdir -p "$2"
  touch "$2/uc_engine_uniffiFFI.h" "$2/uc_engine_uniffiFFI.modulemap" "$2/uc_engine_uniffi.swift"
  mkdir -p "$2/uniffi/uc_engine_uniffi"
  touch "$2/uniffi/uc_engine_uniffi/uc_engine_uniffi.kt"
fi
`)
  writeExecutable(join(bin, 'git'), `#!/bin/sh
if [ "$1" = rev-parse ]; then printf '%040d\n' 0; fi
`)
  writeExecutable(join(bin, 'xcodebuild'), `#!/bin/bash
set -eu
while [[ "$1" != -output ]]; do shift; done
mkdir -p "$2"
`)
  writeExecutable(join(bin, 'ditto'), '#!/bin/bash\nset -eu\ntouch "${@: -1}"\n')
  writeExecutable(join(bin, 'unzip'), `#!/bin/sh
if [ "$1" = -p ]; then echo classes; else echo libs/rustls-platform-verifier-classes.jar; fi
`)
  writeExecutable(join(bin, 'jar'), '#!/bin/sh\necho org/rustls/platformverifier/CertificateVerifier.class\n')
  const env = { ...process.env, ...extraEnv, PATH: `${bin}:${process.env.PATH}`,
    UC_ENGINE_UNIFFI_TARGET_DIR: join(fixture, 'target'),
    UC_ENGINE_UNIFFI_DIST_DIR: join(fixture, 'dist'),
    UC_ENGINE_UNIFFI_SLICE: 'device', PROFILE_TEST_LOG: join(fixture, 'commands'),
    PROFILE_TEST_COMPLETE: complete ? '1' : '0',
    PROFILE_TEST_SOURCE: separateSource ? join(fixture, 'pinned-source') : fixture }
  mkdirSync(env.PROFILE_TEST_SOURCE, { recursive: true })
  mkdirSync(join(env.PROFILE_TEST_SOURCE, 'bindings/uc-engine-uniffi/android'), { recursive: true })
  mkdirSync(join(env.PROFILE_TEST_SOURCE, 'tests/hosts/android'), { recursive: true })
  mkdirSync(join(env.PROFILE_TEST_SOURCE, 'rustls/maven/rustls/rustls-platform-verifier/1.0.0'), { recursive: true })
  writeFileSync(join(env.PROFILE_TEST_SOURCE, 'rustls/maven/rustls/rustls-platform-verifier/1.0.0/rustls-platform-verifier-1.0.0.aar'), '')
  writeExecutable(join(env.PROFILE_TEST_SOURCE, 'tests/hosts/android/gradlew'), `#!/bin/bash
set -eu
printf 'gradle %s\\n' "$*" >> "$PROFILE_TEST_LOG"
mkdir -p "$UC_ENGINE_UNIFFI_GRADLE_BUILD_DIR/outputs/aar"
touch "$UC_ENGINE_UNIFFI_GRADLE_BUILD_DIR/outputs/aar/UniClipboardEngine-debug.aar"
`)
  for (const file of ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'bindings/uc-engine-uniffi/Cargo.toml', 'bindings/uc-engine-uniffi/src/lib.rs']) {
    const path = join(env.PROFILE_TEST_SOURCE, file)
    mkdirSync(resolve(path, '..'), { recursive: true })
    writeFileSync(path, file.endsWith('/src/lib.rs') ? bindingSource : file)
  }
  delete env.UC_ENGINE_UNIFFI_BUILD_PROFILE
  if (profile !== undefined) env.UC_ENGINE_UNIFFI_BUILD_PROFILE = profile
  const args = [join(scripts, name), ...(separateSource ? [env.PROFILE_TEST_SOURCE] : [])]
  const result = spawnSync('bash', args, { env, encoding: 'utf8' })
  const dist = join(fixture, 'dist', platform)
  return { result, dist, commands: () => readFileSync(env.PROFILE_TEST_LOG, 'utf8') }
}

test('iOS 相同公开接口复用绑定，只重新编译设备库', (t) => {
  const bindingCache = join(tmpdir(), `uc-mobile-bindings-${process.pid}-${Date.now()}`)
  t.after(() => rmSync(bindingCache, { recursive: true, force: true }))
  const first = run(t, 'ios', 'dev', false, false, {
    UC_ENGINE_UNIFFI_BINDINGS_CACHE_DIR: bindingCache,
  })
  assert.equal(first.result.status, 77, first.result.stdout + first.result.stderr)

  const second = run(t, 'ios', 'dev', false, false, {
    UC_ENGINE_UNIFFI_BINDINGS_CACHE_DIR: bindingCache,
  })
  assert.equal(second.result.status, 77, second.result.stdout + second.result.stderr)
  assert.equal(second.commands().trim().split('\n').length, 1)

  const changed = run(t, 'ios', 'dev', false, false, {
    UC_ENGINE_UNIFFI_BINDINGS_CACHE_DIR: bindingCache,
  }, 'changed binding')
  assert.equal(changed.result.status, 77, changed.result.stdout + changed.result.stderr)
  assert.equal(changed.commands().trim().split('\n').length, 3)
})

for (const platform of ['ios', 'android']) {
  test(`${platform} 当前打包工具编译指定的旧版源码`, (t) => {
    const { result, commands } = run(t, platform, 'dev', true)
    assert.equal(result.status, 77, result.stdout + result.stderr)
    assert.match(commands().trim().split('\n')[2], /--profile dev/)
  })
  for (const profile of ['dev', 'release', undefined]) {
    test(`${platform} 构建选择 ${profile ?? '默认 release'}`, (t) => {
      const { result, commands } = run(t, platform, profile)
      assert.equal(result.status, 77, result.stdout + result.stderr)
      const lines = commands().trim().split('\n')
      assert.match(lines[0], /--profile dev/)
      assert.match(lines[1], /--profile dev/)
      assert.match(lines[2], new RegExp(`--profile ${profile ?? 'release'}(?: |$)`))
      assert.doesNotMatch(lines[2], /--release/)
    })
  }
  test(`${platform} 拒绝未知构建类型`, (t) => {
    const { result } = run(t, platform, 'typo')
    assert.equal(result.status, 1)
    assert.match(result.stderr, /must be dev or release/)
  })
  test(`${platform} 开发构建完成并标记产物类型`, (t) => {
    const { result, dist, commands } = run(t, platform, 'dev', false, true)
    assert.equal(result.status, 0, result.stdout + result.stderr)
    assert.equal(readFileSync(join(dist, 'build-profile.txt'), 'utf8'), 'dev\n')
    const artifact = platform === 'ios'
      ? 'UniClipboardEngine.xcframework.zip'
      : 'UniClipboardEngine.aar'
    assert.equal(existsSync(join(dist, artifact)), true)
    if (platform === 'android') assert.match(commands(), /gradle .*assembleDebug/)
  })
}
