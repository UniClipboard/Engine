#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
export UC_MANAGED_RELAY_ARTIFACT_DIR="$root/target/managed-relay-acceptance"
mkdir -p "$UC_MANAGED_RELAY_ARTIFACT_DIR"
# 两个 Cargo workspace 串行复用同一个仓库 target。
export CARGO_TARGET_DIR="$root/target"
# iroh-relay 同时输出无哈希名称的 cdylib/rlib。切换 workspace 后仅重建该包，
# 防止旧功能集的别名产物覆盖当前功能集。保留全部 MBX 动作缓存。
just cargo clean --manifest-path third_party/iroh/iroh/Cargo.toml -p iroh-relay \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/transport-outputs.log" 2>&1
just cargo test --manifest-path third_party/iroh/iroh/Cargo.toml \
  relay_credentials_follow_live_map_updates --lib --features test-utils --locked \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/transport.log" 2>&1
just cargo test --manifest-path third_party/iroh/iroh/Cargo.toml \
  test_endpoint_relay_auth_token --lib --features test-utils --locked \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/custom-transport.log" 2>&1
just cargo clean -p iroh-relay \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/engine-outputs.log" 2>&1
just cargo test -p uc-infra-p2p managed_relay --lib --locked \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/control-plane.log" 2>&1
just cargo test -p uc-engine --test search_and_settings_capabilities managed_relay_overview --locked \
  > "$UC_MANAGED_RELAY_ARTIFACT_DIR/engine-routing.log" 2>&1
live_exit=0
if [[ "${UC_MANAGED_RELAY_LIVE:-0}" == 1 ]]; then
  just cargo test -p uc-infra-p2p live_managed_relay_accepts_current_endpoint_credential --lib --locked -- --ignored \
    > "$UC_MANAGED_RELAY_ARTIFACT_DIR/live.log" 2>&1 || live_exit=$?
fi
export UC_MANAGED_RELAY_LIVE_EXIT="$live_exit"
python3 - <<'PY'
import hashlib
import json
import os
import pathlib
import subprocess

root = pathlib.Path(os.environ['UC_MANAGED_RELAY_ARTIFACT_DIR'])
files = sorted(file for file in root.glob('*.json') if file.name != 'manifest.json' and (file.name != 'live.json' or os.environ.get('UC_MANAGED_RELAY_LIVE') == '1'))
for file in files:
    body = file.read_bytes()
    assert b'PRIVATE_' not in body, 'Acceptance artifact contains a secret sentinel'
    json.loads(body)
candidate = hashlib.sha256()
for name in sorted(subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard', '-z']).decode().split('\0')):
    file = pathlib.Path(name)
    if name and file.is_file():
        candidate.update(name.encode() + b'\0' + file.read_bytes())
manifest = {
    'candidate_files_sha256': candidate.hexdigest(),
    'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'network_stack_commit': subprocess.check_output(['git', '-C', 'third_party/iroh', 'rev-parse', 'HEAD'], text=True).strip(),
    'working_tree_changes': bool(subprocess.check_output(['git', 'status', '--porcelain'])),
    'local_passed': True,
    'live_exit_code': int(os.environ.get('UC_MANAGED_RELAY_LIVE_EXIT', '0')),
    'live_requested': os.environ.get('UC_MANAGED_RELAY_LIVE') == '1',
    'logs': {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in sorted(root.glob('*.log'))},
    'files': {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in files if file.name != 'manifest.json'},
}
(root / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(f'Managed relay acceptance artifacts: {root}')
PY

exit "$live_exit"
