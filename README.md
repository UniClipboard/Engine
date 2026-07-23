# UniClipboardCore

UniClipboardCore 是 UniClipboard 的共享核心仓库。这里维护六个平台共同使用的身份、配对、端到端加密、P2P 同步、持久化、数据库迁移和移动绑定。

外部 Rust 使用方只通过 `uc-engine` 接入。内部 crate 不单独发布，也不承诺独立稳定性。

## 仓库结构

| 目录 | 用途 |
| --- | --- |
| `crates/` | 核心实现和唯一稳定入口 `uc-engine` |
| `bindings/` | iOS、Android 和 HarmonyOS 绑定 |
| `compatibility/` | 独立维护的 LAN 兼容线 |
| `tests/hosts/` | 三种移动平台的验收宿主 |
| `docs/` | 架构、安全和迁移记录 |
| `scripts/` | 仓库检查、安全扫描和发布归集 |

## 本地检查

从仓库根目录运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-core-repository.mjs
git diff --check
```

仓库检查会验证目录归属、依赖方向、唯一公开入口、绑定版本、产物来源、密文持久化规则和 LAN 隔离，并用三个错误样例确认检查本身不会失效。

## 生成移动产物

iOS 和 Android：

```bash
UC_ENGINE_UNIFFI_BUILD_LOCKED=1 \
  bindings/uc-engine-uniffi/scripts/build-ios-xcframework.sh

UC_ENGINE_UNIFFI_BUILD_LOCKED=1 \
  bindings/uc-engine-uniffi/scripts/build-android-aar.sh
```

HarmonyOS：

```bash
tests/hosts/ohos/build-emulator.sh
```

完整核心发布由 `.github/workflows/release-core.yml` 从同一提交生成并归集三端产物。LAN 兼容线使用独立的 `.github/workflows/release-lan-compat.yml`。

## 使用方式

迁移阶段的 Rust 使用方固定不可变提交：

```toml
uc-engine = {
  git = "https://github.com/UniClipboard/core.git",
  rev = "<immutable-commit-sha>"
}
```

移动使用方固定完整的 `core-v*` Release，并同时采用对应平台的本地库、生成绑定和校验信息，不允许混用不同版本的文件。

## 进一步阅读

- [架构原则](docs/architecture/principles.md)
- [稳定入口](docs/architecture/uc-engine-interface.md)
- [持久化安全](docs/security/encrypted-persistence.md)
- [发布完整性](docs/security/release-integrity.md)
- [迁移历史映射](docs/migration/source-history-map.md)
