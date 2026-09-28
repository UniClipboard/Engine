# 由 build-android-aar.sh 与 build-ios-xcframework.sh 引入，不单独执行。
#
# 发布库会把依赖源码路径写进 panic 位置与 tracing 事件：依赖位于 CARGO_HOME，
# 装有 rust-src 时标准库路径还会指向本机 rustup。ring 在 Apple 目标上用 -gfull
# 编译 C 与汇编，iOS 静态库保留的对象因此带有 CARGO_HOME 下的 DWARF 源路径。
# 这些路径都随检出、用户和机器变化，使同一源码构建出不同字节。发布构建把它们
# 重映射为固定前缀；工作区代码本来就是相对路径。开发构建保留本机路径，便于调试器
# 直接定位源码。
#
# 调用前须位于被构建源码的根目录（工具链按该目录选择），并已设置 REPO_ROOT、
# TARGET_DIR 与 BUILD_PROFILE。打包工具可构建其他源码目录，因此本文件随脚本取得。

RELEASE_PATH_REMAP_CARGO_ARGS=()
RELEASE_PATH_REMAP_ENV=()
RELEASE_PATH_REMAP_PREFIXES=()

if [[ "$BUILD_PROFILE" == "release" ]]; then
  release_cargo_home="$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd)"
  # Cargo 传给 rustc 的是 CARGO_HOME 下的逻辑路径；C 编译器按物理当前目录记录
  # DW_AT_comp_dir，registry 或 git 为符号链接（如移动端共享下载缓存）时会得到链接目标。
  release_cargo_home_maps=("$release_cargo_home=/cargo-home")
  release_cargo_home_physical="$(cd "$release_cargo_home" && pwd -P)"
  if [[ "$release_cargo_home_physical" != "$release_cargo_home" ]]; then
    release_cargo_home_maps+=("$release_cargo_home_physical=/cargo-home")
  fi
  for release_cache in registry git; do
    if [[ -d "$release_cargo_home/$release_cache" ]]; then
      release_cache_physical="$(cd "$release_cargo_home/$release_cache" && pwd -P)"
      if [[ "$release_cache_physical" != "$release_cargo_home_physical/$release_cache" ]]; then
        release_cargo_home_maps+=("$release_cache_physical=/cargo-home/$release_cache")
      fi
    fi
  done
  release_rust_sysroot="$("${RUSTC:-rustc}" --print sysroot)"
  release_rust_commit="$("${RUSTC:-rustc}" -vV | sed -n 's/^commit-hash: //p')"
  if [[ -z "$release_rust_commit" ]]; then
    echo "Rust compiler commit hash is unavailable" >&2
    exit 1
  fi
  # 未装 rust-src 时标准库本来就记录为 /rustc/<commit>，重映射后两类工具链结果一致。
  release_remap_flags=(
    "--remap-path-prefix=$release_rust_sysroot/lib/rustlib/src/rust=/rustc/$release_rust_commit"
  )
  release_c_flags=""
  release_c_mappable=1
  RELEASE_PATH_REMAP_PREFIXES=("$REPO_ROOT/" "$TARGET_DIR/" "$release_rust_sysroot/")
  for release_map in "${release_cargo_home_maps[@]}"; do
    release_remap_flags+=("--remap-path-prefix=$release_map")
    release_c_flags="${release_c_flags:+$release_c_flags }-ffile-prefix-map=$release_map"
    if [[ "$release_map" =~ [[:space:]] ]]; then release_c_mappable=""; fi
    RELEASE_PATH_REMAP_PREFIXES+=("${release_map%=*}/")
  done
  if [[ -n "${CARGO_BUILD_BUILD_DIR:-}" ]]; then
    RELEASE_PATH_REMAP_PREFIXES+=("$CARGO_BUILD_BUILD_DIR/")
  fi

  # cc 会合并 CFLAGS、TARGET_CFLAGS 与目标专属 CFLAGS，追加到 CFLAGS 不影响其他来源。
  # cc 按空白拆分 CFLAGS，含空白的 CARGO_HOME 无法映射；若因此残留路径，由产物检查拦截。
  if [[ -n "$release_c_mappable" ]]; then
    RELEASE_PATH_REMAP_ENV=("CFLAGS=${CFLAGS:+$CFLAGS }$release_c_flags")
  fi

  # 环境变量 rustflags 会让 Cargo 忽略配置文件里的 rustflags，因此只能追加到环境变量；
  # 否则通过 --config 与仓库和用户配置中的 build.rustflags 合并，保留既有参数。
  if [[ -n "${CARGO_ENCODED_RUSTFLAGS+set}" || -n "${RUSTFLAGS+set}" ]]; then
    if [[ -n "${CARGO_ENCODED_RUSTFLAGS+set}" ]]; then
      release_encoded_flags="$CARGO_ENCODED_RUSTFLAGS"
    else
      read -r -a release_env_flags <<< "$RUSTFLAGS"
      release_encoded_flags="$(IFS=$'\x1f'; printf '%s' "${release_env_flags[*]:-}")"
    fi
    for flag in "${release_remap_flags[@]}"; do
      release_encoded_flags="${release_encoded_flags:+$release_encoded_flags$'\x1f'}$flag"
    done
    # env 的选项必须位于变量赋值之前。
    RELEASE_PATH_REMAP_ENV=(
      -u RUSTFLAGS ${RELEASE_PATH_REMAP_ENV[@]+"${RELEASE_PATH_REMAP_ENV[@]}"}
      "CARGO_ENCODED_RUSTFLAGS=$release_encoded_flags"
    )
  else
    release_toml_flags=""
    for flag in "${release_remap_flags[@]}"; do
      flag="${flag//\\/\\\\}"
      release_toml_flags="${release_toml_flags:+$release_toml_flags, }\"${flag//\"/\\\"}\""
    done
    RELEASE_PATH_REMAP_CARGO_ARGS=(--config "build.rustflags = [$release_toml_flags]")
  fi
fi

# 以环境和参数补齐发布构建的路径重映射；开发构建原样执行。
# 参数是完整的 cargo 命令，重映射参数追加在末尾。
with_release_path_remap() {
  # 空数组写法兼容 macOS 自带 bash 3.2 的 set -u。
  env ${RELEASE_PATH_REMAP_ENV[@]+"${RELEASE_PATH_REMAP_ENV[@]}"} "$@" \
    ${RELEASE_PATH_REMAP_CARGO_ARGS[@]+"${RELEASE_PATH_REMAP_CARGO_ARGS[@]}"}
}

# 发布库仍含构建机路径说明重映射被其他配置覆盖或出现新的路径来源，此时产物不可复现。
verify_release_paths() {
  local library="$1"
  local prefix
  for prefix in ${RELEASE_PATH_REMAP_PREFIXES[@]+"${RELEASE_PATH_REMAP_PREFIXES[@]}"}; do
    if LC_ALL=C grep -a -F -q -- "$prefix" "$library"; then
      echo "Release library still contains a build-machine path: $library" >&2
      exit 1
    fi
  done
}
