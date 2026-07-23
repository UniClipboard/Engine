#!/usr/bin/env node

import { createHash } from 'node:crypto'
import { readFileSync, readdirSync, statSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import process from 'node:process'

const [directoryArg, tag, commit] = process.argv.slice(2)
if (!directoryArg || !tag || !commit) {
  throw new Error('usage: build-lan-release-manifest.mjs <directory> <uc-mobile-v*> <commit>')
}
if (!/^uc-mobile-v\d+\.\d+\.\d+(?:-rc\.\d+)?$/.test(tag)) throw new Error(`invalid tag: ${tag}`)
if (!/^[0-9a-f]{40}$/.test(commit)) throw new Error(`invalid commit: ${commit}`)

const directory = resolve(directoryArg)
const artifacts = readdirSync(directory)
  .filter(name => name !== 'lan-release-manifest.json')
  .sort()
  .map(name => {
    const path = join(directory, name)
    if (!statSync(path).isFile()) throw new Error(`LAN release asset must be a file: ${name}`)
    return {
      name,
      size: statSync(path).size,
      sha256: createHash('sha256').update(readFileSync(path)).digest('hex'),
    }
  })

writeFileSync(
  join(directory, 'lan-release-manifest.json'),
  `${JSON.stringify({ schemaVersion: 1, tag, commit, channel: 'explicit-lan-compatibility', artifacts }, null, 2)}\n`
)
process.stdout.write(`Wrote LAN release manifest with ${artifacts.length} artifacts\n`)
