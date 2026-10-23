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
filter='package(uc-engine) & kind(lib) & test(/real_engine_(lock_waits|revocation_during|space_reset_seals|confirmed_local_removal|abandoned_deletion|shutdown_waits_for_failed_file_settlement_repair)/) | (package(uc-engine) & kind(lib) & test(=runtime::blob_source_authorization::quota::ticket_provider_budget_is_shared_with_a_different_message_author))'
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
assert all(report['actual_native_blob_handles_observed'] and report['native_blob_handles_returned_to_initial'] for report in reports)
tags=[json.loads(path.read_text()) for path in root.glob('sdk-tags-*.json')]
assert len(tags)>=12, 'SDK store shutdown observations missing'
assert all(report['store_open_during_count'] and report['sdk_temp_tags']==0 for report in tags)
schedulers=[]
for path in root.glob('download-scheduler-*.jsonl'):
 records=[json.loads(line) for line in path.read_text().splitlines()]
 assert records and records[0]['event']=='created' and records[0]['slots']==0
 assert all(r['active']<=4 and r['peer_active_max']<=2 and r['queued']<=64 and r['peer_queued_max']<=16 for r in records)
 if any(r['slots'] for r in records):
  final=records[-1]
  assert final['event']=='settled' and final['closed'] and final['slots']==final['active']==final['queued']==0
  schedulers.append({'evidence':path.name,'peak_active':max(r['active'] for r in records),
   'peak_peer_active':max(r['peer_active_max'] for r in records),'final':final})
assert any(r['peak_active']==4 and r['peak_peer_active']==2 for r in schedulers), 'actual provider C quota occupancy missing'
(root/'summary.json').write_text(json.dumps({'completed_lifecycle_cases':len(reports),
 'fd_threads_and_runtime_tasks_verified':True,'download_quota_counters_verified':True,
 'sdk_tags_and_native_connection_counts_directly_observed':True,
 'sdk_store_tag_observations':tags,
 'download_schedulers':schedulers,'cases':reports},indent=2)+'\n')
print('six native lifecycle resources, SDK tags and native handles, and actual download quota counters passed')
PY
