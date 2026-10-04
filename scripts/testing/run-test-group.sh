#!/usr/bin/env bash
set -euo pipefail

readonly NEXTEST_VERSION="0.9.145"
readonly GROUP="${1:-}"
readonly BUILD_SCOPE="${UC_TEST_BUILD_SCOPE:-group}"

if [[ -z "${GROUP}" ]]; then
  printf 'usage: %s <workspace|fast|evidence|persistence-provider|engine-smoke|process|membership-e2e|upgrade-matrix|real-network|device> [group arguments]\n' "$0" >&2
  exit 2
fi
shift

if [[ "${BUILD_SCOPE}" != group && "${BUILD_SCOPE}" != workspace ]]; then
  printf 'UC_TEST_BUILD_SCOPE must be group or workspace\n' >&2
  exit 2
fi

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

# 组的构建范围与选择条件分开声明：本地默认只构建本组涉及的包；CI 设置
# UC_TEST_BUILD_SCOPE=workspace 后，各组统一按整个工作区构建，同一 job 内依次运行多个组也只编译一次。
# 调用方追加的 -E/--filterset 与组条件取交集；nextest 对多个 -E 取并集，不能直接追加。
# 用法：run_group <组构建参数数量> <组构建参数...> <组过滤条件> [调用方参数...]
run_group() {
  local count="$1"
  shift
  local -a group_build=("${@:1:count}")
  shift "${count}"
  local filter="$1"
  shift
  local -a passthrough=()
  while (($#)); do
    case "$1" in
      -E | --filterset | --filter-expr)
        filter="(${filter}) & (${2})"
        shift 2
        ;;
      -E=* | --filterset=* | --filter-expr=*)
        filter="(${filter}) & (${1#*=})"
        shift
        ;;
      *)
        passthrough+=("$1")
        shift
        ;;
    esac
  done
  local -a build=("${group_build[@]}")
  if [[ "${BUILD_SCOPE}" == workspace ]]; then
    build=(--workspace --all-targets)
  fi
  require_nextest
  cargo nextest run --profile ci --locked "${build[@]}" -E "${filter}" ${passthrough[@]+"${passthrough[@]}"}
}

