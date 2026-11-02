import hashlib,json,pathlib,stat,os
root=pathlib.Path('/work/source')
p=pathlib.Path('/work/candidate-source.json')
m=json.loads(p.read_bytes())
for name,v in m['files'].items():
 q=root/name
 if v['kind']=='symlink':
  assert q.is_symlink() and os.readlink(q)==v['target'],name
 else:
  assert not q.is_symlink(),name
  assert hashlib.sha256(q.read_bytes()).hexdigest()==v['sha256'],name
  assert oct(stat.S_IMODE(q.stat().st_mode))==v['mode'],name
print(json.dumps({'manifest_sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'verified_items':len(m['files']),'result':'PASS'}))
