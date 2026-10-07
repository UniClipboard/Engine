#!/usr/bin/env bash
# 用固定生成器从 uc-engine-uniffi 动态库的接口元数据生成 Go 包到 bindings/go/uc_engine_uniffi。
#
#   generate.sh <uniffi-bindgen-go 可执行文件> <libuc_engine_uniffi 动态库>
#
# 元数据来自库，不依赖库的 profile；dev 与 release 构建生成的结果必须一致。
# 只改写两个生成文件，同目录的手写文件（link.go）保持不变。生成结果提交入库，
# CI 重新生成后要求零差异；禁止手改生成文件，修正只能走 generator/*.patch。
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
module_dir="$(cd "$here/.." && pwd)"
generator="${1:?usage: generate.sh <generator> <library>}"
library="${2:?usage: generate.sh <generator> <library>}"

work="$(mktemp -d "${TMPDIR:-/tmp}/uc-go-generate.XXXXXX")"
trap 'rm -rf "$work"' EXIT
"$generator" --library "$library" --out-dir "$work"
for file in uc_engine_uniffi.go uc_engine_uniffi.h; do
  cp "$work/uc_engine_uniffi/$file" "$module_dir/uc_engine_uniffi/$file"
done
