#!/usr/bin/env bash
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"
source scripts/build-cache/env.sh
export UC_HOST_ANCHOR=pr343-e2e
if [[ -n "${UC_E2E_SOURCE_REV:-}" ]]; then
  export UC_HOST_ENGINE_REV="$UC_E2E_SOURCE_REV"
else
  UC_HOST_ENGINE_REV=$(git rev-parse HEAD)
  export UC_HOST_ENGINE_REV
fi

# 单独构建，避免 workspace 测试的 dev-dependency 合并关闭生产网络策略。
cargo build -p uc-connectivity-host --no-default-features --features production-e2e,clipboard-stream-fault,network-test-probe --locked
features=$(cargo tree -p uc-connectivity-host --no-default-features --features production-e2e,clipboard-stream-fault,network-test-probe -e features -i uc-infra-p2p)
if [[ "$features" == *'uc-infra-p2p feature "test-util"'* ]]; then
  echo 'The E2E host must retain the production network policy.' >&2
  exit 2
fi
cargo build -p uc-infra-p2p --example pr343_invitation_fixture --locked
node scripts/testing/pr343-e2e.mjs --invitation-fixture target/debug/examples/pr343_invitation_fixture "$@"
