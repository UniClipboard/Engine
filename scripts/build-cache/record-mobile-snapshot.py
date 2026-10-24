"""保留真实产品与缓存自动化来源，不能把临时覆盖声称为 clean 构建。"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

root = Path(sys.argv[1])
files = ['scripts/build-cache/' + name for name in ('mbx.sh', 'env.sh', 'bin/cargo')]
changed = subprocess.check_output(['git', 'diff', '--name-only'], cwd=root, text=True).splitlines()
assert set(changed) <= set(files)
evidence = dict(product_source=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
                automation_source=os.environ['GITHUB_SHA'], modified_cache_files=changed,
                hashes={name: hashlib.sha256((root/name).read_bytes()).hexdigest() for name in files},
                product_tree_clean=not changed)
(Path(os.environ['RUNNER_TEMP'])/'evidence/cache-entry.json').write_text(json.dumps(evidence, indent=2))
