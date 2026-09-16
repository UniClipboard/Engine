#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
REPO_ROOT="$(cd "${1:-$REPO_ROOT}" && pwd)"
TARGET_DIR="${UC_ENGINE_UNIFFI_TARGET_DIR:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}}"
DIST_ROOT="${UC_ENGINE_UNIFFI_DIST_DIR:-$TARGET_DIR/uc-engine-uniffi-dist}"
DIST_DIR="$DIST_ROOT/ios"
STAGE_DIR="$TARGET_DIR/uc-engine-uniffi-ios-package"
BINDINGS_DIR="$STAGE_DIR/bindings"
INCLUDE_DIR="$BINDINGS_DIR/include"
DEVICE_DIR="$STAGE_DIR/device"
SIMULATOR_ARM64_DIR="$STAGE_DIR/simulator-arm64"
SIMULATOR_X86_64_DIR="$STAGE_DIR/simulator-x86_64"
SIMULATOR_DIR="$STAGE_DIR/simulator"
XCFRAMEWORK="$DIST_DIR/UniClipboardEngine.xcframework"
XCFRAMEWORK_ZIP="$DIST_DIR/UniClipboardEngine.xcframework.zip"
CHECKSUM_FILE="$DIST_DIR/UniClipboardEngine.checksum.txt"
DEBUG_DIR="$DIST_ROOT/debug-symbols/ios"
BINDINGS_CACHE_ROOT="${UC_ENGINE_UNIFFI_BINDINGS_CACHE_DIR:-}"
CARGO_LOCKED_FLAG=""
BUILD_PROFILE="${UC_ENGINE_UNIFFI_BUILD_PROFILE:-release}"
case "$BUILD_PROFILE" in
  dev) PROFILE_DIR=debug ;;
  release) PROFILE_DIR=release ;;
  *) echo "UC_ENGINE_UNIFFI_BUILD_PROFILE must be dev or release" >&2; exit 1 ;;
esac
SLICE="${UC_ENGINE_UNIFFI_SLICE:-universal}"
case "$SLICE" in
  device|simulator|universal) ;;
  *) echo "UC_ENGINE_UNIFFI_SLICE must be device, simulator, or universal" >&2; exit 1 ;;
esac

