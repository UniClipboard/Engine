#!/usr/bin/env bash
set -euo pipefail

# 当前脚本与所有嵌套 Cargo 调用统一经由 MBX。
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../build-cache/env.sh"

usage() {
  echo "Usage: $0 [--suite all|local|network] [--repeat N] [--mode all|direct|known-peer|relay|legacy] [--case PREFIX] [--prebuilt] [--peer-host PATH --peer-side 0|1] [--relay-binary PATH] [--relay-b PATH] [--prepaired-host PATH --upgrade-side 0|1 --presence-role receive|initiate --presence-protocol peer_reachability_v1|peer_reachability_v2]"
  echo "  --repeat applies to the network scenarios only; the local suite always runs once."
  echo "  --peer-host runs one side of the direct, known-peer or relay scenarios with another version's test host; --relay-binary replaces the relay server; --relay-b gives the second node its own relay server (different home relays)."
  echo "  --blob-baseline adds old/old and upgraded bidirectional file readback, then ends before presence recovery."
  echo "  --old-pair-only ends after old/old pairing, profile reopen and bidirectional readback; requires --blob-baseline and matching --host/--prepaired-host."
  echo "  --host PATH selects the real host binary for both sides; --prebuilt reuses it."
  echo "  --prebuilt reuses the test host already built in the cargo target directory, such as by the workspace test build."
}

suite=all
repeat=3
mode=all
case_prefix=
prebuilt=0
blob_baseline=0
old_pair_only=0
host_binary=
peer_host=
peer_side=1
relay_binary=
relay_b=
prepaired_host=
upgrade_side=
presence_role=
presence_protocol=
while (($#)); do
  case "$1" in
    --suite) suite=$2; shift 2 ;;
    --repeat) repeat=$2; shift 2 ;;
    --mode) mode=$2; shift 2 ;;
    --case) case_prefix=$2; shift 2 ;;
    --prebuilt) prebuilt=1; shift ;;
    --host) host_binary=$2; shift 2 ;;
    --blob-baseline) blob_baseline=1; shift ;;
    --old-pair-only) old_pair_only=1; shift ;;
    --peer-host) peer_host=$2; shift 2 ;;
    --peer-side) peer_side=$2; shift 2 ;;
    --relay-binary) relay_binary=$2; shift 2 ;;
    --relay-b) relay_b=$2; shift 2 ;;
    --prepaired-host) prepaired_host=$2; shift 2 ;;
    --upgrade-side) upgrade_side=$2; shift 2 ;;
    --presence-role) presence_role=$2; shift 2 ;;
    --presence-protocol) presence_protocol=$2; shift 2 ;;
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

# 本地部分是确定性测试，只跑一轮；--repeat 只作用于真实网络场景。
# 三个包的测试在一次构建中完成特性解析，避免分次调用时按不同特性组合重复编译共同依赖。
if [[ "$suite" != network ]]; then
  cargo nextest run --profile ci --locked --test-threads 1 \
    -p uc-infra-p2p -p uc-application -p uc-engine --features uc-engine/dev-tools \
    --lib --test space_membership_auto_pairing_e2e \
    -E '(package(uc-infra-p2p) & kind(lib) & (test(peer_reachability) | test(protocol_router) | test(rejecting_new_dials_keeps_established_streams_usable)))
      | (package(uc-application) & kind(lib) & test(space::connectivity))
      | (package(uc-engine) & binary(space_membership_auto_pairing_e2e)
        & (test(=automatic_connections::existing_connections_survive_rejected_new_dials)
          | test(=automatic_connections::failed_content_dial_preserves_peer_connection)))'
fi
[[ "$suite" != local ]] || exit 0
[[ $(uname -s) == Linux ]] || { echo 'Network validation requires Linux.' >&2; exit 2; }

