#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PROJECT_DIR="$REPO_ROOT/tests/hosts/android"
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  TARGET_DIR="$CARGO_TARGET_DIR"
else
  TARGET_DIR="$(mktemp -d "${TMPDIR:-/tmp}/uc-android-probe.XXXXXX")"
  trap 'rm -rf -- "$TARGET_DIR"' EXIT
fi

cd "$REPO_ROOT"
CARGO_TARGET_DIR="$TARGET_DIR" cargo ndk \
  -t arm64-v8a \
  -o "$PROJECT_DIR/app/src/main/jniLibs" \
  build -p uc-mobile-probe-core --release --locked

cd "$PROJECT_DIR"
./gradlew --offline :app:assembleDebug

echo "$PROJECT_DIR/app/build/outputs/apk/debug/app-debug.apk"
