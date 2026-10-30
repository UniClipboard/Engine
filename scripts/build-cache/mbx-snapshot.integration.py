"""固定 MBX 的真实 cold/export/import/hit/失效回归，不访问 R2。"""
import importlib.util
import json
import hashlib
import io
import os
from pathlib import Path
import shutil
import socket
import subprocess
import tarfile
import tempfile
import threading
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('snapshot', Path(__file__).with_name('mbx-snapshot.py'))
snapshot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshot)


def main():
    # 所有测试输出位于仓库 target；结束前等待子进程并清理本次独占目录。
    (ROOT / 'target').mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='snapshot-test-', dir=ROOT/'target') as directory:
        root = Path(directory)
        source = root/'source'
        (source/'src').mkdir(parents=True)
        (source/'Cargo.toml').write_text('[package]\nname="snapshot_fixture"\nversion="0.1.0"\nedition="2021"\n[workspace]\n')
        (source/'src/main.rs').write_text('fn main() { assert_eq!(2 + 2, 4); }\n')
        # 回环监听捕捉任何意外的原生 S3 请求；环境与全局配置同时投毒。
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0))
        listener.listen()
        listener.settimeout(0.1)
        requests = []
        stopped = threading.Event()
        def serve():
            while not stopped.is_set():
                try:
                    connection, _ = listener.accept()
                except socket.timeout:
                    continue
                requests.append(1)
                connection.close()
        thread = threading.Thread(target=serve)
        thread.start()
        endpoint = f'http://127.0.0.1:{listener.getsockname()[1]}'
        config = root/'global-config'/'mbx'
        config.mkdir(parents=True)
        (config/'config.toml').write_text(f'[remote]\nurl="s3://poison"\nnamespace="poison"\ns3_endpoint="{endpoint}"\ns3_region="auto"\n')
        inherited = dict(os.environ)
        os.environ.update(UC_ENGINE_MBX_LOCAL_ONLY='1', GITHUB_ENV=str(root/'test-github-env'), MBX_REMOTE_URL='s3://poison',
                          MBX_REMOTE_NAMESPACE='poison', MBX_REMOTE_S3_ENDPOINT=endpoint,
                          BUILD_CACHE_R2_ENDPOINT=endpoint, BUILD_CACHE_R2_ACCESS_KEY_ID='poison',
                          BUILD_CACHE_R2_SECRET_ACCESS_KEY='poison', AWS_ACCESS_KEY_ID='poison',
                          AWS_SECRET_ACCESS_KEY='poison', MBX_CACHE_EXPORT_GROUP='snapshot-integration',
                          XDG_CONFIG_HOME=str(config.parent), MBX_GC_MIN_FREE_SIZE='0',
                          CARGO_INCREMENTAL='0')
        try:
            producer = root/'producer'
            consumer = root/'consumer'
            os.environ['MBX_CACHE_DIR'] = str(producer)
            def build(label, cache):
                env = dict(os.environ, MBX_CACHE_DIR=str(cache), CARGO_TARGET_DIR=str(root/label),
                           MBX_STATS_REPORT=str(root/(label+'.json')))
                subprocess.run(['bash', str(ROOT/'scripts/build-cache/mbx.sh'), '--r2',
                                'build', '--manifest-path', str(source/'Cargo.toml'), '--offline'],
                               env=env, check=True, cwd=ROOT)
                binary = root/label/'debug'/('snapshot_fixture.exe' if os.name == 'nt' else 'snapshot_fixture')
                subprocess.run([str(binary)], check=True)
                stats = json.loads((root/(label+'.json')).read_text())
                assert stats['downloaded_bytes'] == stats['uploaded_bytes'] == 0, stats
                assert stats['remote_failures'] == stats['background_upload_failures'] == 0, stats
                assert all(stats[k] == 0 for k in ('remote_manifest_lookups', 'remote_action_lookups', 'remote_blob_requests', 'remote_blob_pack_requests')), stats
                return stats
            cold = build('cold-target', producer)
            assert cold['hits'] == 0

            exported = root/'exported'
            snapshot.mbx('cache', 'export', '--group', 'snapshot-integration', '--format', 'directory', str(exported))
            bundle = root/'bundle'
            snapshot.compact_bundle(exported, bundle)
            manifest = json.loads((bundle/snapshot.MANIFEST).read_text())
            assert not manifest.get('attachments') and not manifest.get('objects')
            archive = root/'snapshot.tar.gz'
            with tarfile.open(archive, 'w:gz') as output:
                for member in sorted(bundle.iterdir()):
                    output.add(member, arcname=member.name)
            # 真正执行外层 restore 校验和原生 import；只用内存传输替身，不访问远端。
            body = archive.read_bytes()
            sha = hashlib.sha256(body).hexdigest()
            key = snapshot.PREFIX + '/integration/' + '1'*64
            index = dict(version=1, sha256=sha, size=len(body), key=f'{key}/{sha}.tar.gz')
            class MemoryStore:
                def get_object(self, Bucket, Key):
                    payload = json.dumps(index).encode() if Key.endswith('/latest.json') else body
                    return dict(Body=io.BytesIO(payload), ContentLength=len(payload))
            os.environ['MBX_CACHE_DIR'] = str(consumer)
            report = dict(index_reads=0, archive_reads=0, downloaded_bytes=0)
            restore_root = root/'restore'
            restore_root.mkdir()
            snapshot.restore(MemoryStore(), key, restore_root, report)
            assert report['status'] == 'hit' and report['downloaded_bytes'] > 0
            # 真实原生导入拒绝闭包中的损坏对象，不能以解包成功当作验证通过。
            bad = root/'bad-closure'
            shutil.copytree(bundle, bad)
            blob = next(p for p in (bad/'cas').rglob('*') if p.is_file())
            blob.write_bytes(b'corrupt')
            try:
                snapshot.mbx('cache', 'import', str(bad), cache=root/'bad-cache')
            except subprocess.CalledProcessError:
                pass
            else:
                raise AssertionError('Native MBX accepted corrupt closure')
            for status in ('miss', 'corrupt', 'no-credentials'):
                evidence = root/status
                evidence.mkdir()
                os.environ.update(MBX_CACHE_DIR=str(root/(status+'-cas')),
                                  UC_ENGINE_MBX_STATS_DIR=str(evidence), RUNNER_TEMP=str(root),
                                  UC_MBX_SNAPSHOT_KEY=key)
                argv = ['snapshot', 'restore', '--scene', 'integration']
                if status == 'no-credentials':
                    for name in ('ENDPOINT', 'ACCESS_KEY_ID', 'SECRET_ACCESS_KEY'):
                        os.environ.pop('BUILD_CACHE_R2_'+name, None)
                    with patch('sys.argv', argv):
                        assert snapshot.main() == 0
                else:
                    os.environ.update(BUILD_CACHE_R2_ENDPOINT='https://unused.invalid',
                                      BUILD_CACHE_R2_ACCESS_KEY_ID='poison', BUILD_CACHE_R2_SECRET_ACCESS_KEY='poison')
                    if status == 'corrupt':
                        body = b'x' * len(body)
                    with patch('sys.argv', argv), patch.object(snapshot, 'client', return_value=MemoryStore()):
                        if status == 'miss':
                            with patch.object(snapshot, 'get_index', return_value=None):
                                assert snapshot.main() == 0
                        else:
                            assert snapshot.main() == 0
                outcome = json.loads((evidence/'snapshot-restore.json').read_text())
                assert outcome['status'] == ('error' if status == 'corrupt' else status)
                fallback = build(status+'-target', root/(status+'-cas'))
                assert fallback['hits'] == 0 and fallback['stored_bytes'] > 0
            hot = build('hot-target', consumer)
            assert hot['hits'] > 0 and hot['restored_output_files'] > 0, hot
            # 已导入快照也不能掩盖输入变化。
            (source/'src/main.rs').write_text('fn main() { assert_eq!(3 + 3, 6); }\n')
            changed = build('changed-target', consumer)
            assert changed['hits'] == 0, changed
            assert not requests, 'Native compilation attempted S3 requests'
            print(json.dumps(dict(cold_compilations=cold['unconsulted'] + cold['misses'], snapshot_hits=hot['hits'],
                                  restored_files=hot['restored_output_files'], changed_misses=changed['misses'],
                                  native_remote_requests=len(requests))))
        finally:
            os.environ.clear()
            os.environ.update(inherited)
            stopped.set()
            thread.join()
            listener.close()


if __name__ == '__main__':
    main()
