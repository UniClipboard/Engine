#!/usr/bin/env python3
"""仅复用通过本轮 apt 索引校验的 deb，不缓存索引或已安装状态。"""

import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys


def parse_downloads(output):
    downloads = {}
    for line in output.splitlines():
        if not line.startswith("'"):
            continue
        uri, name, size, digest = shlex.split(line)
        if Path(name).name != name or not name.endswith('.deb'):
            raise ValueError('Invalid apt archive filename')
        if not re.fullmatch(r'SHA256:[0-9a-fA-F]{64}', digest):
            raise ValueError('Apt download must have a SHA256 hash')
        downloads[name] = [uri, int(size), digest.split(':')[1].lower()]
    return downloads


def verify_downloads(archives, downloads):
    reused = 0
    for archive in archives.glob('*.deb'):
        expected = downloads.get(archive.name)
        # 缓存不具备信任权：拒绝链接，丢弃过期、损坏或不在本轮计划内的包。
        valid = not archive.is_symlink() and archive.is_file() and expected is not None
        if valid:
            valid = archive.stat().st_size == expected[1]
        if valid:
            with archive.open('rb') as source:
                digest = hashlib.sha256()
                for block in iter(lambda: source.read(1024 * 1024), b''):
                    digest.update(block)
            valid = digest.hexdigest() == expected[2]
        if valid:
            reused += 1
        else:
            archive.unlink()
    print(f'Verified cached apt downloads: {reused}/{len(downloads)}', flush=True)


def cache_key(packages, downloads, platform, config, sources):
    data = [packages, downloads, platform, config, sources]
    return hashlib.sha256(json.dumps(data, sort_keys=True).encode()).hexdigest()


def main():
    packages = os.environ['APT_PACKAGES'].split()
    if not packages or any(not re.fullmatch(r'[a-z0-9][a-z0-9+.-]*(?::[a-z0-9-]+)?', p) for p in packages):
        raise ValueError('Expected apt package names')
    root = Path(os.environ['RUNNER_TEMP']) / 'engine-apt'
    archives = root / 'archives'
    archives.mkdir(parents=True, exist_ok=True)
    manifest = root / 'downloads.json'
    env = dict(os.environ, LC_ALL='C')
    apt = ['sudo', 'apt-get', '-o', f'Dir::Cache::archives={archives}',
           '-o', 'Acquire::ForceHash=SHA256', '-o', 'APT::Keep-Downloaded-Packages=true']
    if sys.argv[1] == 'prepare':
        # 即使命中缓存也刷新索引，保持原先的版本选择与签名验证。
        subprocess.run(['sudo', 'apt-get', 'update'], env=env, check=True)
        output = subprocess.check_output(apt + ['--print-uris', '--yes', 'install', *packages], env=env, text=True)
        downloads = parse_downloads(output)
        manifest.write_text(json.dumps(downloads))
        sources = {str(p): p.read_text() for p in sorted(Path('/etc/apt').rglob('*'))
                   if p.is_file() and p.suffix in ('.list', '.sources')}
        platform = [Path('/etc/os-release').read_text(),
                    subprocess.check_output(['dpkg', '--print-architecture'], text=True)]
        config = subprocess.check_output(['apt-config', 'dump'], text=True)
        key = cache_key(packages, downloads, platform, config, sources)
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output_file:
            output_file.write(f'key={key}\n')
    elif sys.argv[1] == 'install':
        verify_downloads(archives, json.loads(manifest.read_text()))
        # 不跳过安装，也不以 dpkg -i 绕过 apt；缺失或不合格的缓存由 apt 正常下载。
        subprocess.run(apt + ['--yes', 'install', *packages], env=env, check=True)
    else:
        raise ValueError('Expected prepare or install')


if __name__ == '__main__':
    main()
