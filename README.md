# UniClipboardEngine

[![Codecov](https://codecov.io/gh/UniClipboard/Engine/graph/badge.svg)](https://codecov.io/gh/UniClipboard/Engine)

UniClipboardEngine 是 UniClipboard 在 macOS、Windows、Linux、iOS、Android 和 HarmonyOS 上共享的端到端加密 P2P 引擎。仓库统一维护设备身份、空间与配对、剪贴板同步、历史记录、加密搜索、文件传输、本地密文持久化、数据库迁移、跨平台绑定和发布产物。

它不是一个可以直接运行的终端应用。桌面程序和移动应用把它作为核心能力接入，并负责提供系统目录、安全存储、系统剪贴板、文件选择与生命周期通知。

> 当前仍处于 1.0 之前。外部 Rust 使用方只依赖 `uc-engine`，移动端只使用同一个 `v*` Release 中对应平台的产物。内部 crate 和绑定不发布到 crates.io。

## 核心能力

- 在所有平台使用同一套设备身份、空间、配对和信任关系。
- 默认通过端到端加密的 P2P 通道同步文字、图片和文件。
- 保存加密历史，支持分页、搜索、标签、收藏、资源读取、导出和重发。
- 处理前台、后台、暂停、恢复和关闭，保持设备身份不变。
- 通过宿主安全存储保管密钥，并默认加密所有持久化业务数据。
- 为 iOS、Android 提供 UniFFI 绑定，为 HarmonyOS 提供 N-API 绑定。
- 为所有正式产物生成来源、版本、校验值、依赖许可证和设备验收记录。

## 接入入口

| 使用方 | 接入内容 | 支持范围 |
| --- | --- | --- |
| Rust 桌面或服务宿主 | `uc-engine` | 唯一稳定的 Rust 入口 |
| iOS | `UniClipboardEngine.xcframework.zip` 和 `uc_engine_uniffi.swift` | iOS 16.4 及以上；真机 arm64；模拟器 arm64、x86_64 |
| Android | `UniClipboardEngine.aar` | API 24 及以上；arm64-v8a、x86_64；Java 17 |
| HarmonyOS | `UniClipboardEngine.har` | API 24 及以上；arm64-v8a |

LAN HTTP 同步位于 `compatibility/`，拥有独立版本和 `uc-mobile-v*` 发布线。它只能由用户明确选择，P2P 失败时不会自动切换到 LAN。

## 工作方式

```text
平台应用
  ├─ 私有目录、安全存储、系统剪贴板、文件句柄
  └─ 启动、前后台和退出通知
          │
          ▼
      uc-engine
  ├─ 空间、设备与配对
  ├─ 加密历史与搜索
  ├─ P2P 连接与传输
  └─ 数据库与后台任务
```

应用只向 Engine 提供平台能力，不自行复制加密、持久化、配对、传输或数据迁移规则。

## 安全边界

任何写入 SQLite、磁盘缓存或搜索索引的业务负载，默认都必须先经 MasterKey AEAD 加密，包括剪贴板正文、标题、预览、搜索字段、标签、文件名和文件路径。

只有以下内容可以例外：

- 内容类型分类，例如 `text`、`image`、`file`；
- 文件内容本体，可以按原始字节写入 Engine 管理的 blob store 或导入目录。

安全存储中的密钥材料由宿主平台保护。日志和公开错误不得包含剪贴板内容、密码、密钥、完整令牌、文件名或文件路径。详细规则见 [密文持久化规则](docs/security/encrypted-persistence.md) 和 [安全说明](SECURITY.md)。

## 仓库结构

| 路径 | 用途 |
| --- | --- |
| `crates/uc-engine/` | 唯一稳定入口，负责启动、操作、事件和生命周期 |
| `crates/uc-core/` | 领域模型和平台能力约定 |
| `crates/uc-application/` | 业务流程编排 |
| `crates/uc-infra/` | 数据库、加密、文件和 P2P 实现 |
| `bindings/uc-engine-uniffi/` | iOS 与 Android 绑定及打包脚本 |
| `bindings/uc-ohos-napi/` | HarmonyOS N-API 绑定与 ArkTS 声明 |
| `compatibility/` | 独立版本的 LAN 兼容线 |
| `tests/hosts/` | iOS、Android、HarmonyOS 验收宿主，不承载产品功能 |
| `docs/` | 架构、安全和迁移记录 |
| `scripts/architecture/` | 仓库归属、依赖方向和发布来源检查 |
| `scripts/release/` | 发布产物归集、清单生成和发布前核验 |

## 本地开发

### 环境要求

基础开发环境：

- Git；
- Rust 1.95.0。仓库的 `rust-toolchain.toml` 会自动选择版本；
- Node.js，用于运行仓库和发布检查脚本。

构建移动产物还需要：

- iOS：macOS、Xcode，以及 `aarch64-apple-ios`、`aarch64-apple-ios-sim`、`x86_64-apple-ios` Rust 目标；
- Android：Android SDK/NDK、JDK 17、`cargo-ndk`，以及 `aarch64-linux-android`、`x86_64-linux-android` Rust 目标；
- HarmonyOS：DevEco Studio 和 `aarch64-unknown-linux-ohos` Rust 目标。

安装 Rust 目标和 Android 构建工具：

```bash
rustup target add \
  aarch64-apple-ios \
  aarch64-apple-ios-sim \
  x86_64-apple-ios \
  aarch64-linux-android \
  x86_64-linux-android \
  aarch64-unknown-linux-ohos

cargo install cargo-ndk --locked
```

只开发 Rust 核心时不需要安装全部移动工具链。

### 基础检查

所有 Rust 命令都从仓库根目录运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

仓库检查会验证目录归属、依赖方向、唯一公开入口、绑定版本、产物来源、密文持久化规则和 LAN 隔离。

## 统一集成流程

无论接入哪个平台，都按以下顺序进行：

1. 选择一个完整的 `v*` GitHub Release，不使用可变分支上的临时产物。
2. 下载目标平台文件、`release-manifest.json`、`version.txt` 和 `source-commit.txt`。
3. 根据 `release-manifest.json` 校验每个文件的大小和 SHA-256。
4. 确认平台产物、生成绑定、版本文件和源码提交来自同一个 Release。
5. 实现平台宿主能力，启动 Engine，并立即开始消费事件。
6. 先尝试恢复已有会话；无法恢复时再进入创建空间或加入空间流程。
7. 把应用前后台切换和退出映射到 Engine 生命周期。
8. 在目标平台上完成创建或恢复空间、发送、接收、暂停、恢复和关闭验收。

### 校验发布文件

以下命令以 iOS 产物为例：

```bash
asset=UniClipboardEngine.xcframework.zip
expected="$(jq -r --arg name "$asset" \
  '.artifacts[] | select(.name == $name) | .sha256' \
  release-manifest.json)"
actual="$(shasum -a 256 "$asset" | awk '{print $1}')"
test -n "$expected" && test "$actual" = "$expected"
```

还应检查 `release-manifest.json` 中的版本、源码提交、最低系统版本和设备验收状态。`skipped` 只代表未执行，不能视为通过。

### 宿主必须提供的能力

| 能力 | 要求 |
| --- | --- |
| 私有数据目录 | 保存 Engine 管理的数据库、密文和身份；不得由其他应用访问 |
| 缓存目录 | 供 Engine 管理缓存；业务负载仍按敏感数据处理 |
| 临时目录 | 供短期导入和传输使用；不能自行保存明文业务数据 |
| 安全存储 | 以二进制形式读取、写入和删除密钥材料 |
| 系统剪贴板 | 读取和写入多种表示；平台允许时提供变化通知 |
| 文件句柄 | 把平台 URI 或文件描述符登记为不透明句柄，支持元数据和分块读写 |

文件句柄不能直接使用本机路径充当标识。发送或导出结束后，宿主应释放自己登记的句柄。

### 启动和生命周期

正常顺序如下：

```text
start -> recover/create/join -> operations
      -> suspend -> resume -> operations
      -> shutdown
```

- `start`：打开加密存储，恢复设备身份，启动 P2P 节点和后台任务。
- `quiesce`：停止接受新操作，等待已开始的操作结束；只在 Rust 入口公开。
- `suspend`：取消剩余工作并释放当前连接；引擎实例和事件通道保留。
- `resume`：使用原数据和设备身份重建连接；不会自动重试暂停前取消的操作。
- `shutdown`：停止全部工作并关闭事件通道。

移动应用进入后台时调用 `suspend`，回到前台时调用 `resume`。进程被系统结束后重新调用 `start`，不要尝试恢复旧的内存实例。

事件必须持续消费。收到 `RefreshRequired` 时重新查询当前状态，不要根据旧事件猜测状态。空目标设备列表表示发送给所有符合条件的可信设备；非空列表只缩小目标范围。

## Rust 集成

### 添加依赖

`uc-engine` 不发布到 crates.io。使用方应固定到已经验证的不可变提交：

```toml
[dependencies]
uc-engine = {
  git = "https://github.com/UniClipboard/core.git",
  rev = "<40-character-commit-sha>"
}
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

不要直接依赖 `uc-core`、`uc-application` 或 `uc-infra`。只有明确接入 LAN 兼容通道时才启用 `uc-engine/lan-compat`，正式产品不得启用 `dev-tools`。

### 最小启动流程

下面省略了四个平台适配器的具体实现，但展示了稳定入口的完整顺序：

```rust
use std::{path::PathBuf, time::Duration};
use uc_engine::{
    Engine, EngineConfig, EngineEvent, HostCapabilities, HostDirectories,
    Operation, RecoverSessionInput,
};

// MySecureStorage、MyClipboard 和 MyFileAccess 由宿主实现。
let host = HostCapabilities::new(
    HostDirectories::new(
        PathBuf::from("<private-data-directory>"),
        PathBuf::from("<cache-directory>"),
        PathBuf::from("<temporary-directory>"),
    ),
    Box::new(MySecureStorage::new()),
    Box::new(MyClipboard::new()),
    Box::new(MyFileAccess::new()),
);

let config = EngineConfig::new("<host-app-version>")
    .with_profile_id("default");
let (engine, mut events) = Engine::start(config, host).await?;

let event_task = tokio::spawn(async move {
    while let Some(event) = events.next().await {
        match event {
            EngineEvent::RefreshRequired { .. } => refresh_current_state().await,
            other => handle_engine_event(other).await,
        }
    }
});

let recovery = engine
    .execute(Operation::RecoverSession(RecoverSessionInput {
        allow_secure_storage_unlock: true,
    }))
    .await?;
handle_recovery(recovery).await?;

engine.shutdown(Duration::from_secs(2)).await?;
event_task.await?;
```

宿主适配器分别实现 `HostSecureStorage`、`HostClipboard` 和 `HostFileAccess`。所有业务动作都通过 `Engine::execute(Operation)` 发起，结果只使用 crate 根导出的稳定类型。完整操作和事件说明见 [uc-engine 跨平台核心接口](docs/specs/uc-engine-interface.md)。

## iOS 集成

### 需要的 Release 文件

- `UniClipboardEngine.xcframework.zip`；
- `uc_engine_uniffi.swift`；
- `UniClipboardEngine.xcframework.checksum.txt`；
- `release-manifest.json`、`version.txt`、`source-commit.txt`。

### 加入 Xcode 工程

1. 解压 `UniClipboardEngine.xcframework.zip`。
2. 把 `UniClipboardEngine.xcframework` 加到应用 target，作为静态库链接并选择 `Do Not Embed`。
3. 把 `uc_engine_uniffi.swift` 加到同一 target，并确认 Target Membership 已启用。
4. 将最低系统版本设为 iOS 16.4 或更高。
5. 链接 `Security` 和 `SystemConfiguration`。宿主若实现系统剪贴板和文件类型处理，还需要 `UIKit` 和 `UniformTypeIdentifiers`。
6. 不要同时编译另一个版本的 `uc_engine_uniffi.swift`。

也可以在应用仓库中用 CocoaPods 或 Swift Package 封装这两个文件，但封装必须固定同一个 Release，且构建前再次执行校验。

### 实现宿主并启动

生成代码提供 `BindingHost`。iOS 宿主通常使用 Application Support、Caches 和 Temporary 目录，使用 Keychain 保存安全数据，并把 `UIPasteboard`、文件选择器 URL 和安全作用域访问转换成绑定需要的形式。

```swift
let host = AppleEngineHost() // 应用实现 BindingHost
let engine = try MobileEngine.start(
  config: BindingConfig(
    appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "unknown",
    profileId: "default"
  ),
  host: host
)

let recovery = try engine.recoverSession(allowSecureStorageUnlock: true)
if !recovery.unlocked {
  // 展示创建空间或加入空间流程
}
```

应用进入后台时调用 `try engine.suspend()`，回到前台时调用 `try engine.resume()`，结束时调用 `try engine.shutdown(deadlineMs: 2_000)`。用独立任务轮询 `nextEvent(timeoutMs:)`，不要阻塞主线程。

主应用、分享扩展和键盘扩展不能共享同一个内存实例。扩展需要自己的短生命周期实例，并通过 App Group 目录和 Keychain Access Group 访问同一份受保护状态；同一时刻应只有一个进程持有 P2P 运行会话。

## Android 集成

### 需要的 Release 文件

- `UniClipboardEngine.aar`；
- `UniClipboardEngine.aar.checksum.txt`；
- `UniClipboardEngine.pom` 和 `runtime-dependencies.txt`；
- `release-manifest.json`、`version.txt`、`source-commit.txt`。

`uc_engine_uniffi.kt` 已编译进 AAR。Release 中的独立源码用于审计和自定义封装，不要再加入同一个应用 target，否则会产生重复类。

### 加入 Gradle 工程

把 AAR 放入应用的 `libs/`，然后添加：

```groovy
android {
  defaultConfig {
    minSdk 24
    ndk {
      abiFilters 'arm64-v8a', 'x86_64'
    }
  }

  compileOptions {
    sourceCompatibility JavaVersion.VERSION_17
    targetCompatibility JavaVersion.VERSION_17
  }
}

dependencies {
  implementation files('libs/UniClipboardEngine.aar')
  implementation 'net.java.dev.jna:jna:5.14.0@aar'
  implementation 'org.jetbrains.kotlin:kotlin-stdlib:2.1.20'
}
```

如果把 AAR 和随附 POM 放入内部 Maven 仓库，可以改用坐标 `app.uniclipboard:uniclipboard-engine:<version>`，让运行依赖由 POM 传递。

### 安装 Android 运行环境

当前原生入口要求在首次启动 Engine 前安装 application context。桥接类的包名、类名和方法名必须与下面保持一致：

```kotlin
package expo.modules.ucengine

import android.content.Context

class UcEngineModule {
  companion object {
    init {
      System.loadLibrary("uc_engine_uniffi")
    }

    @JvmStatic
    private external fun nativeInstallAndroidContext(context: Context): Boolean

    fun installAndroidContext(context: Context) {
      check(nativeInstallAndroidContext(context.applicationContext))
    }
  }
}
```

安装成功后，使用生成的 Kotlin 类型实现 `BindingHost`，再启动 Engine：

```kotlin
val engine = MobileEngine.start(
  BindingConfig(BuildConfig.VERSION_NAME, "default"),
  AndroidEngineHost(applicationContext)
)

val recovery = engine.recoverSession(allowSecureStorageUnlock = true)
if (!recovery.unlocked) {
  // 展示创建空间或加入空间流程
}
```

推荐使用 Android Keystore 保护安全存储值，使用应用私有目录和缓存目录，并把 `content://` URI 登记成不透明文件句柄。前后台切换分别调用 `suspend()` 和 `resume()`；服务结束时调用 `shutdown(2_000u)` 并关闭绑定对象。

Android 对后台运行和剪贴板访问有限制。应用应在有合法前台运行窗口时操作 Engine，不能把系统限制解释成切换到 LAN 的理由。

## HarmonyOS 集成

### 需要的 Release 文件

- `UniClipboardEngine.har`；
- `UniClipboardEngine.har.checksum.txt`；
- `release-manifest.json`、`version.txt`、`source-commit.txt`。

HAR 已包含 `arm64-v8a` 的 `libuc_ohos_napi.so` 和 ArkTS 声明。独立的 `libuc_ohos_napi.so`、`index.d.ts` 与验收 HAP 主要用于审计、调试和发布验收。

### 加入应用工程

把 HAR 放入模块的 `libs/`，在 `oh-package.json5` 中添加本地依赖：

```json5
{
  dependencies: {
    '@uniclipboard/engine': 'file:./libs/UniClipboardEngine.har',
  },
}
```

然后安装依赖并在 ArkTS 中导入：

```bash
ohpm install --all
```

```typescript
import engine from '@uniclipboard/engine'

const host = createEngineHost(context) // 应用实现 OhHost
const preparedHost = engine.prepareHost(host)
const active = await engine.startEngine(
  { appVersion: '1.0.0', profileId: 'default' },
  preparedHost
)

const recovery = await active.recoverSession(true)
if (!recovery.unlocked) {
  // 展示创建空间或加入空间流程
}
```

`OhHost` 需要提供三个目录、安全存储、剪贴板和文件句柄方法。由于 ArkTS 数值不能无损表达全部 64 位整数，文件偏移和大小按声明中的字符串传递。应用进入后台时调用 `suspend()`，回到前台时调用 `resume()`，退出时调用 `shutdown(2000)`。

可运行的完整宿主参考位于 `tests/hosts/ohos/entry/src/main/ets/host/`。

## 常用业务流程

### 首次启动

1. 启动 Engine。
2. 在安全存储可访问时调用 `RecoverSession` 或 `recoverSession(true)`。
3. 如果会话已恢复，查询本机设备和当前空间状态。
4. 如果没有空间，让用户选择创建空间或输入邀请加入空间。
5. 开始消费事件，并按需捕获或发送内容。

当应用处于扩展、设备锁定或其他不允许访问安全存储的环境时，应传入 `false`，不能强行读取系统安全存储。

### 配对新设备

已在空间中的设备签发一次邀请，新设备使用邀请码加入。邀请只能使用一次，也可以在被使用前取消。配对结果由操作返回；状态变化后应用重新查询 Engine 的完整设备关系，不应自行维护另一套成员状态。[规格 023](docs/specs/023-durable-membership-proof-and-admission-activation.md)定义了待实施的更严格边界：双方保存并确认同一成员历史和安全状态后才成功，等待流程由 Engine 自动恢复。

### 发送内容

- 文字和图片必须为 1 到 64 KiB；更大内容通过文件入口发送。
- 空目标列表表示发送给所有符合条件的可信设备。
- 文件发送先由宿主登记输入句柄，Engine 分块读取；完成后宿主释放句柄。
- 发送结果区分接受、重复、离线、失败和仍在等待，不能把等待中的设备提前记为失败。

### 处理剪贴板

宿主可以把系统剪贴板变化通知交给 Engine，也可以显式调用捕获操作。Engine 统一处理解锁检查、回写判断、去重、加密历史、搜索更新和发送，宿主不要重复这些步骤。

### 导出文件

宿主先让用户选择目标位置并登记可写句柄，再把条目编号和句柄传给 Engine。Engine 只看到句柄，不读取或持久化用户选择的目标路径；写完后调用宿主的完成写入方法。

## 生成移动产物

### iOS

```bash
UC_ENGINE_UNIFFI_BUILD_LOCKED=1 \
  bindings/uc-engine-uniffi/scripts/build-ios-xcframework.sh
```

默认输出位于 `target/uc-engine-uniffi-dist/ios/`。脚本默认产出同时含真机与模拟器切片的 XCFramework；设置 `UC_ENGINE_UNIFFI_SLICE=device` 或 `UC_ENGINE_UNIFFI_SLICE=simulator` 时只构建对应切片。

### Android

```bash
UC_ENGINE_UNIFFI_BUILD_LOCKED=1 \
  bindings/uc-engine-uniffi/scripts/build-android-aar.sh
```

默认输出位于 `target/uc-engine-uniffi-dist/android/`。

### 本地产物

```bash
bindings/uc-engine-uniffi/scripts/prepare-local-artifacts.mjs
```

按设备与模拟器拆分构建 iOS XCFramework，并构建 Android AAR，归集到 `.artifacts/local/{ios,ios-sim,android}/`，同时生成就绪清单 `.artifacts/local/local-prepared.json`（平台、版本、提交与主产物 sha256）。该目录不纳入版本控制；移动端本地开发可按运行目标切换使用。

### HarmonyOS

```bash
tests/hosts/ohos/build-emulator.sh
```

脚本默认从 `/Applications/DevEco-Studio.app/Contents` 查找 DevEco Studio。自定义安装位置时设置 `DEVECO_STUDIO_CONTENTS`。默认发布目录位于临时构建目录，可通过 `UC_OHOS_DIST_DIR` 指定。

## 平台验收宿主

验收宿主只用于证明绑定和平台能力能一起工作，不应被复制成产品业务层。

### iOS 真机

```bash
tests/hosts/ios/build-device.sh <device-id>
```

需要可用的 Xcode 签名身份和真机。未运行真机流程时必须记录为“跳过”。

### Android 模拟器

```bash
tests/hosts/android/install-emulator.sh
tests/hosts/android/probe-command.sh '<json-command>'
```

需要正在运行且可通过 `adb` 访问的 arm64 模拟器。

### HarmonyOS 模拟器

```bash
tests/hosts/ohos/build-emulator.sh
```

该命令同时生成 HAR 和已签名验收 HAP。签名脚本还需要 `jq`。

## 发布

正式 Engine 使用 `v*` 标签，由 `.github/workflows/release-engine.yml` 从同一个提交构建三端产物。LAN 兼容线使用独立的 `uc-mobile-v*` 标签和 `.github/workflows/release-lan-compat.yml`。

正式 Release 包含：

| 类别 | 主要文件 |
| --- | --- |
| iOS | XCFramework、Swift 绑定、校验值 |
| Android | AAR、Kotlin 绑定、POM、运行依赖、校验值 |
| HarmonyOS | HAR、动态库、ArkTS 声明、验收 HAP、校验值 |
| 通用 | 源码归档、`Cargo.lock`、许可证、依赖许可证、调试资料 |
| 追溯 | `version.txt`、`source-commit.txt`、`release-manifest.json` |

发布包归集后运行：

```bash
node scripts/release/verify-release-bundle.mjs <release-assets-directory>
```

核验通过前不得发布。已经上传的资产不可覆盖；发现问题时发布新版本，并保留旧版本的可追溯记录。详细规则见 [发布完整性](docs/security/release-integrity.md)。

## 升级指南

1. 选择新的完整 `v*` Release，并阅读变更说明。
2. 同时更新原生库、生成绑定、版本和源码提交记录。
3. 重新核对 `release-manifest.json`，不要沿用旧版本的校验值。
4. 检查数据库迁移、最低系统版本和设备验收状态。
5. 分别测试已有空间恢复、设备身份保持、发送与接收、文件导出、暂停与恢复。
6. iOS 扩展和 Android 后台服务也要使用同一版本，不能只升级主应用。

不要长期保留新旧两套 Engine，也不要在产品仓维护核心补丁副本。核心问题应在本仓修复并通过新 Release 交付。

## 进一步阅读

- [UniClipboard 架构圣经](docs/architecture/architecture-bible.md)
- [文档索引](docs/README.md)
- [项目愿景](VISION.md)
- [Port 定义](docs/specs/ports.md)
- [uc-engine 跨平台核心接口](docs/specs/uc-engine-interface.md)
- [密文持久化规则](docs/security/encrypted-persistence.md)
- [发布完整性](docs/security/release-integrity.md)
- [迁移历史映射](docs/migration/source-history-map.md)
- [贡献和维护规则](AGENTS.md)

## 许可证

本项目使用 [Apache License 2.0](LICENSE)。
