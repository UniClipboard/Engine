# Go binding

Go module `github.com/UniClipboard/Engine/bindings/go`：让 Go 宿主（例如后续的 Go daemon）在同一进程内托管真实的 Rust Engine。
Engine、Iroh、加密与持久化仍由 Rust 负责；本模块只提供生成的 UniFFI 绑定、一个薄 Go 门面和原生库身份校验。
设计取舍、失败模型与验收契约见[执行计划](../../docs/exec-plans/active/2026-10-07-go-binding.md)与[ADR-034](../../docs/design-docs/decisions/034-go-binding-via-generated-uniffi.md)。

## 目录

| 路径 | 内容 | 维护方式 |
|---|---|---|
| `engine/` | 门面：`Open`、查询、事件、`Close`、稳定错误、输入校验 | 手写 |
| `native/` | 原生库来源清单校验（`Verify`）、构建与整理脚本 | 手写 |
| `uc_engine_uniffi/` | 生成的 UniFFI 绑定（`.go` 与 `.h`）；`link.go` 为手写 cgo 指令 | 生成，禁止手改 |
| `generator/` | 固定的生成器来源、模板补丁、构建与生成脚本 | 手写 |

## 使用方式

这不是纯 Go 包：需要 `CGO_ENABLED=1`、C 编译器，以及与源码提交匹配的原生库。`go get` 不会下载或构建原生库。

```bash
# 1. 在 Engine 仓库根目录构建并整理原生库（macOS arm64 已实测）
bash bindings/go/native/build-native.sh release
bash bindings/go/native/stage-native.sh release <交付目录>

# 2. 在宿主构建环境提供库路径与 rpath，然后正常 go build
export CGO_ENABLED=1
export CGO_LDFLAGS="-L<交付目录> -Wl,-rpath,<交付目录>"
```

交付目录包含 `libuc_engine_uniffi.<dylib|so>` 与 `native-manifest.json`。宿主应用打包时必须把库放在可执行文件能解析的位置
（`@rpath`/`$ORIGIN`），不要依赖开发者 shell 环境。macOS 上 `stage-native.sh` 把 install_name 改为 `@rpath/…` 并重新 ad-hoc 签名；
面向用户分发仍须宿主自己的 Developer ID 签名与公证。

```go
import "github.com/UniClipboard/Engine/bindings/go/engine"

eng, err := engine.Open(engine.Config{
    AppVersion:     "1.1.0-rc.22",
    ProfileID:      "default",
    NativeManifest: "/path/to/native-manifest.json", // 必填；校验失败即拒绝启动
}, myHost)
if err != nil { /* engine.EngineError / ErrHost* / native.ErrMismatch … */ }
defer eng.Close(10 * time.Second)

device, err := eng.LocalDevice(ctx)
for { event, err := eng.NextEvent(ctx); … }
```

## 契约摘要

- **来源校验**：`Open` 读取清单，核对 `core_version`、动态链接器实际映射的库文件名与 sha256/size。清单缺失、字段不符或库被替换返回 `native.ErrMismatch`；
  没有跳过校验的模式。Windows 暂无加载路径解析，返回 `native.ErrUnsupported`。
- **关闭**：`Close(deadline)` 先拒绝新调用，再请求 Rust 在期限内关闭并 join，排空在途调用后释放对象。期限内未完成返回 `ErrCloseIncomplete`，可重试。
  `Close` 后调用返回 `ErrClosed`；与 `Close` 竞争的在途调用可能得到 Engine 的 `InvalidState` 稳定错误。
- **取消**：`context` 取消只让调用方停止等待，Rust 调用继续，`Close` 仍等待其结束。
- **事件**：Rust 队列容量 256，溢出时丢最旧事件并给出 `RefreshRequired(ConsumerLagged)`，收到后重新查询。门面不再缓冲。未映射的事件只报告种类，不携带载荷。
- **输入**：零值或越界枚举、空配置在进入 Rust 之前返回 `ErrInvalidInput`。
- **宿主回调**：只能返回能力结果，不得在回调内调用同一个 Engine（同步请求会等待正在执行回调的线程）。目录回调失败原样映射为 `ErrHost*`；启动期间安全存储失败由 Engine 归类为稳定的启动错误 `1101`。
- **崩溃隔离**：发布构建 `panic=abort`，Rust 崩溃会终止整个宿主进程，Go 的 `recover` 无效。嵌入后宿主与 Engine 同生共死，Engine 不再有进程隔离；
  GUI/CLI 若仍是独立进程经 HTTP 连接 daemon，则它们不受 daemon 崩溃影响。这是 daemon 重写评审时需要明确讨论的取舍。
- **敏感信息**：错误与事件不含内容、路径或设备信息；`Invitation` 的 `String` 固定脱敏。

## 重新生成

```bash
bash bindings/go/generator/build-generator.sh <外置空目录>     # 固定 revision + 模板补丁
bash bindings/go/generator/generate.sh <生成器>/bin/uniffi-bindgen-go target/debug/libuc_engine_uniffi.dylib
git diff --exit-code bindings/go/uc_engine_uniffi
```

生成物提交入库，CI 重新生成并要求零差异。模板补丁把模板引入的局部变量统一加 `_uniffi` 前缀，避免与用户方法参数同名（上游原样生成器因 `handle` 撞名无法编译）。
生成器与模板为 MPL-2.0，许可证文本随生成包保留为 `uc_engine_uniffi/LICENSE-uniffi-bindgen-go`；本仓其余部分为 Apache-2.0。

## 平台状态

| 目标 | 状态 |
|---|---|
| macOS arm64（本机） | 构建、链接、真实进程验收已实测，见执行计划证据 |
| Linux amd64/arm64 | CI 做原生构建与 Go 编译/链接检查；真实进程验收未覆盖 |
| macOS amd64、Windows amd64/arm64 | 未验证；Windows 需要与 Go C 链接器匹配的导入库，且尚无库身份校验 |

交叉编译不记为原生验证。

## 能力范围

门面当前覆盖：启动/关闭、本机设备、空间状态、设备列表、邀请签发、退出空间、网络时机通知、生命周期（暂停/恢复/状态）与事件。
现有 `MobileEngine` 是 `uc-engine` Operation 的子集；历史、设置、搜索、诊断、解锁与恢复、升级、LAN 兼容等 Desktop 需要的能力
尚未暴露，也未声称覆盖全部 62 个 Desktop 宿主命令。剪贴板与文件句柄宿主能力固定返回“不可用”。后续新增必须经 `uc-engine` Operation 加性补入统一绑定，并核对 Swift/Kotlin。
