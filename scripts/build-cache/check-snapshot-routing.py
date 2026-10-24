"""检查所有 CI 入口：原生 S3 禁用，凭据只能传给快照 action。"""
from pathlib import Path
import re

# 静态矩阵按当前仓库使用的 YAML 子集展开；遇到动态/新语法必须显式扩展检查，不能静默略过。
def matrix_rows(job):
    import itertools
    match = re.search(r'^      matrix:\n((?: {8,}[^\n]*\n|\n)*)', job, re.M)
    if not match:
        assert not re.search(r'^      matrix:', job, re.M), 'Dynamic snapshot writer matrix is unsupported'
        return [{}]
    axes, included, excluded = {}, [], []
    section = None
    for line in match[1].splitlines():
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        axis = re.fullmatch(r'        ([\w-]+): \[([^\]]+)\]', line)
        if axis:
            axes[axis[1]] = [v.strip().strip("\"'") for v in axis[2].split(',')]
            continue
        if line in ('        include:', '        exclude:'):
            section = included if 'include' in line else excluded
            continue
        # include/exclude 的首字段缩进为 10，后续字段缩进为 12。
        first = re.fullmatch(r'          - ([\w-]+): (.+)', line)
        rest = re.fullmatch(r'            ([\w-]+): (.+)', line)
        assert section is not None and (first or rest), f'Unsupported writer matrix line: {line}'
        if first:
            section.append({})
        field = first or rest
        assert section, 'Matrix field without row'
        section[-1][field[1]] = field[2].strip("\"'")
    rows = [dict(zip(axes, values)) for values in itertools.product(*axes.values())] if axes else []
    rows = [row for row in rows if not any(all(row.get(k) == v for k, v in item.items()) for item in excluded)]
    for item in included:
        compatible = [row for row in rows if all(row.get(k) == v for k, v in item.items() if k in axes)] if axes else []
        if compatible:
            for row in compatible:
                row.update(item)
        else:
            rows.append(item.copy())
    return rows


def expand_matrix(value, row):
    result = re.sub(r'\$\{\{\s*matrix\.([\w-]+)\s*\}\}', lambda m: row[m[1]], value)
    assert '${{' not in result, f'Unresolved snapshot routing expression: {result}'
    return result


def writer_inventory(documents):
    writers = {}
    for filename, text in documents.items():
        for job in re.split(r'(?=^  [\w-]+:\n)', text, flags=re.M):
            saves = [step for step in re.split(r'(?=^      - )', job, flags=re.M)
                     if re.search(r"^          operation: ['\"]?save['\"]?$", step, re.M)]
            if not saves:
                continue
            name = job.splitlines()[0].strip().rstrip(':')
            runner = re.search(r'^    runs-on: (.+)$', job, re.M)[1]
            for row in matrix_rows(job):
                host = expand_matrix(runner, row)
                # 保守地忽略镜像版本：同 OS/架构/scene 只能有一个负责人，不能借镜像漂移隐藏重复写入。
                if host.startswith('ubuntu-'):
                    platform = ('Linux', 'ARM64' if host.endswith('-arm') else 'X64')
                elif host == 'macos-15' or host == '[self-hosted, macOS, ARM64, deveco]':
                    platform = ('macOS', 'ARM64')
                elif host in ('windows-2025', 'windows-11-arm'):
                    platform = ('Windows', 'ARM64' if host.endswith('-arm') else 'X64')
                else:
                    raise AssertionError(f'Unclassified snapshot writer host: {host}')
                for step in saves:
                    condition = re.search(r'^        if: (.+)$', step, re.M)
                    condition = condition[1] if condition else ''
                    selectors = re.findall(r"matrix\.([\w-]+) == '([^']+)'", condition)
                    remaining = re.sub(r"matrix\.[\w-]+ == '[^']+'", 'true', condition)
                    assert 'matrix.' not in remaining, f'Unsupported writer selector: {condition}'
                    for field, value in selectors:
                        selector = re.escape(f"matrix.{field} == '{value}'")
                        assert re.search(r'(?:^|&& )' + selector + r'(?: &&|$)', condition), 'Matrix writer selectors must be conjunctive'
                    if any(row[k] != value for k, value in selectors):
                        continue
                    scene = expand_matrix(re.search(r'^          scene: (.+)$', step, re.M)[1].strip("\"'"), row)
                    key = (*platform, scene)
                    writers.setdefault(key, []).append(f'{filename}:{name}:{row}')
    return writers


