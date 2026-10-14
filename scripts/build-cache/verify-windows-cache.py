#!/usr/bin/env python3
"""原生 Windows Cargo → MBX → rustc → exe 验收；保留独立目录和证据。"""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--out', required=True)
a = p.parse_args()
if os.name != 'nt':
    p.error('必须在原生 Windows Python 中运行，不能用交叉编译代替')
out = Path(a.out).resolve()
out.mkdir(parents=True, exist_ok=True)
root = Path(tempfile.mkdtemp(prefix='mbx native cache ', dir=os.environ['RUNNER_TEMP']))
engine = Path(__file__).resolve().parents[2]
wrapper = engine / 'scripts/build-cache/mbx.sh'
bash = shutil.which('bash')
assert bash, '需要 Git for Windows Bash'
env = {k: v for k, v in os.environ.items() if not k.startswith(
    ('MBX_', 'AWS_', 'BUILD_CACHE_', 'UC_ENGINE_MBX_', 'CARGO_PROFILE_'))
    and k not in ('CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'RUSTFLAGS',
                  'CARGO_ENCODED_RUSTFLAGS', 'UC_TOOLS_DIR')}
env.update(MBX_CACHE_DIR=str(root / 'cas'), UC_TOOLS_DIR=str(root / 'tools'),
           MBX_REMOTE_URL='', MBX_REMOTE_MODE='read-only', MBX_GC_MAX_SIZE='1GiB',
           MBX_SHARE_WORKSPACE_ROOT='0', CARGO_BUILD_JOBS='2', CARGO_INCREMENTAL='0')
manifest = '''[package]
name = "cache-consumer"
version = "0.1.0"
edition = "2021"
[workspace]
[dependencies]
uc-content-hash = { path = "%s" }
[features]
alternate = []
''' % (engine / 'crates/uc-content-hash').as_posix()
source = '''fn main() {
 let hash = uc_content_hash::content_hash(b"abc");
 let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
 assert_eq!(hex, "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
 println!("{}:debug={}:flag={}:{}", cfg!(feature="alternate"), cfg!(debug_assertions), cfg!(cache_input_change), hex);
}
'''
results = []
parent = dict(os.environ)
rustc = subprocess.check_output(['rustc', '-Vv'], text=True)
expected_host = {'AMD64': 'x86_64', 'ARM64': 'aarch64'}[platform.machine().upper()]
assert 'host: ' + expected_host + '-pc-windows-msvc' in rustc, rustc
try:
    # 运行实际 CI 装配并使用它的不可变脚本快照，验证配置文件/原生子进程边界。
    ci_env = root / 'github.env'
    ci_path = root / 'github.path'
    ci_env.touch()
    ci_path.touch()
    env.update(RUNNER_TEMP=str(root), GITHUB_ENV=str(ci_env), GITHUB_PATH=str(ci_path))
    with (out / 'setup.log').open('w') as log:
        subprocess.run([bash, (engine / 'scripts/build-cache/start-ci-mbx.sh').as_posix()],
                       env=env, check=True, stdout=log, stderr=subprocess.STDOUT)
    for line in ci_env.read_text().splitlines():
        key, value = line.split('=', 1)
        env[key] = value
    env['CARGO_BUILD_JOBS'] = '2'
    wrapper = Path(env['UC_ENGINE_MBX_BIN']).parent / 'mbx.sh'
    with Path(env['UC_ENGINE_MBX_ENV_FILE']).open('a') as config:
        config.write('export UC_CACHE_CONFIG_LOADED=1\n')
    for name in ('engine', 'desktop', 'mobile'):
        repo = root / name
        (repo / 'src').mkdir(parents=True)
        (repo / 'Cargo.toml').write_text(manifest)
        (repo / 'src/main.rs').write_text(source)
    subprocess.run(['cargo', 'generate-lockfile'], cwd=root / 'engine', env=env, check=True)
    for name in ('desktop', 'mobile'):
        shutil.copy2(root / 'engine/Cargo.lock', root / name / 'Cargo.lock')
    shutil.copy2(root / 'engine/Cargo.lock', out / 'Cargo.lock')
    (out / 'fixture.rs').write_text(source)
    (out / 'fixture.toml').write_text(manifest)
    # --exec 内由原生 Python CreateProcess 找 cargo.exe；不是 Bash 的 cargo 替身。
    child = 'import os,subprocess,sys; assert os.environ.get("UC_CACHE_CONFIG_LOADED")=="1"; subprocess.run(["cargo"]+sys.argv[1:],check=True)'
    for label, repo, extra in (
        ('cold', 'engine', []), ('warm-desktop', 'desktop', []),
        ('warm-mobile', 'mobile', []), ('feature', 'mobile', ['--features', 'alternate']),
        ('profile', 'mobile', ['--release']), ('flags', 'mobile', []),
        ('source', 'mobile', []),
    ):
        if label == 'source':
            (root / repo / 'src/main.rs').write_text(source.replace('b"abc"', 'b""').replace(
                '6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85',
                'af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262'))
        target = root / ('target-' + label)
        stats = out / (label + '.json')
        runenv = dict(env, CARGO_TARGET_DIR=str(target), MBX_STATS_REPORT=str(stats),
                      RUSTC_WRAPPER=str(root / 'must-not-exist.exe'),
                      CARGO_BUILD_RUSTC_WRAPPER=str(root / 'must-not-exist.exe'))
        if label == 'flags':
            runenv['RUSTFLAGS'] = '--cfg cache_input_change'
        cmd = [bash, wrapper.as_posix(), '--exec', shutil.which('python'),
               '-c', child, 'build', '--locked', *extra]
        started = time.monotonic()
        with (out / (label + '.log')).open('w') as log:
            subprocess.run(cmd, cwd=root / repo, env=runenv, check=True,
                           stdout=log, stderr=subprocess.STDOUT, timeout=600)
        binary = target / ('release' if label == 'profile' else 'debug') / 'cache-consumer.exe'
        output = subprocess.check_output([str(binary)], text=True).strip()
        assert output.startswith(str(label == 'feature').lower() + ':')
        assert 'debug=' + str(label != 'profile').lower() in output
        assert 'flag=' + str(label == 'flags').lower() in output
        report = json.loads(stats.read_text())
        if label.startswith('warm-'):
            assert report['hits'] > 0 and report['restored_output_files'] > 0
        for key in ('downloaded_bytes', 'uploaded_bytes', 'remote_failures', 'background_upload_failures'):
            assert report[key] == 0, key
        results.append(dict(label=label, seconds=round(time.monotonic() - started, 3),
                            output=output, stats=report))
        (out / 'results.json').write_text(json.dumps(results, indent=2))
    assert dict(os.environ) == parent, '父进程环境不应发生变更'
finally:
    # 失败/超时保留 scratch；不删除缓存，不终止可能仍存活的进程。
    (out / 'provenance.json').write_text(json.dumps(dict(
        platform=platform.platform(), machine=platform.machine(), scratch=str(root),
        source=subprocess.check_output(['git', '-C', str(engine), 'rev-parse', 'HEAD'], text=True).strip(),
        rustc=subprocess.check_output(['rustc', '-Vv'], text=True),
        completed=[r['label'] for r in results],
        limitation='三个 fixture 目录使用同一个 Engine wrapper；产品调用方整合须另行原生验收。',
    ), indent=2))
