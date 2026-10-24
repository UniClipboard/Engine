"""快照传输、提取与信任边界的故障回归。"""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('snapshot', Path(__file__).with_name('mbx-snapshot.py'))
snapshot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshot)


class Store:
    def __init__(self):
        self.objects = {}
        self.calls = []

    def get_object(self, Bucket, Key):
        self.calls.append(('get', Key))
        body = self.objects[Key]
        return dict(Body=io.BytesIO(body), ContentLength=len(body))

    def upload_file(self, path, bucket, key):
        self.calls.append(('archive', key))
        self.objects[key] = Path(path).read_bytes()

    def put_object(self, Bucket, Key, Body, **kwargs):
        self.calls.append(('index', Key))
        self.objects[Key] = Body


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.key = snapshot.PREFIX + '/test/' + '1' * 64
        self.report = dict(index_reads=0, archive_reads=0, downloaded_bytes=0)

    def archive(self, members):
        path = self.root / 'test.tar.gz'
        with tarfile.open(path, 'w:gz') as output:
            for name, content, kind in members:
                entry = tarfile.TarInfo(name)
                entry.type = kind
                entry.size = len(content)
                output.addfile(entry, io.BytesIO(content))
        return path

    def store(self):
        archive = self.archive([(snapshot.MANIFEST, b'{"version":1}', tarfile.REGTYPE),
                                ('cas/ab/blob', b'bytes', tarfile.REGTYPE)])
        body = archive.read_bytes()
        sha = hashlib.sha256(body).hexdigest()
        index = dict(version=1, sha256=sha, size=len(body), key=f'{self.key}/{sha}.tar.gz')
        store = Store()
        store.objects[self.key + '/latest.json'] = json.dumps(index).encode()
        store.objects[index['key']] = body
        return store, index

    def test_hit_is_one_index_and_one_archive_then_native_import(self):
        store, index = self.store()
        with patch.object(snapshot, 'mbx') as mbx:
            snapshot.restore(store, self.key, self.root, self.report)
        self.assertEqual(self.report['status'], 'hit')
        self.assertEqual(len(store.calls), 2)
        self.assertEqual(mbx.call_args.args[:2], ('cache', 'import'))

    def test_missing_index_is_cold_and_does_not_read_archive(self):
        with patch.object(snapshot, 'get_index', return_value=None), patch.object(snapshot, 'mbx') as mbx:
            snapshot.restore(Store(), self.key, self.root, self.report)
        self.assertEqual(self.report['status'], 'miss')
        self.assertEqual(self.report['archive_reads'], 0)
        mbx.assert_not_called()

    def test_corrupt_archive_never_imports(self):
        store, index = self.store()
        store.objects[index['key']] = b'x' * index['size']
        with patch.object(snapshot, 'mbx') as mbx, self.assertRaises(ValueError):
            snapshot.restore(store, self.key, self.root, self.report)
        mbx.assert_not_called()

    def test_native_closure_failure_is_not_a_hit(self):
        store, _ = self.store()
        with patch.object(snapshot, 'mbx', side_effect=RuntimeError('incomplete closure')), self.assertRaises(RuntimeError):
            snapshot.restore(store, self.key, self.root, self.report)
        self.assertNotEqual(self.report.get('status'), 'hit')

    def test_reject_unsafe_archive_and_workspace_attachments(self):
        for name, kind in [('cas/../../escape', tarfile.REGTYPE), ('/cas/escape', tarfile.REGTYPE),
                           ('cas/link', tarfile.SYMTYPE), ('cas/hard', tarfile.LNKTYPE),
                           ('cas/device', tarfile.CHRTYPE), ('target/file', tarfile.REGTYPE),
                           ('cas/a\\b', tarfile.REGTYPE), ('cas/CON', tarfile.REGTYPE),
                           ('cas/a:stream', tarfile.REGTYPE), ('cas/a.', tarfile.REGTYPE)]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as target:
                archive = self.archive([(name, b'', kind)])
                with self.assertRaises(ValueError):
                    snapshot.unpack(archive, Path(target))
        for field in ['attachments', 'objects']:
            archive = self.archive([(snapshot.MANIFEST, json.dumps({field: ['secret']}).encode(), tarfile.REGTYPE)])
            with tempfile.TemporaryDirectory() as target, self.assertRaises(ValueError):
                snapshot.unpack(archive, Path(target))

    def test_duplicate_and_expansion_limits(self):
        for members in [[('cas/a', b'x', tarfile.REGTYPE)] * 2,
                        [('cas/a', b'oversize', tarfile.REGTYPE)]]:
            archive = self.archive(members)
            with tempfile.TemporaryDirectory() as target, patch.object(snapshot, 'MAX_EXPANDED_BYTES', 4), self.assertRaises(ValueError):
                snapshot.unpack(archive, Path(target))

    def test_index_scope_size_and_digest(self):
        _, index = self.store()
        for change in [dict(key='other/archive'), dict(size=0), dict(size=True), dict(sha256='X'*64), dict(version=2)]:
            with self.subTest(change=change), self.assertRaises(ValueError):
                snapshot.validate_index({**index, **change}, self.key)

    def test_save_requires_trusted_designated_main_and_index_is_last(self):
        for env in [{}, dict(GITHUB_REF='refs/heads/main', GITHUB_REF_PROTECTED='false'),
                    dict(GITHUB_REF='refs/heads/main', GITHUB_REF_PROTECTED='true', GITHUB_EVENT_NAME='pull_request'),
                    dict(GITHUB_REF='refs/heads/main', GITHUB_REF_PROTECTED='true', GITHUB_EVENT_NAME='push')]:
            with patch.dict(os.environ, env, clear=True), self.assertRaises(ValueError), patch.object(snapshot, 'mbx') as mbx:
                snapshot.save(Store(), self.key, self.root, {}, 'test')
            mbx.assert_not_called()
        def compact(exported, destination):
            destination.mkdir()
            (destination / snapshot.MANIFEST).write_text('{"version":1}')
        env = dict(GITHUB_REF='refs/heads/main', GITHUB_REF_PROTECTED='true', GITHUB_EVENT_NAME='push',
                   UC_MBX_SNAPSHOT_WRITER='true', GITHUB_SHA='a'*40)
        store = Store()
        with patch.dict(os.environ, env, clear=True), patch.object(snapshot, 'mbx'), patch.object(snapshot, 'compact_bundle', side_effect=compact):
            snapshot.save(store, self.key, self.root, {}, 'test')
        self.assertEqual([c[0] for c in store.calls], ['archive', 'index'])

    def test_no_credentials_and_transport_failure_are_local_only(self):
        env = dict(RUNNER_TEMP=str(self.root), UC_ENGINE_MBX_STATS_DIR=str(self.root),
                   UC_MBX_SNAPSHOT_KEY=self.key, GITHUB_ENV=str(self.root/'env'))
        for creds, expected in [({}, 'no-credentials'),
                                ({'BUILD_CACHE_R2_ENDPOINT':'https://example.invalid'}, 'error')]:
            with patch.dict(os.environ, {**env, **creds}, clear=True), patch('sys.argv', ['snapshot', 'restore', '--scene', 'test']), patch.object(snapshot, 'client') as client:
                self.assertEqual(snapshot.main(), 0)
            client.assert_not_called()
            report = json.loads((self.root/'snapshot-restore.json').read_text())
            self.assertEqual(report['status'], expected)
            self.assertFalse(report['object_fallback'])
        self.assertNotIn('BUILD_CACHE_R2', (self.root/'env').read_text())

    def test_compatibility_key_separates_scene_architecture_toolchain_and_inputs(self):
        for name in ['Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'rust-toolchain.toml',
                     '.github/actions/rust-ci-setup/action.yml', 'scripts/build-cache/mbx.sh']:
            path = self.root/name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('baseline')
        with patch.object(snapshot, 'ROOT', self.root), patch.dict(os.environ, {}, clear=True), \
             patch.object(snapshot.subprocess, 'check_output', return_value=b'rustc version') as rustc:
            baseline = snapshot.scene_key('test')
            self.assertNotEqual(snapshot.scene_key('different-target'), baseline)
            with patch.dict(os.environ, RUNNER_ARCH='ARM64'):
                self.assertNotEqual(snapshot.scene_key('test'), baseline)
            rustc.return_value = b'new toolchain'
            self.assertNotEqual(snapshot.scene_key('test'), baseline)
            rustc.return_value = b'rustc version'
            (self.root/'Cargo.lock').write_text('new lock')
            self.assertNotEqual(snapshot.scene_key('test'), baseline)

    def test_transport_failure_is_reported_without_native_fallback(self):
        env = dict(RUNNER_TEMP=str(self.root), UC_ENGINE_MBX_STATS_DIR=str(self.root),
                   UC_MBX_SNAPSHOT_KEY=self.key, BUILD_CACHE_R2_ENDPOINT='https://example.invalid',
                   BUILD_CACHE_R2_ACCESS_KEY_ID='sentinel', BUILD_CACHE_R2_SECRET_ACCESS_KEY='sentinel')
        with patch.dict(os.environ, env, clear=True), patch('sys.argv', ['snapshot', 'restore', '--scene', 'test']), \
             patch.object(snapshot, 'client', side_effect=TimeoutError('secret-sentinel')), patch.object(snapshot, 'mbx') as mbx:
            self.assertEqual(snapshot.main(), 0)
        mbx.assert_not_called()
        raw = (self.root/'snapshot-restore.json').read_text()
        self.assertNotIn('sentinel', raw)
        self.assertEqual(json.loads(raw)['status'], 'error')

    def test_mbx_subprocess_cannot_inherit_remote_identity(self):
        with patch.dict(os.environ, BUILD_CACHE_R2_ENDPOINT='secret', AWS_ACCESS_KEY_ID='secret',
                        MBX_REMOTE_URL='s3://bad', UC_ENGINE_MBX_ENV_FILE='secret-file'), patch.object(snapshot.subprocess, 'run') as run:
            snapshot.mbx('cache', 'import', 'fixture')
        env = run.call_args.kwargs['env']
        self.assertFalse(any(k.startswith(('AWS_', 'BUILD_CACHE_R2_')) for k in env))
        self.assertEqual(env['MBX_REMOTE_URL'], '')
        self.assertEqual(env['UC_ENGINE_MBX_LOCAL_ONLY'], '1')


if __name__ == '__main__':
    unittest.main()
