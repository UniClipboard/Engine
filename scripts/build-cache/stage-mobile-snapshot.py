"""只修改本次临时 Mobile 驱动，在不可变 Engine worktree 中装配当前缓存入口。"""
import hashlib
import json
from pathlib import Path
import sys

root = Path(sys.argv[1])
script = root / 'scripts/update-unified-engine-core.sh'
original = script.read_text()
anchor = '  git -C "$engine_root" worktree add --detach "$worktree" "$pin_commit"\n'
if original.count(anchor) != 1:
    raise ValueError('Mobile worktree creation contract changed')
injection = '''  for entry in mbx.sh env.sh bin/cargo; do
    cp "$UC_ENGINE_SNAPSHOT_AUTOMATION/scripts/build-cache/$entry" "$worktree/scripts/build-cache/$entry"
  done
  python3 "$UC_ENGINE_SNAPSHOT_AUTOMATION/scripts/build-cache/record-mobile-snapshot.py" "$worktree"
'''
script.write_text(original.replace(anchor, anchor + injection))
print(json.dumps(dict(driver_original_sha256=hashlib.sha256(original.encode()).hexdigest(),
                      driver_staged_sha256=hashlib.sha256(script.read_bytes()).hexdigest())))