# 统一构建范围下直接执行已构建的示例，避免按单包重新解析特性后重复编译。
run_testkit_demo() {
  if [[ "${BUILD_SCOPE}" == workspace ]]; then
    cargo build --quiet --locked --workspace --examples
    target/debug/examples/scenario_demo "$1"
  else
    cargo run --quiet --locked -p uc-testkit --example scenario_demo -- "$1"
  fi
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
  workspace)
    # PR 必需门禁：全工作区测试，基准测试只由 cargo check 验证编译，完整成员多设备场景属于 nightly；
    # 升级兼容矩阵依赖先构建的旧版宿主，只经 upgrade-matrix 分组运行。
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    run_group 2 --workspace --all-targets \
      'not kind(bench) & not (package(uc-engine) & binary(space_membership_auto_pairing_e2e)) & not package(uc-upgrade-matrix)' \
      "$@"
    printf 'workspace artifacts: %s\n' "${artifact_root}"
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    ;;
  fast)
    if [[ $# -ne 0 ]]; then
      printf 'fast does not accept additional arguments\n' >&2
      exit 2
    fi
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    run_group 4 -p uc-testkit -p uc-application \
      'package(uc-testkit) | package(uc-application) & (test(admission_recovery_scenarios) | test(device_trust_recovery_scenario) | test(legacy_candidate_convergence_scenario) | test(virtual_membership_network) | test(file_transfer_completion_scenario_reports_final_state) | test(text_transfer_scenario))'
    run_testkit_demo success
    run_testkit_demo failure
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
    run_group 6 -p uc-testkit -p uc-application -p uc-infra-p2p -p uc-infra-profile \
      'package(uc-testkit) | package(uc-application) & (test(admission_recovery_scenarios) | test(device_trust_recovery_scenario) | test(legacy_candidate_convergence_scenario) | test(virtual_membership_network) | test(file_transfer_completion_scenario_reports_final_state) | test(text_transfer_scenario)) | package(uc-infra-p2p) & test(provider_dependency_evidence) | package(uc-infra-profile) & binary(profile_storage_upgrade_crash)'
    require_scenario_result "${artifact_root}" "text-transfer-dispatch"
    require_scenario_result "${artifact_root}" "file-transfer-completion"
    run_testkit_demo success
    run_testkit_demo failure
    printf 'testkit artifacts: %s\n' "${artifact_root}"
    printf 'membership artifacts: target/test-artifacts/membership-recovery\n'
    printf 'real dependency artifacts: target/test-artifacts/real-dependencies\n'
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    ;;
  persistence-provider)
    run_group 2 -p uc-infra-profile -p uc-infra-p2p \
      'package(uc-infra-profile) & (binary(membership_record) | binary(profile_storage_upgrade) | binary(space_admission_state)) | package(uc-infra-p2p) & test(provider_dependency_evidence)' \
      "$@"
    ;;
  engine-smoke)
    run_group 4 -p uc-engine --test public_contract \
      'package(uc-engine) & binary(public_contract)' \
      "$@"
    ;;
  process)
    run_group 4 -p uc-engine -p uc-infra-profile \
      'package(uc-engine) & binary(host_contract) | package(uc-infra-profile) & binary(profile_storage_upgrade_crash)' \
      "$@"
    ;;
  membership-e2e)
    # 成员多设备场景：每项启动多个完整 Engine，经本机回环 QUIC 通信；测试文件只在 dev-tools 下编译。
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    status=0
    run_group 6 -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e \
      'package(uc-engine) & binary(space_membership_auto_pairing_e2e)' \
      "$@" || status=$?
    printf 'scenario artifacts: %s/membership-e2e\n' "${artifact_root}"
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    exit "${status}"
    ;;
  upgrade-matrix)
    # 升级兼容矩阵（各锚点到当前源码与 D1 完整链）：旧版宿主按 rev 缓存，入口自行确保所需锚点已构建；
    # --smoke 只跑上一个锚点到当前源码，--dimension d1|d2|d3|d4 只跑一个维度（CI 分片），其余参数原样交给 nextest。
    smoke=false
    dimension=
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --smoke) smoke=true; shift ;;
        --dimension)
          [[ "${2:-}" =~ ^d[1-4]$ ]] || { printf -- '--dimension expects d1, d2, d3 or d4\n' >&2; exit 2; }
          dimension=$2
          shift 2
          ;;
        *) break ;;
      esac
    done
    artifact_root="$(artifact_root)"
    export UC_TEST_ARTIFACTS_DIR="${artifact_root}"
    UC_UPGRADE_TARGET_DIR="$(cargo metadata --locked --no-deps --format-version 1 |
      node -e 'let s="";process.stdin.on("data",x=>s+=x).on("end",()=>process.stdout.write(JSON.parse(s).target_directory))')"
    export UC_UPGRADE_TARGET_DIR
    cargo build --locked -p uc-connectivity-host
    filter='package(uc-upgrade-matrix)'
    if [[ "${smoke}" == true ]]; then
      latest="$(node -e 'const a=JSON.parse(require("fs").readFileSync("tests/upgrade-matrix/anchors.json","utf8")).anchors;process.stdout.write(a[a.length-1].id)')"
      bash scripts/testing/build-upgrade-anchors.sh "${latest}"
      filter="${filter} & test(/::${latest}_to_head(_old_inviter|_new_inviter)?\$/)"
    else
      bash scripts/testing/build-upgrade-anchors.sh --all
    fi
    if [[ -n "${dimension}" ]]; then
      filter="${filter} & test(/^${dimension}::/)"
    fi
    summary_mode=()
    if [[ "${smoke}" == true || -n "${dimension}" || $# -gt 0 ]]; then
      summary_mode=(--partial)
    fi
    status=0
    run_group 2 -p uc-upgrade-matrix "${filter}" "$@" || status=$?
    node scripts/testing/summarize-upgrade-matrix.mjs "${artifact_root}/upgrade-matrix" \
      ${summary_mode[@]+"${summary_mode[@]}"} || status=$?
    printf 'matrix artifacts: %s/upgrade-matrix\n' "${artifact_root}"
    printf 'nextest JUnit: target/nextest/ci/junit.xml\n'
    exit "${status}"
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
