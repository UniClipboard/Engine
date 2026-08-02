#!/usr/bin/env bash
set -euo pipefail

if [[ -z "${ANDROID_SERIAL:-}" ]]; then
  echo "usage: ANDROID_SERIAL=<emulator-serial> $0" >&2
  exit 2
fi

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APK="$($PROJECT_DIR/build-emulator.sh | tail -n 1)"

adb -s "$ANDROID_SERIAL" install -r "$APK"
