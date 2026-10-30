#!/usr/bin/env bash
# 默认验收CI生命周期；双1GiB资源基线仅由开发者本机显式启用。
set -euo pipefail
cd "$(dirname "$0")/../.."
include_local_large=false
if [[ $# -gt 0 ]]; then
  if [[ $# != 1 || "$1" != --include-local-large ]]; then
    echo 'usage: run-blob-lifecycle-resources.sh [--include-local-large]' >&2
    exit 2
  fi
  if [[ "${CI:-}" == true || "${CI:-}" == 1 || "${GITHUB_ACTIONS:-}" == true ]]; then
    echo '双1GiB资源基线仅供开发者本机运行，CI不得启用。' >&2
    exit 2
  fi
  include_local_large=true
fi
source scripts/build-cache/env.sh
python3 scripts/testing/verify-blob-lifecycle-resources.test.py
export UC_RECEIVE_RESOURCE_EVIDENCE_DIR="${UC_RECEIVE_RESOURCE_EVIDENCE_DIR:-${UC_TEST_ARTIFACTS_DIR:-target/test-artifacts}/blob-resources/lifecycle-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
# nextest会以crate目录运行测试，所有原始计数必须汇入同一个绝对目录。
export UC_RECEIVE_RESOURCE_EVIDENCE_DIR="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR")"
python3 - "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR" <<'PYSETUP'
from pathlib import Path
import shutil,sys
root=Path(sys.argv[1]);root.mkdir(parents=True,exist_ok=False)
previous=Path('target/nextest/ci/junit.xml')
if previous.exists():
    shutil.copy2(previous,root/'previous-junit.xml')
    previous.unlink()
PYSETUP
filter='package(uc-engine) & kind(lib) & test(/real_engine_(lock_waits|revocation_during|space_reset_seals|confirmed_local_removal|abandoned_deletion|shutdown_waits_for_failed_file_settlement_repair)/) | (package(uc-engine) & kind(lib) & test(=runtime::blob_source_authorization::quota::ticket_provider_budget_is_shared_with_a_different_message_author)) | (package(uc-engine) & kind(lib) & test(=testing::host_adapter_contract::receive_retry::backpressure::real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup))'
test_args=()
verification_args=()
if [[ "$include_local_large" == true ]]; then
  filter="($filter) | (package(uc-engine) & kind(lib) & test(=testing::host_adapter_contract::blob_resources::managed_engine_two_one_gib_files_stream_with_bounded_memory_and_settle_fd_and_tasks))"
  test_args=(--run-ignored all)
  verification_args=(--include-local-large)
  export UC_MANAGED_BLOB_RESOURCE_EVIDENCE="$UC_RECEIVE_RESOURCE_EVIDENCE_DIR/managed-engine-large.json"
fi
if [[ "${UC_TEST_BUILD_SCOPE:-group}" == workspace ]]; then
  bash scripts/testing/run-test-group.sh workspace -E "$filter" "${test_args[@]}"
else
  just cargo nextest run --profile ci --locked -p uc-engine --lib \
    --features dev-tools,uc-application/test-support,uc-infra-p2p/test-util,uc-infra-storage/test-util -E "$filter" "${test_args[@]}"
fi
python3 scripts/testing/verify-blob-lifecycle-resources.py "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR" "${verification_args[@]}"