selective_strip_archive() {
  # 本机调试保留符号，也不做发布包的归档重写。
  if [[ "$BUILD_PROFILE" == "dev" ]]; then return; fi
  local archive="$1"
  local work_dir
  local members
  local duplicates
  local object
  local load_commands
  local rebuilt
  local strip_objects=()

  work_dir="$(mktemp -d "${TMPDIR:-/tmp}/uc-engine-uniffi-strip.XXXXXX")"
  members="$work_dir/members.txt"
  rebuilt="$work_dir/rebuilt.a"
  xcrun ar -t "$archive" > "$members"
  duplicates="$(sort "$members" | uniq -d)"
  if [[ -n "$duplicates" ]]; then
    echo "Cannot safely repack archive with duplicate member names: $archive" >&2
    rm -rf "$work_dir"
    return 1
  fi

  (
    cd "$work_dir"
    xcrun ar -x "$archive"
    for object in ./*.o; do
      load_commands="$(otool -l "$object")"
      if [[ "$load_commands" != *"sectname __eh_frame"* ]]; then
        strip_objects+=("$object")
      fi
    done
    xcrun strip -S "${strip_objects[@]}"
    xcrun ar rcs "$rebuilt" ./*.o
  )
  mv "$rebuilt" "$archive"
  rm -rf "$work_dir"
}

if [[ -n "${UC_ENGINE_UNIFFI_BUILD_LOCKED:-}" ]]; then
  CARGO_LOCKED_FLAG="--locked"
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "iOS packaging requires macOS and Xcode" >&2
  exit 1
fi

export CARGO_TARGET_DIR="$TARGET_DIR"
export CARGO_PROFILE_RELEASE_DEBUG="${CARGO_PROFILE_RELEASE_DEBUG:-0}"
export IPHONEOS_DEPLOYMENT_TARGET="${UC_ENGINE_UNIFFI_IOS_DEPLOYMENT_TARGET:-16.4}"
cd "$REPO_ROOT"
rm -rf "$STAGE_DIR" "$DIST_DIR" "$DEBUG_DIR"
mkdir -p \
  "$INCLUDE_DIR" \
  "$DEVICE_DIR" \
  "$SIMULATOR_ARM64_DIR" \
  "$SIMULATOR_X86_64_DIR" \
  "$SIMULATOR_DIR" \
  "$DIST_DIR" \
  "$DEBUG_DIR"

binding_inputs_sha256() {
  {
    printf '%s\n' 'uc-engine-uniffi-swift-bindings-v1'
    for file in Cargo.toml Cargo.lock rust-toolchain.toml bindings/uc-engine-uniffi/Cargo.toml; do
      printf 'file:%s\n' "$file"
      shasum -a 256 "$file"
    done
    find bindings/uc-engine-uniffi/src -type f -print | LC_ALL=C sort | while IFS= read -r file; do
      printf 'file:%s\n' "$file"
      shasum -a 256 "$file"
    done
  } | shasum -a 256 | awk '{print $1}'
}

BINDINGS_INPUT_SHA256="$(binding_inputs_sha256)"
BINDINGS_CACHE_ENTRY=""
if [[ -n "$BINDINGS_CACHE_ROOT" ]]; then
  BINDINGS_CACHE_ENTRY="$BINDINGS_CACHE_ROOT/$BINDINGS_INPUT_SHA256"
fi

if [[ -n "$BINDINGS_CACHE_ENTRY" &&
      -f "$BINDINGS_CACHE_ENTRY/complete" &&
      -f "$BINDINGS_CACHE_ENTRY/uc_engine_uniffi.swift" &&
      -f "$BINDINGS_CACHE_ENTRY/uc_engine_uniffiFFI.h" &&
      -f "$BINDINGS_CACHE_ENTRY/uc_engine_uniffiFFI.modulemap" ]]; then
  echo "==> Reuse Swift bindings ($BINDINGS_INPUT_SHA256)"
  cp "$BINDINGS_CACHE_ENTRY/uc_engine_uniffi.swift" "$BINDINGS_DIR/"
  cp "$BINDINGS_CACHE_ENTRY/uc_engine_uniffiFFI.h" "$BINDINGS_DIR/"
  cp "$BINDINGS_CACHE_ENTRY/uc_engine_uniffiFFI.modulemap" "$BINDINGS_DIR/"
else
  echo "==> Generate Swift bindings from the host library"
  # 宿主库只用于读取接口元数据，不进入发布包；公开接口输入不变时复用已验证生成物。
  cargo build -p uc-engine-uniffi --profile dev --features bindgen-cli \
    --lib --bin uc-engine-uniffi-bindgen $CARGO_LOCKED_FLAG
  cargo run -p uc-engine-uniffi --profile dev --features bindgen-cli \
    --bin uc-engine-uniffi-bindgen $CARGO_LOCKED_FLAG -- \
    generate --library "$TARGET_DIR/debug/libuc_engine_uniffi.dylib" \
    --language swift --out-dir "$BINDINGS_DIR"
  if [[ -n "$BINDINGS_CACHE_ENTRY" ]]; then
    mkdir -p "$BINDINGS_CACHE_ROOT"
    pending_cache="$(mktemp -d "$BINDINGS_CACHE_ROOT/.bindings.XXXXXX")"
    cp "$BINDINGS_DIR/uc_engine_uniffi.swift" "$pending_cache/"
    cp "$BINDINGS_DIR/uc_engine_uniffiFFI.h" "$pending_cache/"
    cp "$BINDINGS_DIR/uc_engine_uniffiFFI.modulemap" "$pending_cache/"
    printf '%s\n' "$BINDINGS_INPUT_SHA256" > "$pending_cache/complete"
    if [[ ! -e "$BINDINGS_CACHE_ENTRY" ]]; then
      mv "$pending_cache" "$BINDINGS_CACHE_ENTRY"
    else
      rm -rf "$pending_cache"
    fi
  fi
fi
cp "$BINDINGS_DIR/uc_engine_uniffiFFI.h" "$INCLUDE_DIR/"
cp "$BINDINGS_DIR/uc_engine_uniffiFFI.modulemap" "$INCLUDE_DIR/module.modulemap"

if [[ "$SLICE" != "simulator" ]]; then
  echo "==> Build iOS device library"
  cargo build -p uc-engine-uniffi --profile "$BUILD_PROFILE" --target aarch64-apple-ios $CARGO_LOCKED_FLAG
  cp "$TARGET_DIR/aarch64-apple-ios/$PROFILE_DIR/libuc_engine_uniffi.a" "$DEVICE_DIR/"
  cp "$DEVICE_DIR/libuc_engine_uniffi.a" "$DEBUG_DIR/device.a"
  selective_strip_archive "$DEVICE_DIR/libuc_engine_uniffi.a"
fi

if [[ "$SLICE" != "device" ]]; then
  echo "==> Build iOS simulator libraries"
  cargo build -p uc-engine-uniffi --profile "$BUILD_PROFILE" --target aarch64-apple-ios-sim $CARGO_LOCKED_FLAG
  cargo build -p uc-engine-uniffi --profile "$BUILD_PROFILE" --target x86_64-apple-ios $CARGO_LOCKED_FLAG
  cp "$TARGET_DIR/aarch64-apple-ios-sim/$PROFILE_DIR/libuc_engine_uniffi.a" \
    "$SIMULATOR_ARM64_DIR/"
  cp "$TARGET_DIR/x86_64-apple-ios/$PROFILE_DIR/libuc_engine_uniffi.a" \
    "$SIMULATOR_X86_64_DIR/"
  cp "$SIMULATOR_ARM64_DIR/libuc_engine_uniffi.a" "$DEBUG_DIR/simulator-arm64.a"
  cp "$SIMULATOR_X86_64_DIR/libuc_engine_uniffi.a" "$DEBUG_DIR/simulator-x86_64.a"
  selective_strip_archive "$SIMULATOR_ARM64_DIR/libuc_engine_uniffi.a"
  selective_strip_archive "$SIMULATOR_X86_64_DIR/libuc_engine_uniffi.a"
  lipo -create \
    "$SIMULATOR_ARM64_DIR/libuc_engine_uniffi.a" \
    "$SIMULATOR_X86_64_DIR/libuc_engine_uniffi.a" \
    -output "$SIMULATOR_DIR/libuc_engine_uniffi.a"
fi

echo "==> Create XCFramework"
XCFRAMEWORK_ARGS=()
if [[ "$SLICE" != "simulator" ]]; then
  XCFRAMEWORK_ARGS+=(-library "$DEVICE_DIR/libuc_engine_uniffi.a" -headers "$INCLUDE_DIR")
fi
if [[ "$SLICE" != "device" ]]; then
  XCFRAMEWORK_ARGS+=(-library "$SIMULATOR_DIR/libuc_engine_uniffi.a" -headers "$INCLUDE_DIR")
fi
xcodebuild -create-xcframework "${XCFRAMEWORK_ARGS[@]}" -output "$XCFRAMEWORK"

cp "$BINDINGS_DIR/uc_engine_uniffi.swift" "$DIST_DIR/"
ditto -c -k --keepParent "$XCFRAMEWORK" "$XCFRAMEWORK_ZIP"
shasum -a 256 "$XCFRAMEWORK_ZIP" | awk '{print $1}' > "$CHECKSUM_FILE"

VERSION="$(cargo pkgid -p uc-engine-uniffi)"
VERSION="${VERSION##*#}"
COMMIT="$(git rev-parse HEAD)"
printf 'v%s\n' "$VERSION" > "$DIST_DIR/version.txt"
printf '%s\n' "$COMMIT" > "$DIST_DIR/source-commit.txt"
printf '%s\n' "$BUILD_PROFILE" > "$DIST_DIR/build-profile.txt"

echo "OK: $XCFRAMEWORK"
echo "OK: $XCFRAMEWORK_ZIP"
echo "OK: $CHECKSUM_FILE"
