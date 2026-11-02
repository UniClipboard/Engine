#!/bin/bash
set -euo pipefail
trap 'code=$?; printf "%s\n" "$code" > /work/evidence/e2e-exit-code.txt' EXIT
cd /work/source
python3 /work/record-provenance.py > /work/evidence/provenance-console.json
python3 /work/check-feature-off.py > /work/evidence/feature-off-console.txt
node scripts/testing/pr343-e2e.mjs --host /work/evidence/bin/normal --case L04 --repeat 3 --evidence /work/evidence/normal-l04 > /work/evidence/normal-e2e-console.jsonl 2>&1
node scripts/testing/pr343-e2e.mjs --host /work/evidence/bin/fault --case R01,R02,R02A,R03,R04 --repeat 3 --evidence /work/evidence/recovery-causal > /work/evidence/causal-e2e-console.jsonl 2>&1
python3 /work/verify-source.py > /work/evidence/source-after-e2e.json
