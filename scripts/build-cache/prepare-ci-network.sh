#!/usr/bin/env bash
# MBX 并发 S3 请求不能让各连接的自动缓冲增长耗尽 macOS 的共享 mbuf 池。
# 仅调整用完即销毁的 GitHub 托管 runner；不改本机、self-hosted 或持久系统配置。
set -euo pipefail
if [[ "$(uname -s)" != Darwin ]]; then exit 0; fi
if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_ENVIRONMENT:-}" != github-hosted ]]; then
  echo 'MBX network buffer preparation only applies to GitHub-hosted macOS runners'
  exit 0
fi

# MBX 1.18.0 最多并发 64 个远端传输；每方向 256 KiB 给缓存之外的连接和 IPC 留出余量。
# 保留镜像中更小的限制，不增加任何连接的缓冲预算；不关闭 TCP 自动调节。
for setting in net.inet.tcp.autorcvbufmax net.inet.tcp.autosndbufmax; do
  current=$(sysctl -n "$setting")
  case "$current" in
    '' | *[!0-9]*) echo "Invalid numeric sysctl: $setting" >&2; exit 1 ;;
  esac
  if (( current > 262144 )); then
    sudo -n sysctl -w "$setting=262144"
  else
    printf '%s: %s (already within budget)\n' "$setting" "$current"
  fi
done
