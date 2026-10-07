# Go binding 基础

状态：失败模型与验收契约已先行；实现按本文顺序分片。关联研究：t-0196。基线：main `e86f94cebcec46c1b3a6f49f88cce7a833777640`。

## 目标与边界

Engine 仓库拥有一个可重复生成、可 `import` 的版本化 Go module（`bindings/go`），让后续 Go daemon 在同一进程内托管真实 Rust Engine。Rust Engine、Iroh、加密、持久化和业务流程的负责人不变；Go 只做宿主：一次 Start、调用 `uc-engine` 既有完整动作、读取事件、确定性 Close。

不在本片：移除或改写现有 Rust daemon、改变产品退出语义、修改移动 pin 或移动公开 API、直接授权 daemon。Go daemon 的常驻、GUI 退出存活、CLI 复用、单实例、HTTP/WS 与平台服务生命周期仍是后续计划；本片只提供其所需的 Engine 生命周期事件与能力映射（见“能力映射与未完成范围”）。

完整负责人：`bindings/uc-engine-uniffi`（既有薄绑定）拥有 start、命令分发、事件队列与 shutdown/join；`bindings/go/engine` 只负责 Go 侧所有权（调用与 Close 的互斥、输入校验、错误归一）。Go 不拼接底层业务流程，不依赖 Application 内部。

## 工具链选择

- UniFFI 固定 `=0.31.1`（既有）。Go 生成器采用 NordSecurity `uniffi-bindgen-go` 固定 revision `0b7fb4ceef12021bd7f790cc516fa9133e001813`（tag `v0.7.1+v0.31.0`），生成器自身 `Cargo.lock` 固定，不加入本仓 workspace，不改本仓 `Cargo.lock`。
- 第一片不修改 Rust 源码，使被测 native 库可直接追溯到 main 提交。
- 生成物提交入库（`go get` 不能运行生成器）。CI 重新生成并要求零差异。
- 模板补丁：研究只改了 callback 的 `handle`。本片对所有由模板引入、且与用户方法参数名同一作用域的局部变量统一加保留前缀 `_uniffi`，并用完整组件编译覆盖；补丁文件与其 sha256 记入来源清单。MPL-2.0 许可证头与生成器 LICENSE 随生成物保留。

## 失败模型（实现前）

