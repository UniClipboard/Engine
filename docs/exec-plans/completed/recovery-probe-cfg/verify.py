"""校验恢复观测切片的 E2E 工件及其源码树。"""
import collections
import gzip
import hashlib
import json
import os
import pathlib
import stat

here = pathlib.Path(__file__).resolve().parent
repo = here.parents[3]
sha = lambda data: hashlib.sha256(data).hexdigest()
ledger = json.loads((here / 'artifact-manifest.json').read_bytes())
actual = {str(p.relative_to(here)) for p in here.rglob('*') if p.is_file() and p.name != 'artifact-manifest.json'}
assert actual == set(ledger['files']), 'artifact file set changed'
for name, record in ledger['files'].items():
    data = (here / name).read_bytes()
    assert len(data) == record['bytes'] and sha(data) == record['sha256'], name
source_bytes = gzip.decompress((here / 'source-manifest.json.gz').read_bytes())
source = json.loads(source_bytes)
source_hash = sha(source_bytes)
for name, record in source['files'].items():
    path = repo / name
    if record['kind'] == 'symlink':
        assert path.is_symlink() and os.readlink(path) == record['target'], name
    else:
        assert not path.is_symlink() and sha(path.read_bytes()) == record['sha256'], name
        assert oct(stat.S_IMODE(path.stat().st_mode)) == record['mode'], name
for name in ['source-before', 'source-after-normal', 'source-after-fault', 'source-after-e2e']:
    record = json.loads((here / f'{name}.json').read_bytes())
    assert record == {'manifest_sha256': source_hash, 'verified_items': len(source['files']), 'result': 'PASS'}, name
provenance = json.loads((here / 'compiled-feature-provenance.json').read_bytes())
assert provenance['source_manifest_sha256'] == source_hash
records = {record['binary']: record for record in provenance['records']}
assert set(records) == {'normal', 'fault'}
common = {'production-e2e', 'join-status', 'device-group-choices', 'network-recovery', 'process-observability', 'startup-progress', 'transfer-progress'}
for variant in records:
    wanted = common if variant == 'normal' else common | {'clipboard-stream-fault', 'network-test-probe'}
    assert set(records[variant]['compiled_features']) == wanted
    fingerprint = (here / f'{variant}-fingerprint.json').read_bytes()
    assert sha(fingerprint) == records[variant]['fingerprint_sha256']
    assert json.loads(json.loads(fingerprint)['features']) == records[variant]['compiled_features']
    for package in ['uc-engine', 'uc-infra-p2p']:
        graph = (here / f'{variant}-{package}-inverse-features.txt').read_text()
        assert 'uc-engine feature "dev-tools"' not in graph
        assert 'uc-infra-p2p feature "test-util"' not in graph
        if variant == 'normal':
            assert 'clipboard-stream-fault' not in graph and 'network-test-probe' not in graph
control = json.loads((here / 'feature-off-process.json').read_bytes())
assert control['result'] == 'PASS' and control['binary_sha256'] == records['normal']['binary_sha256']
assert control['shutdown_complete'] and control['profile_removed']
assert not {'clipboard-stream-fault', 'network-test-probe'} & set(control['capabilities'])
assert {record['command'] for record in control['commands']} == {'stream_fault', 'address_probe', 'subnet_probe'}
assert all(record['rejected'] and record['error'] == 'operation_failed' for record in control['commands'])
total = 0
journals = 0
for directory, variant, cases in [('normal-l04', 'normal', ['L04']), ('recovery-causal', 'fault', ['R01', 'R02', 'R02A', 'R03', 'R04', 'R04F', 'R04E'])]:
    result = json.loads(gzip.decompress((here / directory / 'result.json.gz').read_bytes()))
    assert result['complete'] and result['binary_sha256'] == records[variant]['binary_sha256']
    assert result['driver_sha256'] == sha((repo / 'scripts/testing/pr343-e2e.mjs').read_bytes())
    expected = {(case, iteration) for case in cases for iteration in [1, 2, 3]}
    assert collections.Counter((record['id'], record['iteration']) for record in result['results']) == collections.Counter(expected)
    for record in result['results']:
        assert record['outcome'] == 'passed' and not record.get('failed_attempt'), record['id']
        assert record['cleanup']['cleaned'] and record['cleanup']['plaintext_clean']
        assert record['cleanup']['canary_matches'] == 0
        assert all(readback['matched'] for readback in record['readbacks'])
        for journal in record['facts']['network_diagnostics']:
            data = gzip.decompress((here / directory / (journal['file'] + '.gz')).read_bytes())
            assert len(data) == journal['bytes'] and sha(data) == journal['sha256']
            lines = data.splitlines()
            assert len(lines) == journal['rows']
            for line in lines:
                json.loads(line)
            journals += 1
        total += 1
assert total == 24 and journals == 48
assert (here / 'final-netns.txt').read_bytes() == b''
assert (here / 'e2e-exit-code.txt').read_text().strip() == '0'
assert (here / 'build-exit-code.txt').read_text().strip() == '0'
gates = json.loads((here / 'gate-results.json').read_bytes())
assert gates['result'] == 'PASS' and gates['local_delivery_exit_code'] == 0
cleanup = json.loads((here / 'lab-cleanup.json').read_bytes())
assert all(cleanup[key] for key in ['owned_container_removed', 'owned_directory_removed', 'other_running_containers_preserved', 'shared_caches_preserved', 'canonical_target_preserved'])
print(f'PASS: {total} E2E runs, {journals} journals, {len(source["files"])} source items, both compiled feature selections, feature-off controls, cleanup and privacy.')
