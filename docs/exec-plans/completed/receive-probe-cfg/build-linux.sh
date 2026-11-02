#!/bin/bash
set -euo pipefail
cd /work/source
export PATH=/usr/local/cargo/bin:$PATH
source scripts/build-cache/env.sh
export UC_ENGINE_MBX_LOCAL_ONLY=1 CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=0 MBX_CACHE_DIR=/root/.cache/mise/mbx
export UC_HOST_ANCHOR=receive-probe-cfg UC_HOST_ENGINE_REV=b5537ad1232549fce71c5039422ea9f952c3eebe
mkdir -p /work/evidence/bin
python3 /work/verify-source.py > /work/evidence/source-before.json
for variant in normal fault; do
 features=production-e2e
 if [[ "$variant" == fault ]]; then features=production-e2e,clipboard-stream-fault,network-test-probe; fi
 cargo build -p uc-connectivity-host --no-default-features --features "$features" --locked --message-format=json > "/work/evidence/build-$variant.jsonl" 2> "/work/evidence/build-$variant.log"
 cp target/debug/uc-connectivity-host "/work/evidence/bin/$variant"
 for package in uc-engine uc-infra-p2p; do
  cargo tree --locked -p uc-connectivity-host --no-default-features --features "$features" -e features -i "$package" > "/work/evidence/$variant-$package-inverse-features.txt"
 done
 python3 /work/verify-source.py > "/work/evidence/source-after-$variant.json"
 echo "built $variant"
done
sha256sum /work/evidence/bin/* > /work/evidence/binary-hashes.txt
