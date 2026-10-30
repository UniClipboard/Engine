#!/usr/bin/env bash
# 真实原生生命周期的外部资源验收，复用原测试分组和共享构建。
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/build-cache/env.sh
python3 scripts/testing/verify-blob-lifecycle-resources.test.py
export UC_RECEIVE_RESOURCE_EVIDENCE_DIR="${UC_RECEIVE_RESOURCE_EVIDENCE_DIR:-${UC_TEST_ARTIFACTS_DIR:-target/test-artifacts}/blob-resources/lifecycle-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
# nextest会以crate目录运行测试，所有原始计数必须汇入同一个绝对目录。
export UC_RECEIVE_RESOURCE_EVIDENCE_DIR="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR")"
export UC_MANAGED_BLOB_RESOURCE_EVIDENCE="$UC_RECEIVE_RESOURCE_EVIDENCE_DIR/managed-engine-large.json"
python3 - "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR" <<'PYSETUP'
from pathlib import Path
import shutil,sys
root=Path(sys.argv[1]);root.mkdir(parents=True,exist_ok=False)
previous=Path('target/nextest/ci/junit.xml')
if previous.exists():
    shutil.copy2(previous,root/'previous-junit.xml')
    previous.unlink()
PYSETUP
filter='(package(uc-engine) & kind(lib) & test(=testing::host_adapter_contract::blob_resources::managed_engine_two_one_gib_files_stream_with_bounded_memory_and_settle_fd_and_tasks)) | package(uc-engine) & kind(lib) & test(/real_engine_(lock_waits|revocation_during|space_reset_seals|confirmed_local_removal|abandoned_deletion|shutdown_waits_for_failed_file_settlement_repair)/) | (package(uc-engine) & kind(lib) & test(=runtime::blob_source_authorization::quota::ticket_provider_budget_is_shared_with_a_different_message_author)) | (package(uc-engine) & kind(lib) & test(=testing::host_adapter_contract::receive_retry::backpressure::real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup))'
if [[ "${UC_TEST_BUILD_SCOPE:-group}" == workspace ]]; then
  bash scripts/testing/run-test-group.sh workspace -E "$filter"
else
  just cargo nextest run --profile ci --locked -p uc-engine --lib \
    --features dev-tools,uc-application/test-support,uc-infra-p2p/test-util,uc-infra-storage/test-util -E "$filter"
fi
python3 scripts/testing/verify-blob-lifecycle-resources.py "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR"
