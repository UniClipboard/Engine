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
})

test('uses a GitHub App installation token for both repository dispatches', () => {
  assert.match(workflow, /actions\/create-github-app-token@v3/)
  assert.match(workflow, /ENGINE_RELEASE_APP_CLIENT_ID/)
  assert.match(workflow, /ENGINE_RELEASE_APP_PRIVATE_KEY/)
  assert.match(workflow, /permission-contents:\s*write/)
  assert.match(workflow, /for repository in UniClipboard\/UniClipboard UniClipboard\/UniClip/)
  assert.match(workflow, /repos\/\$repository\/dispatches/)
  assert.doesNotMatch(workflow, /PERSONAL_ACCESS_TOKEN|PAT/)
})
