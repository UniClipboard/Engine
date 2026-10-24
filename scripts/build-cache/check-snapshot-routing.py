"""检查所有 CI 入口：原生 S3 禁用，凭据只能传给快照 action。"""
from pathlib import Path
import re

root = Path(__file__).resolve().parents[2]
workflows = list((root/'.github/workflows').glob('*.yml'))
for path in workflows:
    text = path.read_text()
    assert '--r2' not in text and 'MBX_REMOTE_MODE:' not in text, path
    for step in re.split(r'(?=^      - )', text, flags=re.M):
        if 'secrets.BUILD_CACHE_R2_' in step:
            assert ('uses: ./.github/actions/rust-ci-setup' in step
                    or 'uses: ./.github/actions/mbx-snapshot' in step), path
            assert not re.search(r'^          BUILD_CACHE_R2_', step, re.M), path
        if 'operation: save' in step:
            assert 'engine-build-cache-writer' in text, path
            assert "github.ref_protected" in step or 'Require protected main' in text, path
    assert 'cache-directories:' not in text, path
setup = (root/'.github/actions/rust-ci-setup/action.yml').read_text()
assert 'cache-directories:' not in setup
assert "cache-targets: 'false'" in setup
start = (root/'scripts/build-cache/start-ci-mbx.sh').read_text()
assert 'source ' not in start and 'printf \'export BUILD_CACHE_R2_' not in start
assert 'echo "$variable="' in start
wrapper = (root/'scripts/build-cache/mbx.sh').read_text()
assert wrapper.index('unset UC_ENGINE_MBX_ENV_FILE') < wrapper.index('source "$UC_ENGINE_MBX_ENV_FILE"')
assert 'GITHUB_ACTIONS:-' in wrapper and 'export MBX_REMOTE_URL=""' in wrapper
for name in ('release-engine.yml', 'release-lan-compat.yml'):
    text = (root/'.github/workflows'/name).read_text()
    assert 'secrets.BUILD_CACHE_R2_' not in text and 'operation: save' not in text
print(f'Snapshot-only routing passed for {len(workflows)} workflows')
