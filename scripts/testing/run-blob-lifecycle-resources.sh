#!/usr/bin/env bash
# 真实原生生命周期的外部资源验收，复用原测试分组和共享构建。
set -euo pipefail
cd "$(dirname "$0")/../.."
source scripts/build-cache/env.sh
export UC_RECEIVE_RESOURCE_EVIDENCE_DIR="${UC_RECEIVE_RESOURCE_EVIDENCE_DIR:-${UC_TEST_ARTIFACTS_DIR:-target/test-artifacts}/blob-resources/lifecycle-$(date -u +%Y%m%dT%H%M%SZ)-$$}"
python3 - "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR" <<'PYSETUP'
from pathlib import Path
import shutil,sys
root=Path(sys.argv[1]);root.mkdir(parents=True,exist_ok=False)
previous=Path('target/nextest/ci/junit.xml')
if previous.exists():
    shutil.copy2(previous,root/'previous-junit.xml')
    previous.unlink()
PYSETUP
filter='package(uc-engine) & kind(lib) & test(/real_engine_(lock_waits|revocation_during|space_reset_seals|confirmed_local_removal|abandoned_deletion|shutdown_waits_for_failed_file_settlement_repair)/)'
if [[ "${UC_TEST_BUILD_SCOPE:-group}" == workspace ]]; then
  bash scripts/testing/run-test-group.sh workspace -E "$filter"
else
  just cargo nextest run --profile ci --locked -p uc-engine --lib \
    --features dev-tools,uc-application/test-support,uc-infra-p2p/test-util,uc-infra-storage/test-util -E "$filter"
fi
python3 - "$UC_RECEIVE_RESOURCE_EVIDENCE_DIR" <<'PY'
from pathlib import Path
import json,sys
root=Path(sys.argv[1])
reports=[json.loads(path.read_text()) for path in root.glob('receive-lifecycle-*.json')]
assert len(reports)==6, 'all six independent lifecycle reports are required'
assert all(report['resource_settlement_verified'] is True for report in reports)
assert all(report['fd_return_to_initial_verified'] and report['thread_return_to_initial_verified']
           and report['runtime_tasks_return_to_initial_verified'] for report in reports)
(root/'summary.json').write_text(json.dumps({'completed_cases':len(reports),
 'fd_threads_and_runtime_tasks_verified':True,
 'tag_and_quota_counts_directly_observed':False,
 'cases':reports},indent=2)+'\n')
print('six native lifecycle resource reports passed')
PY
