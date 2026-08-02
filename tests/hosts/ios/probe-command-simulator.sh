#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <simulator-udid> '<json-command>'" >&2
  exit 2
fi

SIMULATOR_ID="$1"
COMMAND_JSON="$2"
BUNDLE_ID="app.uniclipboard.EngineProbe"

xcrun simctl launch "$SIMULATOR_ID" "$BUNDLE_ID" >/dev/null
CONTAINER="$(xcrun simctl get_app_container "$SIMULATOR_ID" "$BUNDLE_ID" data)"
RESULT_FILE="$CONTAINER/Library/Application Support/probe-result.json"

send_and_wait() {
  local command_json="$1"
  local attempts="$2"
  local request_id
  local payload
  request_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  payload="$(printf '%s' "$command_json" | base64 | tr '+/' '-_' | tr -d '=\n')"
  rm -f "$RESULT_FILE"
  xcrun simctl openurl "$SIMULATOR_ID" \
    "ucengineprobe://command?payload=$payload&request_id=$request_id"

  for _ in $(seq 1 "$attempts"); do
    if [[ -f "$RESULT_FILE" ]] && \
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
  sleep 0.1
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
