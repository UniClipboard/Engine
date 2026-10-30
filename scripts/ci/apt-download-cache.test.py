"""缓存命中不能改变 apt 的解析、安装及校验边界。"""

import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('apt_cache', Path(__file__).with_name('apt-download-cache.py'))
cache = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cache)


class AptDownloadCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.payload = b'authenticated package bytes'
        self.name = 'example_1.0_amd64.deb'
        self.hash = hashlib.sha256(self.payload).hexdigest()
        self.uri = f"'https://mirror.example/{self.name}' {self.name} {len(self.payload)} SHA256:{self.hash}"
        self.downloads = cache.parse_downloads('Reading package lists...\n' + self.uri)

    def test_cold_cache_leaves_downloads_to_apt(self):
        cache.verify_downloads(self.root, self.downloads)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_hot_cache_preserves_verified_bytes(self):
        archive = self.root / self.name
        archive.write_bytes(self.payload)
        cache.verify_downloads(self.root, self.downloads)
        self.assertEqual(archive.read_bytes(), self.payload)

    def test_corrupt_same_size_stale_and_symlink_are_removed(self):
        archive = self.root / self.name
        archive.write_bytes(b'x' * len(self.payload))
        (self.root / 'old_1_all.deb').write_bytes(self.payload)
        cache.verify_downloads(self.root, self.downloads)
        self.assertEqual(list(self.root.iterdir()), [])
        target = self.root / 'original'
        target.write_bytes(self.payload)
        archive.symlink_to(target)
        cache.verify_downloads(self.root, self.downloads)
        self.assertFalse(archive.is_symlink())
        self.assertEqual(target.read_bytes(), self.payload)

    def test_short_download_is_removed(self):
        archive = self.root / self.name
        archive.write_bytes(self.payload[:-1])
        cache.verify_downloads(self.root, self.downloads)
        self.assertFalse(archive.exists())

    def test_unsafe_name_and_missing_strong_hash_fail_closed(self):
        for line in [self.uri.replace(self.name, '../escape.deb'),
                     self.uri.replace('SHA256:', 'MD5Sum:'),
                     self.uri.rsplit(' ', 1)[0]]:
            with self.subTest(line=line), self.assertRaises(ValueError):
                cache.parse_downloads(line)

    def test_key_changes_for_platform_packages_sources_and_resolution(self):
        inputs = [['example'], self.downloads, ['ubuntu-24.04', 'amd64'], 'apt-config', {'ubuntu.sources': 'source-v1'}]
        baseline = cache.cache_key(*inputs)
        for index in range(len(inputs)):
            changed = copy.deepcopy(inputs)
            changed[index] = ['changed']
            self.assertNotEqual(cache.cache_key(*changed), baseline)
        changed = copy.deepcopy(inputs)
        changed[1][self.name][2] = '0' * 64
        self.assertNotEqual(cache.cache_key(*changed), baseline)
        self.assertEqual(cache.cache_key(*copy.deepcopy(inputs)), baseline)

    def test_cold_and_hot_both_execute_normal_install(self):
        archives = self.root / 'engine-apt' / 'archives'
        archives.mkdir(parents=True)
        (archives.parent / 'downloads.json').write_text(json.dumps(self.downloads))
        for hot in [False, True]:
            if hot:
                (archives / self.name).write_bytes(self.payload)
            with patch.dict(os.environ, RUNNER_TEMP=str(self.root), APT_PACKAGES='example'), \
                 patch('sys.argv', ['apt-download-cache.py', 'install']), \
                 patch.object(cache.subprocess, 'run') as run:
                cache.main()
                args = run.call_args.args[0]
                self.assertEqual(args[-3:], ['--yes', 'install', 'example'])
                self.assertIn('Acquire::ForceHash=SHA256', args)
                self.assertNotIn('--allow-unauthenticated', args)
                self.assertTrue(run.call_args.kwargs['check'])

    def test_update_failure_stops_before_cache_lookup(self):
        with patch.dict(os.environ, RUNNER_TEMP=str(self.root), APT_PACKAGES='example'), \
             patch('sys.argv', ['apt-download-cache.py', 'prepare']), \
             patch.object(cache.subprocess, 'run', side_effect=subprocess.CalledProcessError(100, 'apt-get')) as run, \
             patch.object(cache.subprocess, 'check_output') as output:
            with self.assertRaises(subprocess.CalledProcessError):
                cache.main()
            self.assertEqual(run.call_args.args[0], ['sudo', 'apt-get', 'update'])
            output.assert_not_called()
            self.assertFalse((self.root / 'engine-apt' / 'downloads.json').exists())


if __name__ == '__main__':
    unittest.main()
