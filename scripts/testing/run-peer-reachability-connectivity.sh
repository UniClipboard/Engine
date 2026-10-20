#!/usr/bin/env bash
# 第六切片默认必需门禁；同一普通用户、共享 target 串行构建，真实资料逐格独立生成与回收。
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
matrix_root="$repo/target/peer-reachability-connectivity-evidence"
root="${UC_PRESENCE_EVIDENCE_ROOT:-$matrix_root}"
mkdir -p "$root"
# 所有依赖缺失均失败，不跳过格子。
for tool in node ip nft just rg; do command -v "$tool" >/dev/null; done
just cargo build --locked -p uc-connectivity-host -p uc-connectivity-relay
bash scripts/testing/build-peer-reachability-main.sh
bash scripts/testing/build-upgrade-anchors.sh a07
main_host="$matrix_root/frozen-main/uc-connectivity-host"
rc15_host="$repo/target/upgrade-anchors/f6f305d9689e4e79e7ab6d0e4921061f9416e4a6/bin/uc-connectivity-host"
# 每格独立工件，首次失败不被其他格覆盖。任一失败立即阻断。
run() {
  local label=$1; shift
  UC_CONNECTION_RECOVERY_EVIDENCE="$root/$label" \
    bash scripts/testing/run-connection-recovery-e2e.sh --suite network --prebuilt "$@" \
    >"$root/$label.log" 2>&1
}
run new-new-direct --mode direct --case E02 --repeat 1
run new-new-relay --mode relay --case E10-relay-only --repeat 20
for anchor in frozen-main rc15; do
  if [[ "$anchor" == frozen-main ]]; then host="$main_host"; protocol=peer_reachability_v2; rounds=3
  else host="$rc15_host"; protocol=peer_reachability_v1; rounds=1; fi
  for topology in direct relay; do
    for role in receive initiate; do
      # rc.15 Sponsor 首次保存 Joiner 时可能只有启动直连地址；保留其原资料，
      # 接收角色升级 Sponsor，主动角色升级 Joiner，不靠补地址或改旧库制造通过。
      if [[ "$role" == receive ]]; then upgraded_side=0; else upgraded_side=1; fi
      run "$anchor-$topology-$role" --mode "$topology" --case P00-prepaired-upgrade \
        --prepaired-host "$host" --upgrade-side "$upgraded_side" --presence-role "$role" \
        --presence-protocol "$protocol" --repeat "$rounds"
    done
  done
done
run legacy-rejection --mode legacy --case E09 --repeat 1
