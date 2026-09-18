#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: $0 [--suite all|local|network] [--repeat N] [--mode all|direct|known-peer|relay|legacy] [--case PREFIX]"
}

suite=all
repeat=3
mode=all
case_prefix=
while (($#)); do
  case "$1" in
    --suite) suite=$2; shift 2 ;;
    --repeat) repeat=$2; shift 2 ;;
    --mode) mode=$2; shift 2 ;;
    --case) case_prefix=$2; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done
[[ "$repeat" =~ ^[1-9][0-9]*$ ]] || exit 2
[[ "$suite" == all || "$suite" == local || "$suite" == network ]] || exit 2
[[ "$mode" == all || "$mode" == direct || "$mode" == known-peer || "$mode" == relay || "$mode" == legacy ]] || exit 2
[[ "$suite" != local || "$mode" == all ]] || { echo '--mode only applies to network validation.' >&2; exit 2; }
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"

if [[ "$suite" != network ]]; then
  for ((run=1; run<=repeat; run++)); do
    cargo test -p uc-infra --lib --locked peer_reachability -- --test-threads=1
    cargo test -p uc-infra --lib --locked protocol_router -- --test-threads=1
    cargo test -p uc-infra --lib --locked rejecting_new_dials_keeps_established_streams_usable -- --test-threads=1
    cargo test -p uc-application --lib --locked space::connectivity -- --test-threads=1
    cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e --locked -- automatic_connections::existing_connections_survive_rejected_new_dials --test-threads=1
    cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e --locked -- automatic_connections::failed_content_dial_preserves_peer_connection --test-threads=1
  done
fi
[[ "$suite" != local ]] || exit 0
[[ $(uname -s) == Linux ]] || { echo 'Network validation requires Linux.' >&2; exit 2; }

cargo build -p uc-connectivity-host -p uc-connectivity-relay --locked
target=$(cargo metadata --locked --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')
evidence="$target/connection-recovery-evidence"
mkdir -p "$evidence"
legacy_revision=f6f305d9689e4e79e7ab6d0e4921061f9416e4a6
legacy=$(mktemp -d "${TMPDIR:-/tmp}/uc-connectivity-rc15.XXXXXX")
cleanup() {
  local status=$?
  rm -rf -- "$legacy"
  if ((EUID != 0)); then
    sudo chown -R -- "$(id -u):$(id -g)" "$evidence" || status=1
  fi
  exit "$status"
}
trap cleanup EXIT
if [[ "$mode" == all || "$mode" == legacy ]]; then
  git archive "$legacy_revision" | tar -x -C "$legacy"
  cp -R tests/hosts/connectivity "$legacy/tests/hosts/connectivity"
  git -C "$legacy" apply "$repo/scripts/testing/rc15-test-host.patch"
  CARGO_TARGET_DIR="$target/rc15" cargo build --manifest-path "$legacy/Cargo.toml" -p uc-connectivity-host --no-default-features --offline
fi

git rev-parse HEAD > "$evidence/current-revision.txt"
printf '%s\n' "$legacy_revision" > "$evidence/legacy-revision.txt"
git diff --binary | shasum -a 256 > "$evidence/working-diff-sha256.txt"
node --input-type=module - "$evidence" <<'NODE'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, lstatSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

const paths = execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], { encoding: 'utf8' }).split('\0').filter(Boolean)
const digest = createHash('sha256')
for (const path of [...new Set(paths)].sort()) {
  if (!existsSync(path) || !lstatSync(path).isFile()) continue
  digest.update(path).update('\0').update(createHash('sha256').update(readFileSync(path)).digest())
}
writeFileSync(join(process.argv[2], 'source-tree-sha256.txt'), `${digest.digest('hex')}\n`)
NODE
rustc --version > "$evidence/rust-version.txt"
node --version > "$evidence/node-version.txt"
runner=(node "$repo/scripts/testing/connection-recovery-network.mjs" --host "$target/debug/uc-connectivity-host" --repeat "$repeat" --evidence "$evidence")
if [[ -n "$case_prefix" ]]; then runner+=(--case "$case_prefix"); fi
if ((EUID != 0)); then runner=(sudo -- "${runner[@]}"); fi
if [[ "$mode" == all || "$mode" == direct ]]; then "${runner[@]}" --mode direct; fi
if [[ "$mode" == all || "$mode" == known-peer ]]; then "${runner[@]}" --mode known-peer; fi
if [[ "$mode" == all || "$mode" == relay ]]; then "${runner[@]}" --mode relay --relay "$target/debug/uc-connectivity-relay"; fi
if [[ "$mode" == all || "$mode" == legacy ]]; then
  "${runner[@]}" --mode legacy --legacy-host "$target/rc15/debug/uc-connectivity-host" --legacy-side 0
fi