def unique_writers(documents):
    writers = writer_inventory(documents)
    duplicates = {key: owners for key, owners in writers.items() if len(owners) != 1}
    assert not duplicates, f'Duplicate snapshot writers: {duplicates}'
    return writers


def writer_regressions(documents):
    # 重建本次真实错误：anchor 和四个 shard 同时向同 key 发布，必须识别五个 writer。
    nightly = documents['engine-real-environment.yml']
    end = nightly.index('  upgrade-matrix-report:\n')
    save = '''      - name: Regression duplicate writer
        uses: ./.github/actions/mbx-snapshot
        with:
          operation: save
          scene: engine-real-environment-upgrade-matrix
'''
    broken = dict(documents, **{'engine-real-environment.yml': nightly[:end] + save + nightly[end:]})
    inventory = writer_inventory(broken)
    assert len(inventory[('Linux', 'X64', 'engine-real-environment-upgrade-matrix')]) == 5
    for candidate in (broken, dict(documents, **{'pr-check.yml': documents['pr-check.yml'].replace("matrix.mode == 'direct' && ", '')}),
                      dict(documents, **{'engine-real-environment.yml': nightly.replace("matrix.mode == 'direct' && ", '')})):
        try:
            unique_writers(candidate)
        except AssertionError as error:
            assert 'Duplicate snapshot writers' in str(error)
        else:
            raise AssertionError('Duplicate writer regression was not rejected')
    # 检查所有维度的笛卡尔积，以及 Windows/mobile include 的架构路由。
    fixture = '      matrix:\n        os: [a, b]\n        mode: [c, d]\n'
    assert len(matrix_rows(fixture)) == 4
    extended = fixture + '        exclude:\n          - os: a\n            mode: c\n        include:\n          - os: e\n            mode: f\n'
    assert len(matrix_rows(extended)) == 4
    assert {'os': 'e', 'mode': 'f'} in matrix_rows(extended)
    assert {'os': 'a', 'mode': 'c'} not in matrix_rows(extended)
    assert set(row['arch'] for row in matrix_rows(documents['windows-build.yml'])) == {'x64', 'arm64'}
    assert len(matrix_rows(documents['mobile-target-cache.yml'])) == 3
    shard = nightly[nightly.index('  upgrade-matrix:\n'):nightly.index('  upgrade-matrix-report:\n')]
    assert {row['dimension'] for row in matrix_rows(shard)} == {'d1', 'd2', 'd3', 'd4'}
    assert 'run-test-group.sh upgrade-matrix --dimension ${{ matrix.dimension }}' in shard
    assert 'operation: save' not in shard and 'name: engine-build-cache-reader' in shard
    producer = nightly[nightly.index('  upgrade-anchors:\n'):nightly.index('  upgrade-matrix:\n')]
    assert 'build-upgrade-anchors.sh --all' in producer
    assert 'cargo build --locked -p uc-connectivity-host' in producer
    assert "cargo nextest run --profile ci --locked -p uc-upgrade-matrix -E 'package(uc-upgrade-matrix)' --no-run" in producer
    network = documents['pr-check.yml'].split('  connection-recovery:\n')[1]
    assert {row['mode'] for row in matrix_rows(network)} == {'direct', 'known-peer', 'relay', 'legacy'}
    assert 'run-connection-recovery-e2e.sh --suite network --repeat 1 --mode "${{ matrix.mode }}"' in network
    assert 'cargo build -p uc-connectivity-host -p uc-connectivity-relay --locked' in network