| # | 失败 | 契约 |
|---|---|---|
| F1 | 上游原样生成器因局部变量撞名无法编译 | 原失败证据保留；补丁只通过生成器源码补丁交付，禁止手改生成后的 Go 文件；重新生成必须字节一致 |
| F2 | 补丁只修已知撞名，其他模板局部变量仍可能与用户参数同名 | 审计 VTableImpl/callback/async 模板的全部局部名；验收为完整组件编译，不称通用卫生修复 |
| F3 | 生成物与 native 库错配（版本、UniFFI contract、API checksum） | 生成包 init 的 checksum 检查必须失败即 panic；facade 另在 Open 时核对 `core_version` 与来源清单 |
| F4 | 加载了非预期的动态库（路径、替换、旧库） | facade 通过 `dladdr` 取实际加载路径，对其计算 sha256 并与来源清单比对；不一致返回 `ErrNativeMismatch`。Windows 暂不支持该校验，明确返回不支持而非静默通过 |
| F5 | 来源清单缺失或字段不符 | 校验失败即拒绝启动，不降级为未验证模式；清单含 Engine revision、`Cargo.lock` sha256、目标、profile、toolchain、features、生成器 revision 与补丁 hash、库 sha256/size |
| F6 | Go 零值枚举/必填字段转 Rust 时 panic | facade 在越界前校验所有输入枚举与必填字段，返回 `ErrInvalidInput`；不得依赖生成层 panic |
| F7 | Rust release `panic=abort`、段错误或 OOM | Go `recover` 无效，进程直接退出；契约明确声明。嵌入后宿主与 Engine 同生共死，不再有进程隔离。崩溃隔离作为 daemon 重写评审时的显式取舍，不默认嵌入更优。GUI 仍可保持为独立进程经 HTTP 连接 daemon |
| F8 | Close 与进行中的调用并发 | Close 先标记关闭，不持锁调用 `shutdown(deadline)`（这会关闭事件队列并唤醒 `next_event`），再取写锁等待在途调用排空，最后 Destroy。重复 Close 幂等；Close 后调用返回 `ErrClosed`，不 panic |
| F9 | Go `context` 取消被误当作 Rust 取消 | 只让调用方停止等待；Rust 调用继续，Close 仍等待被放弃的调用。文档与测试明确 |
| F10 | `Destroy` 被误当作 shutdown | `Destroy` 只释放引用。唯一关闭边界是 `shutdown(deadline)` 成功返回与 worker join；deadline 超时返回错误且不声称已停止，可重试 |
| F11 | 事件消费者落后 | Rust 侧容量 256，丢最旧并给出 `RefreshRequired(ConsumerLagged)`；Go 不再设第二层丢弃缓冲。消费者收到后重新查询权威状态 |
| F12 | 宿主回调重入同一 Engine 的阻塞 API 造成环形等待 | 回调只能返回能力结果；facade 文档禁止在回调内调用同一实例。E2E 用带超时的真实进程断言重入被拒绝或有界失败，不允许无限挂起 |
| F13 | 回调返回的类型化宿主错误丢失 | 往返断言 `PermissionDenied`、`Unavailable` 映射到 `BindingError` 且 `errors.Is` 成立 |
| F14 | 重启后加密 profile 不可读 | 安全存储以临时文件持久化，第二个进程用同一 profile 重启并断言同一本地设备身份 |
| F15 | 测试触碰真实 profile、钥匙串、剪贴板或网络 | 全部使用临时目录和内存/文件 host；剪贴板与文件回调返回 Unavailable；E2E 在 macOS `sandbox-exec` 的拒绝网络配置下运行，网络不可达本身即证据 |
| F16 | 日志泄露内容或秘密 | 观测只写本地文件到临时目录；E2E 在日志与 stdout/stderr 中扫描哨兵字符串，必须零命中 |
| F17 | 并发调用过载 | Engine 命令队列无界；本片不引入限流，文档声明并发调用语义，调用方负责背压 |
| F18 | 跨目标构建被当作原生验证 | 仅 macOS arm64 做本机链接与真实进程；其他目标只记录实际执行的生成/编译/link 检查，交叉编译不记通过 |
| F19 | 公共 UniFFI 表面变化破坏 Swift/Kotlin | 第一片不改 Rust 表面。任何后续新增必须加性，并运行 `bindings/uc-engine-uniffi` 契约测试 |
| F20 | 生成物许可证头丢失 | 生成包保留模板 MPL header，并随附生成器 LICENSE；检查脚本断言存在 |

## 验收契约

1. 来源：`just cargo build` 的 native 库对应不可变 main 提交与干净工作区；清单记录上表 F5 全部字段。重新冻结干净来源后重建并重复关键链。
2. 生成：固定生成器 + 补丁 → 完整 Go package；二次生成与入库内容字节一致；Go 1.24.0 与 Go 1.27.1 均编译通过。
3. 真实进程 E2E（独立临时 profile/密钥/内存或文件 host，网络隔离）依序：
   start → query（本地设备、空间状态）→ 类型化 Engine 错误 → 成功的 host 回调 → 事件（suspend/resume 触发 `StateChanged`）→ `shutdown(deadline)` 与 join → 同一 profile 重启并断言身份一致 → 最终退出码 0。
4. 负向：释放后调用、零值枚举、重复 Close、并发调用与 Close、Close 中 `next_event`、校验和错配（篡改库）、清单缺失，各自得到明确 Go 错误而非挂起或 panic。
5. 证据：可验证 JSON、原始日志、命令、provenance 保存在本任务 library 目录；E2E 无产品单元测试补写，失败也要保留。
6. 门禁：`node scripts/architecture/check-engine-repository.mjs`、`check-rust-style.mjs`、`cargo fmt --check`、`git diff --check` 通过；新增检查纳入 PR Check。

## 能力映射与未完成范围

既有 `MobileEngine` 是 `uc-engine` Operation 的子集。第一片只暴露 start、`query_local_device`、`query_space_state`、`list_devices`、生命周期（suspend/resume/lifecycle_state/shutdown）与 `next_event`。其余 Desktop 需要的历史、设置、搜索、诊断、解锁与恢复、升级、LAN 兼容等能力，只能经 `uc-engine` 的 Operation 加性补入统一绑定，并同时核对 Swift/Kotlin。完整清单见研究的 `api-inventory.json`。本片不声称覆盖 Desktop 的 62 个宿主命令。

后续独立计划：Go daemon 的常驻与单实例、GUI 退出存活、CLI 复用、HTTP/WS、平台服务生命周期、真实 keyring/剪贴板的 Go 或复用 Rust 适配、崩溃隔离取舍评审、Linux/Windows 原生验证与签名分发。
