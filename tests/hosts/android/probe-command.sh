#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || -z "${ANDROID_SERIAL:-}" ]]; then
  echo "usage: ANDROID_SERIAL=<emulator-serial> $0 '<json-command>'" >&2
  exit 2
fi

adb -s "$ANDROID_SERIAL" shell am start -W \
  -n app.uniclipboard.engineprobe/.ProbeActivity >/dev/null

COMMAND_BASE64="$(printf '%s' "$1" | base64 | tr -d '\n')"
adb -s "$ANDROID_SERIAL" shell am broadcast \
  --receiver-foreground \
  -n app.uniclipboard.engineprobe/.ProbeReceiver \
  -a app.uniclipboard.engineprobe.COMMAND \
  --es command_base64 "$COMMAND_BASE64"
