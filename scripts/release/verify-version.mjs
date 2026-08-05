#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import process from 'node:process'

const [tag, packageName = 'uc-engine'] = process.argv.slice(2)
if (!tag) throw new Error('usage: verify-version.mjs <tag> [package-name]')

const prefix = packageName === 'uc-mobile' ? 'uc-mobile-v' : 'v'
if (!tag.startsWith(prefix)) throw new Error(`tag must start with ${prefix}`)

function parseJson(input, source) {
  try {
    return JSON.parse(input)
  } catch (error) {
    throw new Error(`invalid JSON from ${source}`, { cause: error })
  }
}

const metadata = parseJson(
  execFileSync('cargo', ['metadata', '--no-deps', '--locked', '--format-version', '1'], {
    encoding: 'utf8',
  }),
  'cargo metadata'
)
const packageMetadata = metadata.packages.find(candidate => candidate.name === packageName)
if (!packageMetadata) throw new Error(`workspace package is missing: ${packageName}`)

const expected = `${prefix}${packageMetadata.version}`
if (tag !== expected) throw new Error(`tag ${tag} does not match ${packageName} version ${expected}`)

if (packageName === 'uc-engine') {
  const harmonyPackage = readFileSync('tests/hosts/ohos/engine/oh-package.json5', 'utf8')
  const match = harmonyPackage.match(/^\s*(?:"version"|version)\s*:\s*['"]([^'"]+)['"],?\s*$/m)
  if (!match) throw new Error('HarmonyOS package version is missing')
  if (match[1] !== packageMetadata.version) {
    throw new Error(
      `HarmonyOS package version ${match[1]} does not match ${packageName} version ${packageMetadata.version}`
    )
  }
}

process.stdout.write(`${packageMetadata.version}\n`)
