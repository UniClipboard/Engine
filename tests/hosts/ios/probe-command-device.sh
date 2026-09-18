#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <device-id> '<json-command>'" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
DEVICE_ID="$1"
COMMAND_JSON="$2"
BUNDLE_ID="app.uniclipboard.EngineProbe"
RESULT_DIR="$REPO_ROOT/target/ios-device-probe-results"
RESULT_FILE="$RESULT_DIR/probe-result.json"

mkdir -p "$RESULT_DIR"
xcrun devicectl device process launch \
  --device "$DEVICE_ID" \
  "$BUNDLE_ID" >/dev/null

send_and_wait() {
  local command_json="$1"
  local attempts="$2"
  local request_id
  local payload
  request_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  payload="$(printf '%s' "$command_json" | base64 | tr '+/' '-_' | tr -d '=\n')"
  xcrun devicectl device process launch \
    --device "$DEVICE_ID" \
    --payload-url "ucengineprobe://command?payload=$payload&request_id=$request_id" \
    "$BUNDLE_ID" >/dev/null

  for _ in $(seq 1 "$attempts"); do
    rm -f "$RESULT_FILE"
    if xcrun devicectl device copy from \
      --device "$DEVICE_ID" \
      --domain-type appDataContainer \
      --domain-identifier "$BUNDLE_ID" \
      --source "Library/Application Support/probe-result.json" \
      --destination "$RESULT_FILE" >/dev/null 2>&1 && \
      [[ "$(jq -r '.request_id // empty' "$RESULT_FILE")" == "$request_id" ]]; then
      cat "$RESULT_FILE"
      return 0
    fi
    sleep 0.1
  done
  return 1
}

READY=false
for _ in $(seq 1 30); do
  if readiness="$(send_and_wait '{"command":"event_summary"}' 30)" && \
    [[ "$(printf '%s' "$readiness" | jq -r '.ok')" == true ]]; then
    READY=true
    break
  fi
done

if [[ "$READY" != true ]]; then
  echo "probe did not start" >&2
  exit 1
fi

if result="$(send_and_wait "$COMMAND_JSON" 900)"; then
  printf '%s\n' "$result"
  exit 0
fi

echo "probe command timed out" >&2
exit 1
