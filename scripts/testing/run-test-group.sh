#!/usr/bin/env bash
set -euo pipefail

readonly NEXTEST_VERSION="0.9.145"
readonly GROUP="${1:-}"

if [[ -z "${GROUP}" ]]; then
  printf 'usage: %s <fast|persistence-provider|engine-smoke|process|real-network|device> [group arguments]\n' "$0" >&2
  exit 2
fi
shift

require_nextest() {
  local installed
  if ! installed="$(cargo nextest --version 2>/dev/null)"; then
    printf 'cargo-nextest %s is required; install it with:\n' "${NEXTEST_VERSION}" >&2
    printf '  cargo install cargo-nextest --locked --version %s\n' "${NEXTEST_VERSION}" >&2
    exit 2
  fi
  if [[ "${installed}" != *"${NEXTEST_VERSION}"* ]]; then
    printf 'cargo-nextest %s is required, found: %s\n' "${NEXTEST_VERSION}" "${installed}" >&2
    exit 2
  fi
}

run_nextest() {
  require_nextest
  cargo nextest run --profile ci --locked "$@"
}

case "${GROUP}" in
  fast)
    if [[ $# -ne 0 ]]; then
      printf 'fast does not accept additional arguments\n' >&2
      exit 2
    fi
    artifact_root="${UC_TEST_ARTIFACTS_DIR:-target/test-artifacts/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    run_nextest -p uc-testkit
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- success
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- failure
    printf 'testkit artifacts: %s\n' "${artifact_root}"
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    ;;
  persistence-provider)
    run_nextest -p uc-infra \
      --test membership_ledger \
      --test profile_storage_upgrade \
      --test space_admission_state \
      "$@"
    ;;
  engine-smoke)
    run_nextest -p uc-engine --test public_contract "$@"
    ;;
  process)
    run_nextest \
      -E 'package(uc-engine) & binary(host_contract) | package(uc-infra) & binary(profile_storage_upgrade_crash)' \
      "$@"
    ;;
  real-network)
    exec bash scripts/testing/run-connection-recovery-e2e.sh "$@"
    ;;
  device)
    printf 'device tests require an explicit platform host and attached target.\n' >&2
    printf 'available host: tests/hosts/uc-mobile-probe-core\n' >&2
    exit 2
    ;;
  *)
    printf 'unknown test group: %s\n' "${GROUP}" >&2
    exit 2
    ;;
esac
