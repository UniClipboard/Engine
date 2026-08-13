#!/usr/bin/env node

import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../..')
const workflow = readFileSync(resolve(root, '.github/workflows/release-engine.yml'), 'utf8')

test('notifies consumers only after release publication and interop verification', () => {
  assert.match(workflow, /version-interop:[\s\S]*needs:\s*assemble/)
  assert.match(workflow, /notify-consumers:[\s\S]*needs:\s*version-interop/)
  assert.match(workflow, /if:\s*\$\{\{\s*needs\.assemble\.outputs\.published == 'true'\s*\}\}/)
  assert.match(workflow, /event_type:\s*["']engine_release_published["']/)
  assert.match(workflow, /test_pair_e2e\.sh/)
  assert.match(workflow, /-p\s+uc-daemon\s+--bin\s+uniclipd/)
  assert.match(workflow, /-p\s+uc-cli\s+--bin\s+uniclip/)
  assert.match(workflow, /reverify_existing_release/)
  assert.match(workflow, /commit=\$\(git rev-list -n 1 "\$tag"\)/)
  assert.match(workflow, /ALICE_CLI:\s*.*old[\s\S]*BOB_CLI:\s*.*new/)
  assert.match(workflow, /ALICE_CLI:\s*.*new[\s\S]*BOB_CLI:\s*.*old/)
  assert.match(workflow, /git clone --local desktop new-desktop/)
  assert.match(workflow, /Build a previous Engine-compatible desktop consumer/)
  assert.match(workflow, /previous_desktop_commit="\$candidate"/)
  assert.match(workflow, /git -C engine merge-base --is-ancestor "\$candidate_engine_commit" "\$PREVIOUS_COMMIT"/)
  assert.match(workflow, /&& cargo build \\\n+[\s\S]*--target-dir "\$RUNNER_TEMP\/old-target"/)
  assert.match(workflow, /No desktop consumer builds with previous Engine commit/)
  assert.match(workflow, /git clone --local desktop old-desktop/)
  assert.match(workflow, /fetch-depth:\s*0/)
  assert.doesNotMatch(workflow, /uses:\s*\S+@(?![0-9a-f]{40})/)
})

test('uses a GitHub App installation token for both repository dispatches', () => {
  assert.match(workflow, /actions\/create-github-app-token@bcd2ba49218906704ab6c1aa796996da409d3eb1/)
  assert.match(workflow, /client-id:\s*\$\{\{\s*secrets\.ENGINE_RELEASE_APP_CLIENT_ID\s*\}\}/)
  assert.match(workflow, /private-key:\s*\$\{\{\s*secrets\.ENGINE_RELEASE_APP_PRIVATE_KEY\s*\}\}/)
  assert.match(workflow, /permission-contents:\s*write/)
  assert.match(workflow, /for repository in UniClipboard\/UniClipboard UniClipboard\/UniClip/)
  assert.match(workflow, /repos\/\$repository\/dispatches/)
  assert.doesNotMatch(workflow, /app-id:|vars\.ENGINE_RELEASE_APP_CLIENT_ID|PERSONAL_ACCESS_TOKEN|PAT/)
})

test('builds mobile release assets in parallel with target-specific caches', () => {
  assert.match(workflow, /\n  ios:\n[\s\S]*?needs:\s*prepare/)
  assert.match(workflow, /\n  android:\n[\s\S]*?needs:\s*prepare/)
  assert.match(workflow, /shared-key:\s*engine-release-ios/)
  assert.match(workflow, /shared-key:\s*engine-release-android/)
  assert.match(workflow, /name:\s*ios-assets/)
  assert.match(workflow, /name:\s*android-assets/)
  assert.match(workflow, /assemble:[\s\S]*?needs:\s*\[prepare, ios, android, harmonyos\]/)
  assert.doesNotMatch(workflow, /ios-android:/)
})