hosts=(-p uc-connectivity-host)
if ((prebuilt)); then hosts=(); fi
if ((!prebuilt)) && [[ "$mode" == all || "$mode" == relay ]]; then hosts+=(-p uc-connectivity-relay); fi
if ((${#hosts[@]})); then cargo build "${hosts[@]}" --locked; fi
target=$(cargo metadata --locked --no-deps --format-version 1 | node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')
evidence="${UC_CONNECTION_RECOVERY_EVIDENCE:-$target/connection-recovery-evidence}"
host_binary=${host_binary:-$target/debug/uc-connectivity-host}
[[ -x "$host_binary" ]] || { echo 'The test host has not been built; run without --prebuilt.' >&2; exit 2; }
mkdir -p "$evidence"
# 旧版互通使用升级兼容矩阵的 a07 锚点（Desktop v1.0.0-alpha.10，Engine v1.1.0-rc.15），由同一构建脚本按 rev 缓存。
legacy_anchor=a07
legacy_revision=f6f305d9689e4e79e7ab6d0e4921061f9416e4a6
cleanup() {
  local status=$?
  if ((EUID != 0)) && [[ "${UC_NETWORK_UNPRIVILEGED:-0}" != 1 ]]; then
    sudo chown -R -- "$(id -u):$(id -g)" "$evidence" || status=1
  fi
  exit "$status"
}
trap cleanup EXIT
if [[ "$mode" == all || "$mode" == legacy ]]; then
  bash scripts/testing/build-upgrade-anchors.sh "$legacy_anchor"
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
runner=(node "$repo/scripts/testing/connection-recovery-network.mjs" --host "$host_binary" --repeat "$repeat" --evidence "$evidence" --blob-baseline "$blob_baseline" --old-pair-only "$old_pair_only")
if ((old_pair_only)); then
  [[ "$blob_baseline" == 1 && -n "$prepaired_host" && "$host_binary" == "$prepaired_host" ]] || { echo '--old-pair-only requires --blob-baseline and the same --host/--prepaired-host.' >&2; exit 2; }
fi
if [[ -n "$case_prefix" ]]; then runner+=(--case "$case_prefix"); fi
if [[ -n "$peer_host" ]]; then
  [[ "$mode" != all && "$mode" != legacy && -x "$peer_host" && "$peer_side" =~ ^[01]$ ]] || { echo "--peer-host needs an executable, --peer-side 0|1 and a single non-legacy --mode." >&2; exit 2; }
  runner+=(--peer-host "$peer_host" --peer-side "$peer_side")
fi
if ((blob_baseline)); then
  [[ -n "$prepaired_host" && "$case_prefix" == P00-prepaired-upgrade ]] || { echo '--blob-baseline requires the prepaired upgrade scenario.' >&2; exit 2; }
fi
if [[ -n "$prepaired_host" ]]; then
  [[ "$suite" == network && ( "$mode" == direct || "$mode" == relay ) && -x "$prepaired_host" && "$upgrade_side" =~ ^[01]$ && "$case_prefix" == P00-prepaired-upgrade && -z "$peer_host" ]] || { echo '--prepaired-host requires network direct|relay, --upgrade-side 0|1 and --case P00-prepaired-upgrade.' >&2; exit 2; }
  runner+=(--prepaired-host "$prepaired_host" --upgrade-side "$upgrade_side")
  if [[ -n "$presence_role" ]]; then runner+=(--presence-role "$presence_role"); fi
  if [[ -n "$presence_protocol" ]]; then runner+=(--presence-protocol "$presence_protocol"); fi
fi
if ((EUID != 0)); then
  if [[ "${UC_NETWORK_UNPRIVILEGED:-0}" == 1 ]]; then
    runner=(unshare --user --map-root-user --mount --net --fork bash -c 'mount -t tmpfs tmpfs /run; exec "$@"' bash "${runner[@]}")
  else
    runner=(sudo -- "${runner[@]}")
  fi
fi
if [[ "$mode" == all || "$mode" == direct ]]; then "${runner[@]}" --mode direct; fi
if [[ "$mode" == all || "$mode" == known-peer ]]; then "${runner[@]}" --mode known-peer; fi
if [[ "$mode" == all || "$mode" == relay ]]; then "${runner[@]}" --mode relay --relay "${relay_binary:-$target/debug/uc-connectivity-relay}" ${relay_b:+--relay-b "$relay_b"}; fi
if [[ "$mode" == all || "$mode" == legacy ]]; then
  "${runner[@]}" --mode legacy --legacy-host "$target/upgrade-anchors/$legacy_revision/bin/uc-connectivity-host" --legacy-side 0
fi
