import hashlib,json,pathlib,subprocess,tempfile,shutil,select,time
binary='/work/evidence/bin/normal'
root=tempfile.mkdtemp(prefix='uc-cfg-off-',dir='/dev/shm')
p=subprocess.Popen([binary],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True,bufsize=1)
receipt={'binary_sha256':hashlib.sha256(pathlib.Path(binary).read_bytes()).hexdigest(),'commands':[]}
def call(value):
 p.stdin.write(json.dumps(value)+'\n');p.stdin.flush()
 due=time.monotonic()+45
 while time.monotonic()<due:
  ready,_,_=select.select([p.stdout],[],[],max(0,due-time.monotonic()))
  assert ready,'host_response_timeout'
  line=p.stdout.readline()
  assert line,'host_eof'
  try: response=json.loads(line)
  except ValueError: continue
  if response.get('uc_connectivity')==1:return response
 raise AssertionError('host_response_timeout')
try:
 start=call({'root':root})
 assert start.get('ready') is True,'host_not_ready'
 caps=start['host']['capabilities'];receipt['capabilities']=caps
 assert 'clipboard-stream-fault' not in caps and 'network-test-probe' not in caps,caps
 for command in ['stream_fault','address_probe','subnet_probe']:
  response=call({'command':command,'action':'snapshot'})
  assert response.get('error')=='operation_failed' and 'ok' not in response,command
  receipt['commands'].append({'command':command,'rejected':True,'error':response['error']})
 stopped=call({'command':'shutdown'})
 assert 'ok' in stopped,'shutdown_failed'
 p.stdin.close();assert p.wait(timeout=30)==0,'host_exit_failed'
 receipt['shutdown_complete']=True
finally:
 if p.poll() is None:
  p.kill();p.wait(timeout=10)
 shutil.rmtree(root)
 receipt['profile_removed']=not pathlib.Path(root).exists()
assert receipt['profile_removed']
receipt['result']='PASS'
pathlib.Path('/work/evidence/feature-off-process.json').write_text(json.dumps(receipt,indent=2)+'\n')
print('feature-off process acceptance: PASS')
