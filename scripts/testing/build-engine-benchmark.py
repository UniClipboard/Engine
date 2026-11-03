#!/usr/bin/env python3
"""Serial release builds with shared MBX and reclaimed baseline build storage."""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import tomllib

p=argparse.ArgumentParser()
p.add_argument('--baseline',default='origin/main')
p.add_argument('--output',required=True,type=pathlib.Path)
p.add_argument('--isolation-root',required=True,type=pathlib.Path,
               help='Explicit external directory for the temporary baseline worktree and target')
a=p.parse_args()
root=pathlib.Path.cwd()
assert (root/'Cargo.toml').is_file()
a.output=a.output.resolve()
a.output.mkdir(parents=True,exist_ok=False)
a.isolation_root.mkdir(parents=True,exist_ok=True)
snapshot=pathlib.Path(tempfile.mkdtemp(prefix='uc-benchmark-',dir=a.isolation_root))
baseline_target=snapshot.parent/(snapshot.name+'-target')
assert not baseline_target.exists()

def command(*args,cwd=root):
    return subprocess.check_output(args,text=True,cwd=cwd).strip()

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

host_source=hashlib.sha256()
for path in sorted((root/'tests/hosts/connectivity/src').glob('*.rs')):
    host_source.update(path.name.encode());host_source.update(path.read_bytes())
revision=command('git','rev-parse',a.baseline)
registered=False
try:
    subprocess.run(['git','worktree','add','--detach',str(snapshot),revision],check=True)
    registered=True
    subprocess.run(['git','-c','url.https://github.com/.insteadOf=git@github.com:','submodule','update','--init','--recursive'],cwd=snapshot,check=True)
    # 两个 Engine 使用相同宿主源码；不复制生产 crate。
    shutil.copytree(root/'tests/hosts/connectivity/src',snapshot/'tests/hosts/connectivity/src',dirs_exist_ok=True)
    manifest=snapshot/'tests/hosts/connectivity/Cargo.toml'
    if 'async-trait =' not in manifest.read_text():
        manifest.write_text(manifest.read_text().replace('[dependencies]\n','[dependencies]\nasync-trait = "0.1"\n'))
    lock=snapshot/'Cargo.lock'
    content=lock.read_text()
    start=content.index('name = "uc-connectivity-host"')
    end=content.index('\n[[package]]',start)
    if '"async-trait"' not in content[start:end]:
        content=content[:start]+content[start:end].replace('dependencies = [\n','dependencies = [\n "async-trait",\n')+content[end:]
    lock.write_text(content)
    for label,source,commit in [('baseline',snapshot,revision),('candidate',root,command('git','rev-parse','HEAD'))]:
        environment=dict(os.environ)
        if label=='baseline': environment['CARGO_TARGET_DIR']=str(baseline_target)
        metadata=json.loads(subprocess.check_output(['just','cargo','metadata','--locked','--no-deps','--format-version','1'],cwd=source,env=environment))
        target=pathlib.Path(metadata['target_directory'])
        if label=='baseline': assert target.resolve()==baseline_target.resolve(), 'unexpected isolated target'
        subprocess.run(['just','cargo','build','--release','--locked','-p','uc-connectivity-host'],cwd=source,env=environment,check=True)
        binary=a.output/f'{label}-host'
        shutil.copy2(target/'release/uc-connectivity-host',binary)
        provenance={'source_commit':commit,'dirty':bool(command('git','status','--porcelain',cwd=source)),
                    'engine_source_dirty':bool(command('git','status','--porcelain','--','crates','bindings','compatibility','third_party',cwd=source)),
                    'host_source_overlay':label=='baseline',
                    'source_diff_sha256':hashlib.sha256(subprocess.check_output(['git','diff','HEAD'],cwd=source)).hexdigest(),
                    'host_source_sha256':host_source.hexdigest(),'binary_sha256':digest(binary),'lock_sha256':digest(source/'Cargo.lock'),
                    'rustc':command('rustc','-Vv'),'profile':'release','features':'connectivity-host/default;uc-engine/dev-tools',
                    'release_profile':tomllib.loads((source/'Cargo.toml').read_text())['profile']['release'],
                    'cargo_config_sha256':digest(source/'.cargo/config.toml'),
                    'rustflags_sha256':hashlib.sha256(environment.get('RUSTFLAGS','').encode()).hexdigest(),
                    'build_command':'just cargo build --release --locked -p uc-connectivity-host',
                    'network_submodule':command('git','-C','third_party/iroh','rev-parse','HEAD',cwd=source)}
        binary.with_suffix('.manifest.json').write_text(json.dumps(provenance,indent=2))
    subprocess.run(['just','cargo','build','--release','--locked','-p','uc-benchmark'],check=True)
    target=pathlib.Path(json.loads(command('just','cargo','metadata','--no-deps','--format-version','1'))['target_directory'])
    shutil.copy2(target/'release/uc-benchmark',a.output/'uc-benchmark')
finally:
    # 子进程均由同步 run 等待完成；仅回收本次创建的专属路径，不接触共享缓存或候选 target。
    active=command('ps','-axo','args=')
    if any(str(baseline_target) in line and any(tool in line for tool in ['rustc','cargo','mbx']) for line in active.splitlines()):
        raise RuntimeError('isolated build still active; storage retained')
    if registered:
        subprocess.run(['git','worktree','remove','--force',str(snapshot)],check=True)
    elif snapshot.exists(): shutil.rmtree(snapshot)
    if baseline_target.exists(): shutil.rmtree(baseline_target)
