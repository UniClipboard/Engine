#!/usr/bin/env bash
# Replay synthetic storage scenarios and retain their original exit status.
set -euo pipefail
cd "$(dirname "$0")/../.."
root="${BLOB_PERSISTENCE_EVIDENCE:-target/test-artifacts/blob-persistence/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
export BLOB_PERSISTENCE_EVIDENCE="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$root")"
mkdir -p "$BLOB_PERSISTENCE_EVIDENCE"
python3 - "$BLOB_PERSISTENCE_EVIDENCE" <<'PY'
import hashlib,json,platform,subprocess,sys
from pathlib import Path
root=Path(sys.argv[1])
def command(*args):
    return subprocess.check_output(args,text=True).strip()
files=['Cargo.lock','third_party/iroh/Cargo.lock',
       'third_party/iroh/iroh-blobs/src/store/fs.rs',
       'third_party/iroh/iroh-blobs/src/store/fs/persistence_tests.rs']
(root/'provenance.json').write_text(json.dumps({
    'engine':command('git','rev-parse','HEAD'),
    'network_stack':command('git','-C','third_party/iroh','rev-parse','HEAD'),
    'engine_status':command('git','status','--porcelain'),
    'network_status':command('git','-C','third_party/iroh','status','--porcelain'),
    'platform':platform.platform(),
    'rustc':command('rustc','-Vv'),
    'sha256':{p:hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in files},
},indent=2)+'\n')
PY
set +e
just cargo test --manifest-path third_party/iroh/Cargo.toml --target-dir "$PWD/target" \
  --locked -p iroh-blobs --lib store::fs::persistence_tests -- --nocapture \
  > "$BLOB_PERSISTENCE_EVIDENCE/test.log" 2>&1
result=$?
set -e
python3 - "$BLOB_PERSISTENCE_EVIDENCE" "$result" <<'PY'
import json,sys
from pathlib import Path
root=Path(sys.argv[1]); code=int(sys.argv[2])
cases={p.stem:json.loads(p.read_text()) for p in root.glob('*.json') if p.name not in ('provenance.json','result.json')}
(root/'result.json').write_text(json.dumps({'exit_code':code,'cases':cases},indent=2)+'\n')
print('blob persistence evidence:',root,'exit:',code)
PY
exit "$result"
