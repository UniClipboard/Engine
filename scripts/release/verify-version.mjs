#!/usr/bin/env node

import { execFileSync } from 'node:child_process'
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

process.stdout.write(`${packageMetadata.version}\n`)