def unique_matrix_artifacts(documents):
    # 证据按具体执行实例命名，不能沿用共享缓存场景；每个 workflow run 的 v4 artifact 不可覆盖。
    artifacts = {}
    jobs_checked = 0
    for filename, text in documents.items():
        for job in re.split(r'(?=^  [\w-]+:\n)', text, flags=re.M):
            if not re.search(r'^      matrix:', job, re.M):
                continue
            uploads = [step for step in re.split(r'(?=^      - )', job, flags=re.M)
                       if 'uses: actions/upload-artifact@' in step]
            if not uploads:
                continue
            jobs_checked += 1
            job_name = job.splitlines()[0].strip().rstrip(':')
            for row in matrix_rows(job):
                host = expand_matrix(re.search(r'^    runs-on: (.+)$', job, re.M)[1], row)
                if host.startswith('ubuntu-'):
                    os_name, arch = 'Linux', 'ARM64' if host.endswith('-arm') else 'X64'
                elif host == 'macos-15':
                    os_name, arch = 'macOS', 'ARM64'
                elif host in ('windows-2025', 'windows-11-arm'):
                    os_name, arch = 'Windows', 'ARM64' if host.endswith('-arm') else 'X64'
                else:
                    raise AssertionError(f'Unclassified artifact matrix host: {host}')
                for number, step in enumerate(uploads):
                    match = re.search(r'^          name: (.+)$', step, re.M)
                    name = match[1].strip("\"'") if match else 'artifact'
                    for context, value in [('github.job', job_name), ('github.run_attempt', '<attempt>'),
                                           ('runner.os', os_name), ('runner.arch', arch)]:
                        name = re.sub(r'\$\{\{\s*' + re.escape(context) + r'\s*\}\}', value, name)
                    name = expand_matrix(name, row)
                    key = (filename, name)
                    artifacts.setdefault(key, []).append(f'{job_name}:{row}:upload-{number}')
    duplicates = {key: owners for key, owners in artifacts.items() if len(owners) > 1}
    assert not duplicates, f'Duplicate matrix artifacts: {duplicates}'
    return jobs_checked, len(artifacts)


def artifact_regressions(documents):
    changes = [
        ('engine-real-environment.yml', 'name: engine-real-environment-${{ matrix.mode }}', 'name: engine-real-environment-network'),
        ('engine-real-environment.yml', 'name: mbx-${{ github.job }}-${{ matrix.mode }}-', 'name: mbx-${{ github.job }}-'),
        ('engine-real-environment.yml', 'name: engine-upgrade-matrix-${{ matrix.dimension }}-', 'name: engine-upgrade-matrix-'),
        ('pr-check.yml', 'name: connection-recovery-evidence-${{ matrix.mode }}', 'name: connection-recovery-evidence'),
    ]
    for filename, original, replacement in changes:
        assert original in documents[filename]
        broken = dict(documents, **{filename: documents[filename].replace(original, replacement)})
        try:
            unique_matrix_artifacts(broken)
        except AssertionError as error:
            assert 'Duplicate matrix artifacts' in str(error)
        else:
            raise AssertionError(f'Artifact collision not rejected: {original}')
    # 同一缓存场景保留；手动场景的证据也继续保留输入模式，避免名称失去含义。
    nightly = documents['engine-real-environment.yml']
    assert nightly.count('cache-key: engine-real-environment-network') == 2
    assert 'scene: engine-real-environment-network' in nightly
    assert 'name: engine-real-environment-${{ inputs.mode }}-${{ github.run_attempt }}' in nightly


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

documents = {path.name: path.read_text() for path in workflows}
writers = unique_writers(documents)
writer_regressions(documents)
print(f'Matrix-expanded writer uniqueness passed for {len(writers)} platform/architecture/scenario keys; duplicate regressions rejected')

artifact_jobs, artifact_names = unique_matrix_artifacts(documents)
artifact_regressions(documents)
print(f'Matrix artifact uniqueness passed: {artifact_jobs} jobs, {artifact_names} expanded names; four collision regressions rejected')
