import hashlib,json,pathlib,subprocess,os
root=pathlib.Path('/work/source');out=pathlib.Path('/work/evidence');os.chdir(root)
for variant in ['normal','fault']:
 messages=[]
 for line in (out/f'build-{variant}.jsonl').read_text().splitlines():
  try: messages.append(json.loads(line))
  except ValueError: pass
 finished=[m for m in messages if m.get('reason')=='build-finished']
 assert len(finished)==1 and finished[0].get('success') is True,(variant,finished)
expected={hashlib.sha256((out/'bin'/name).read_bytes()).hexdigest():name for name in ['normal','fault']}
records=[]
for binary in (root/'target/debug/deps').glob('uc_connectivity_host-*'):
 if not binary.is_file() or not os.access(binary,os.X_OK):continue
 h=hashlib.sha256(binary.read_bytes()).hexdigest()
 if h not in expected:continue
 suffix=binary.name.rsplit('-',1)[1]
 fingerprint=root/'target/debug/.fingerprint'/('uc-connectivity-host-'+suffix)/'bin-uc-connectivity-host.json'
 if not fingerprint.exists():continue
 raw=fingerprint.read_bytes();meta=json.loads(raw)
 (out/f'{expected[h]}-fingerprint.json').write_bytes(raw)
 records.append({'binary':expected[h],'binary_sha256':h,'deps_executable':binary.name,'fingerprint':str(fingerprint.relative_to(root)),'fingerprint_sha256':hashlib.sha256(raw).hexdigest(),'compiled_features':json.loads(meta['features']),'profile':meta['profile']})
assert {r['binary'] for r in records}==set(expected.values()),records
common={'production-e2e','join-status','device-group-choices','network-recovery','process-observability','startup-progress','transfer-progress'}
for record in records:
 wanted=common if record['binary']=='normal' else common|{'clipboard-stream-fault','network-test-probe'}
 assert set(record['compiled_features'])==wanted,record
 for package in ['uc-engine','uc-infra-p2p']:
  data=(out/f"{record['binary']}-{package}-inverse-features.txt").read_text()
  assert 'uc-infra-p2p feature "test-util"' not in data
  assert 'uc-engine feature "dev-tools"' not in data
  if record['binary']=='normal': assert 'clipboard-stream-fault' not in data and 'network-test-probe' not in data
(out/'compiled-feature-provenance.json').write_text(json.dumps({'method':'Frozen ELF SHA-256 matches Cargo deps executable and compiled fingerprint. Inverse graphs use the original feature selections.','source_manifest_sha256':hashlib.sha256(pathlib.Path('/work/candidate-source.json').read_bytes()).hexdigest(),'records':records},indent=2)+'\n')
print(json.dumps(records,indent=2))
