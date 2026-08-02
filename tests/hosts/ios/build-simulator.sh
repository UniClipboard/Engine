#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <simulator-udid>" >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PROJECT_DIR="$REPO_ROOT/tests/hosts/ios"
GENERATED_PROJECT="$REPO_ROOT/target/ios-probe-simulator-project/EngineProbe.xcodeproj"
DERIVED_DATA="$REPO_ROOT/target/ios-probe-simulator-derived"
SIMULATOR_ID="$1"

cd "$REPO_ROOT"
IPHONEOS_DEPLOYMENT_TARGET=17.0 \
  CARGO_TARGET_DIR="$REPO_ROOT/target/ios-probe-cargo" \
  cargo build -p uc-mobile-probe-core --release --target aarch64-apple-ios-sim --locked

mkdir -p "$(dirname "$GENERATED_PROJECT")"
GEM_HOME="/opt/homebrew/Cellar/cocoapods/1.16.2_2/libexec" \
  ruby "$PROJECT_DIR/project.rb" "$GENERATED_PROJECT" simulator

xcodebuild \
  -project "$GENERATED_PROJECT" \
  -scheme EngineProbe \
  -configuration Debug \
  -destination "id=$SIMULATOR_ID" \
  -derivedDataPath "$DERIVED_DATA" \
  CODE_SIGNING_ALLOWED=YES \
  CODE_SIGNING_REQUIRED=YES \
  CODE_SIGN_IDENTITY=- \
  build

echo "$DERIVED_DATA/Build/Products/Debug-iphonesimulator/EngineProbe.app"
