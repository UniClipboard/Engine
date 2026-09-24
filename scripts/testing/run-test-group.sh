#!/usr/bin/env bash
set -euo pipefail

readonly NEXTEST_VERSION="0.9.145"
readonly GROUP="${1:-}"

if [[ -z "${GROUP}" ]]; then
  printf 'usage: %s <fast|evidence|persistence-provider|engine-smoke|process|membership-e2e|real-network|device> [group arguments]\n' "$0" >&2
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

artifact_root() {
  local root="${UC_TEST_ARTIFACTS_DIR:-target/test-artifacts/$(date -u +%Y%m%dT%H%M%SZ)-$$}"
  if [[ "${root}" != /* ]]; then
    root="${PWD}/${root}"
  fi
  printf '%s\n' "${root}"
}

require_scenario_result() {
  local root="$1"
  local scenario="$2"
  local result
  result="$(find "${root}" -mindepth 2 -maxdepth 2 -type f -path "*/${scenario}-seed-*/result.json" -print -quit)"
  if [[ -z "${result}" ]]; then
    printf 'missing structured result for scenario %s under %s\n' "${scenario}" "${root}" >&2
    exit 1
  fi
}

case "${GROUP}" in
  fast)
    if [[ $# -ne 0 ]]; then
      printf 'fast does not accept additional arguments\n' >&2
      exit 2
    fi
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    run_nextest \
      -p uc-testkit \
      -p uc-application \
      -E 'package(uc-testkit) | package(uc-application) & (test(admission_recovery_scenarios) | test(device_trust_recovery_scenario) | test(legacy_candidate_convergence_scenario) | test(virtual_membership_network) | test(file_transfer_completion_scenario_reports_final_state) | test(text_transfer_scenario))'
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- success
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- failure
    printf 'testkit artifacts: %s\n' "${artifact_root}"
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    ;;
  evidence)
    if [[ $# -ne 0 ]]; then
      printf 'evidence does not accept additional arguments\n' >&2
      exit 2
    fi
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    run_nextest \
      -p uc-testkit \
      -p uc-application \
      -p uc-infra \
      -E 'package(uc-testkit) | package(uc-application) & (test(admission_recovery_scenarios) | test(device_trust_recovery_scenario) | test(legacy_candidate_convergence_scenario) | test(virtual_membership_network) | test(file_transfer_completion_scenario_reports_final_state) | test(text_transfer_scenario)) | package(uc-infra) & (test(provider_dependency_evidence) | binary(profile_storage_upgrade_crash))'
    require_scenario_result "${artifact_root}" "text-transfer-dispatch"
    require_scenario_result "${artifact_root}" "file-transfer-completion"
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- success
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- failure
    printf 'testkit artifacts: %s\n' "${artifact_root}"
    printf 'membership artifacts: target/test-artifacts/membership-recovery\n'
    printf 'real dependency artifacts: target/test-artifacts/real-dependencies\n'
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    ;;
  persistence-provider)
    run_nextest \
      -p uc-infra \
      -E 'package(uc-infra) & (binary(membership_record) | binary(profile_storage_upgrade) | binary(space_admission_state) | test(provider_dependency_evidence))' \
      "$@"
    ;;
  engine-smoke)
    run_nextest -p uc-engine --test public_contract "$@"
    ;;
  process)
    run_nextest \
      -p uc-engine \
      -p uc-infra \
      -E 'package(uc-engine) & binary(host_contract) | package(uc-infra) & binary(profile_storage_upgrade_crash)' \
      "$@"
    ;;
  membership-e2e)
    # 成员多设备场景：每项启动多个完整 Engine，经本机回环 QUIC 通信；测试文件只在 dev-tools 下编译。
    run_nextest \
      -p uc-engine \
      --features dev-tools \
      --test space_membership_auto_pairing_e2e \
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
