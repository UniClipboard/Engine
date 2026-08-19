# 应用层按业务领域收口

> 本文中的旧成员自动升级布局为历史记录。当前升级行为以 ADR-023 和规格 026 的本机独立化与全部重新配对为准。

## 状态

已采纳，实施中。

本文把 [ADR-018](../adr/018-domain-oriented-application-layout.md) 的目录和入口决策落实为可执行的迁移规格。它只改变应用层内部组织、可见性和 Engine 的组装边界；`uc-engine` 对宿主暴露的操作、结果、错误和事件语义不在本次迁移中改变。

相关规格：

- [工作空间全局收敛](016-workspace-wide-convergence.md)
- [配对作为工作空间准入通道](017-pairing-as-workspace-admission.md)
- [uc-engine 跨平台核心接口](uc-engine-interface.md)
- [仓库检查](engine-repository-checks.md)

## 目标与完成定义

完成本规格时，以下条件必须同时成立：

1. `uc-application` 的业务实现按剪贴板、空间、传输、搜索和设置五个领域组织；不存在 crate 根目录的 `usecases/`，也不存在领域内同名嵌套目录。
2. 每个领域有一个能负责完整结果的内部负责人。短动作、持续工作、恢复、关闭和测试位于同一领域，而不是由 Engine、`facade/` 或调用方拼接。
3. `facade/` 只包含外部调用方需要理解的入口和类型；运行期、协调器、会话、缓存、事件总线、投影构建和内部适配全部离开该目录。
4. `uc-engine` 除 `uc_application::facade` 和 `uc_application::deps` 外，不再导入或持有应用层类型。它仍负责构造基础设施和宿主适配，但不再手工推进任何领域流程。
5. 所有旧模块路径、公开再导出、旧构造入口和临时转发层在对应领域迁移完成的同一轮删除，不保留兼容路径。
6. 默认 P2P、持久化加密、既有恢复边界和稳定 Engine 合同保持不变，并通过本规格的检查矩阵。

## 范围

本规格覆盖：

- `crates/uc-application/src/` 的目录、模块可见性和对外入口；
- `crates/uc-engine/` 对应用层的组装、持有和调用方式；
- `uc-infra` 中具体密码适配的归属；
- 用户明确启用的 LAN 兼容线与默认 P2P 路径的隔离；
- 对应单元、集成、架构边界和公开合同检查；
- 与本决策有关的架构文档和应用层维护说明。

本规格不覆盖：

- 改写领域规则、数据库结构、持久化格式或既有密文记录；
- 改变 Engine 对宿主暴露的操作名、结果、错误码或事件语义；
- 用目录迁移顺带重写 P2P、LAN、文件协议或配对协议；
- 为了目录完整性预先创建空目录或新的通用抽象层。

## 已确认的现状

审计确认当前问题不是单一的 `usecases/` 目录，而是三类历史结构同时存在：

| 现状 | 后果 | 本规格的处理 |
| --- | --- | --- |
| crate 根目录、`usecases/`、领域内 `usecases/` 和 `facade/` 同时承载同一业务 | 无法从一个位置找到完整流程 | 以业务领域为第一层，同一轮移动实现、测试、内部引用和可见性 |
| `facade/` 包含运行期、协调器、会话、缓存、投影构建、事件总线和具体适配 | 外部调用方能够绕过完整负责人，门面失去边界含义 | 只留下稳定入口与外部类型，其余移入领域或 `support/` |
| Engine 直接构造并持有剪贴板写入、入站接收、传输接收、空间收敛和搜索运行期等对象 | Engine 重新掌握业务步骤，目录移动后复杂度仍会分散 | Engine 只组装依赖并调用门面级完整动作 |
| `AppFacade` 已成为平铺动作清单 | 新动作持续增加总入口的表面面积 | 改为聚合少量领域入口，领域门面承载命令、查询和订阅 |

工作区内只有 `uc-engine` 直接依赖 `uc-application`；绑定和移动宿主只依赖 `uc-engine`。因此收紧应用层内部路径不会改变绑定的对外能力，但 Engine 和它的集成测试必须在迁移中同步修改。

## 固定目录形状

以下是迁移完成后的稳定组织。它表示归属，不要求为尚无实现的子域创建空目录。

```text
crates/uc-application/src/
  facade/                         # 唯一对外业务入口和外部需要理解的类型
    mod.rs                        # 对外白名单
    app_facade.rs                 # 少量领域入口的聚合
    clipboard.rs
    space.rs
    transfer.rs
    search.rs
    settings.rs
  clipboard/
    capture/
    write/
    history/
    restore/
    sync/
    active/
    entry_identity.rs
    file_set_query.rs
  space/
    lifecycle/
    admission/
    roster/
    convergence/
  transfer/
    blob/
    file/
    receive/
  search/
  settings/
  support/
  deps.rs                         # 仅组装数据
```

`facade/` 的领域文件可以按需要拆分为只含公开合同的子模块，但不能再次收纳领域实现。目录名不是判断标准：一个文件即使名称带有 `facade`，只要它执行重试、恢复、后台任务、协议处理、缓存管理或步骤级编排，就属于对应业务领域或 `support/`。

### 目录归属规则

| 位置 | 允许内容 | 禁止内容 |
| --- | --- | --- |
| `facade/` | `AppFacade`、领域门面、命令、查询、结果、错误、稳定状态和订阅合同 | 运行期、协调器、会话、用例实现、缓存、事件总线、投影构建、具体适配和依赖构造 |
| 领域目录 | 该领域完整流程、短动作、持续工作、恢复、关闭、内部测试和私有适配位置 | 无业务所有权的通用杂项，或另一个领域的流程 |
| `support/` | 不拥有业务结果的小型共享能力，例如事件投递实现和无业务语义的内存缓存 | 业务顺序、重试策略、持久化恢复或任何领域流程负责人 |
| `deps.rs` | Engine 构造应用层所需的被动依赖分组和端口 | 工厂逻辑、动作、运行期、流程状态和可由领域自行创建的对象 |
| `uc-infra` | 密码、数据库、网络、文件系统等具体能力适配 | 业务顺序、加入完成判定、重试和恢复策略 |
| LAN 兼容线 | 仅用户显式启用的 LAN 工作流与其特有适配 | 默认 P2P 路径、P2P 失败时的自动接管 |

`use case` 仍是短动作的职责名称，可以作为文件名或类型名使用；它不再是跨领域或领域内部的目录分类。持续工作可以使用 `runtime.rs`、`coordinator.rs` 等文件名，但必须位于所属领域，不能重建根 `runtime/` 目录。

## 唯一负责人

| 领域 | 完整结果 | 子域与内部责任 | 外部入口 |
| --- | --- | --- | --- |
| `clipboard` | 一条本地或远端内容从捕获、保存、同步、恢复到活动状态和历史可查询的完整结果 | 捕获、写入、历史、恢复、入站、出站、重发、活动状态、资源读取和文件集查询 | `ClipboardFacade` |
| `space` | 空间创建、解锁、会话、准入、切换、成员操作、成员变化收敛和网络恢复 | `lifecycle`、`admission`、`roster`、`convergence`；准入通道是收敛负责人的私有内部协作，不形成第二个流程负责人 | `SpaceFacade` |
| `transfer` | 内容或文件从发布、接收、进度、取消到重启恢复和清理的完整结果 | blob 发布和获取、文件会话、接收就绪、接收尝试核对、传输事件投递 | `TransferFacade` |
| `search` | 查询、实时索引、重建、修复、维护和关闭的完整结果 | 查询、标签、投影、索引运行期和维护 | `SearchFacade` |
| `settings` | 设置读取、校验、保存、升级、配置迁移、存储统计和中继诊断的完整结果 | 配置迁移、升级、存储、诊断和中继设置 | `SettingsFacade` |

没有独立领域价值的原 `trusted_peer/` 应用模块不迁移。空间收敛已经负责需要的关系事实和推进；`uc-core` 中仍被收敛与投递使用的可信对端事实、端口和仓储能力保留。

## 对外边界与 Engine 规则

### 应用层公开表面

迁移完成后，`crates/uc-application/src/lib.rs` 只能公开：

```rust
pub mod deps;
pub mod facade;
```

所有业务领域模块均为 `pub(crate)` 或私有。`facade/mod.rs` 是唯一公开白名单，只能导出：

- `AppFacade` 和五个领域门面；
- 门面方法需要的命令、查询、结果、错误、稳定状态和订阅类型；
- 外部事件的稳定合同。事件的总线、注册表和投递实现不在 `facade/`；
- 不能以“测试需要”或“Engine 当前会构造”为理由公开运行期、协调器、会话、裸用例或内部端口。

`AppFacade` 聚合领域入口，而不是继续增加平铺转发方法。现有稳定 Engine 操作应改为调用相应领域门面；一项新能力先确定其领域入口，再确定内部实现。若一次外部调用需要多个内部步骤，领域门面只选择一个完整负责人，不在门面中公开中间步骤。

### Engine 的唯一调用方式

Engine 的职责固定为：构造 `uc-infra` 和宿主适配、填充 `AppDeps`、创建 `AppFacade`、把稳定 Engine 操作转换为对应领域门面调用，以及调用门面级生命周期动作。它不得直接构造、保存或调用应用层内部对象。

迁移完成后，Engine 只允许出现以下形式的应用层导入：

```rust
use uc_application::deps::...;
use uc_application::facade::...;
```

以下对象不得继续成为 Engine 字段、函数参数、返回值或集成测试的构造对象：

- `ClipboardInboundRuntime`、`ClipboardSyncRuntime`、`ClipboardWriteCoordinator`、`CaptureClipboardUseCase` 和入站用例依赖；
- `FileTransferSession`、接收就绪协调器、接收核对用例和 `FileTransferLifecycle`；
- `WorkspaceConvergence`、其运行期、群组更新投递器、成员连通和网络恢复内部对象；
- `SearchRuntime`、历史维护运行期、移动上传协调器、`HostEventBus` 和内部缓存；
- 任何领域的 `*Deps`、`*Coordinator`、`*Runtime`、`*Session`、`*UseCase` 或具体实现适配。

如果 Engine 需要启动、暂停、恢复、关闭某个后台流程，必须发出一个门面级完整生命周期动作。应用层在内部决定哪些领域运行期参与、它们的顺序、失败处理和关闭等待；Engine 不能按自己的顺序分别启动或停止它们。若 Engine 需要更换事件接收方，也必须通过一个明确的门面级事件注册动作，而不是取得事件总线对象。

`AppDeps` 保持纯数据分组。它可以携带 `uc-core` 端口、宿主回调和已构造的基础设施适配，但不得携带已经启动的领域运行期、流程负责人或绕过门面的动作入口。

## 路径迁移表

下表是本规格的删除和归属清单。表中“移入”表示在同一轮完成实现、测试、内部引用和公开入口收口；旧路径随后删除。

### 集中和嵌套的 `usecases/`

| 当前路径 | 目标归属 | 要求 |
| --- | --- | --- |
| `usecases/blob_transfer/` | `transfer/blob/` | 发布、获取和路径变体随 blob 传输一起移动 |
| `usecases/clipboard_history/` | `clipboard/history/` | 清理、保留、删除、详情、资源、收藏和文件核对一起移动 |
| `usecases/clipboard_restore/` | `clipboard/restore/` | 恢复、纯文本和文件路径模式一起移动 |
| `usecases/clipboard_sync/` | `clipboard/sync/` | 入站物化、编解码、发送、重发、接收门、活动状态与测试不再分散 |
| `usecases/pairing/` | `space/admission/` | 与现有准入通信实现收成一个子域；它仍由空间收敛负责完整加入结果 |
| `usecases/presence/` | `space/convergence/connectivity/reachability.rs` | 可达性保证属于成员收敛，不形成独立 presence 领域 |
| `usecases/setup/` | `space/lifecycle/` | 创建、解锁、切换、恢复和重置随空间生命周期收口 |
| `usecases/search/` | `search/query.rs` | 查询与索引运行期、投影和维护同属搜索领域 |
| `usecases/upgrade/` | `settings/upgrade/` | 检测、状态和确认一起移动 |
| `usecases/mobile_sync/` | 专用 LAN 兼容模块 | 不混入默认 `clipboard/`，也不移入纯协议叶子 `compatibility/uc-mobile-proto` |
| `space/roster/usecases/` | `space/roster/` | 直接拍平到成员名单子域，不保留嵌套 `usecases/` |
| `space/convergence/admission/` | `space/admission/` | 目录移动不改变 ADR-017 的所有权：准入通信仍是收敛负责人的私有协作 |

### crate 根目录的历史模块

| 当前路径 | 目标归属 | 要求 |
| --- | --- | --- |
| `clipboard_capture/` | `clipboard/capture/` | 捕获用例和捕获门面的内部实现一同移动 |
| `clipboard_write/` | `clipboard/write/` | 写入协调、活动状态推进、恢复广播和移动可用性判断一同移动 |
| `sync_planner/` | `clipboard/sync/outbound_plan.rs` | 发送计划属于出站同步，不形成新的通用模块 |
| `entry_identity.rs` | `clipboard/entry_identity.rs` | 所有条目身份写入共享同一内部负责人 |
| `file_set_query.rs` | `clipboard/file_set_query.rs` | 文件集查询属于剪贴板内容读取 |
| `content_tags.rs` | `search/tagging.rs` | 标签是搜索索引的业务事实 |
| `file_transfer/` | `transfer/file/` | 文件时间线和错误模型与文件会话一起移动 |
| `receive_reconciliation.rs` | `transfer/receive/reconciliation.rs` | 接收就绪、启动恢复、超时清理和接收尝试核对由传输完整负责 |
| `group_update_delivery.rs` | `space/convergence/membership/group_update_delivery.rs` | 不再由 Engine 构造并注入 |
| `trusted_peer/` | 删除 | 不迁移应用层流程；保留 `uc-core` 的相关事实和端口 |
| `file_sync/` | 删除 | 唯一配额函数未被调用且依赖失效缓存路径；需要新配额能力时从当前缓存模型重新设计 |
| `proof.rs` | `uc-infra` 的 `security/admission_proof.rs` | `HmacProofAdapter` 是具体密码实现；Engine 从 `uc-infra` 组装为 `ProofPort`，应用层只依赖该端口 |

### 误放在 `facade/` 的实现

| 当前内容 | 目标归属 | 门面保留内容 |
| --- | --- | --- |
| `active_clipboard/`、`clipboard_capture/`、`clipboard_history/`、`clipboard_inbound/`、`clipboard_outbound/`、`clipboard_restore/`、`clipboard_sync_runtime.rs`、`resource/` | `clipboard/active`、`capture`、`history`、`sync`、`restore` 和 `resource` | `ClipboardFacade` 及其公开命令、查询、结果、错误和订阅 |
| `clipboard_live_index/` | `search/live_index.rs` | 搜索或剪贴板对外状态，不公开索引器 |
| `blob_transfer/`、`file_transfer/` 和传输事件发布 | `transfer/blob`、`transfer/file`、`transfer/events.rs` | `TransferFacade` 和稳定传输状态 |
| `space_setup/`、`space_admission.rs`、`space_session.rs`、`space_runtime.rs`、`setup_status/`、`encryption/`、`device/` | `space/lifecycle` 与 `space/admission` | `SpaceFacade` 和空间状态、命令、查询、订阅 |
| `roster/` | `space/roster/` | 成员名单的公开命令、查询、结果和错误 |
| `membership_connectivity.rs`、`network_recovery.rs`、`legacy_upgrade.rs` | `space/convergence/` | 空间收敛和恢复的公开状态与订阅 |
| `search/` 的 coordinator、projection、runtime | `search/` | `SearchFacade` 的查询和状态 |
| `config_migration/`、`storage/`、`diagnostics.rs`、`upgrade/`、`settings/` 中的实际流程 | `settings/` 对应子域 | `SettingsFacade` 的公开设置和诊断合同 |
| `host_event/bus.rs`、`host_event/publisher.rs`、`host_event/outbound_entry_cache.rs` | `support/host_event_bus.rs`、`support/outbound_entry_cache.rs` | 只保留稳定事件类型和事件接收合同 |
| `app_paths.rs` 与 `host_event/event.rs` 的转发别名 | 删除转发 | Engine 直接使用其 `uc-core` 的规范定义 |
| `mobile_sync/`、移动上传与内部衔接 | 用户显式启用的 LAN 兼容模块 | LAN 操作的稳定 Engine 合同不变，不把兼容实现留在默认门面 |

### LAN 兼容线

LAN 兼容能力是与默认 P2P 主路径隔离的独立工作流。实施时必须满足：

1. `crates/uc-engine/src/compatibility/mobile_lan/` 继续只负责稳定 Engine 合同转换和显式 feature 门；LAN 工作流实现从默认 `uc-application` 目录移出，进入专用兼容模块。
2. 若需要新的兼容 crate，它必须位于 `compatibility/`、有独立版本和发布来源，并且只在 `lan-compat` 显式启用时成为依赖；不得把它放回默认 `uc-application`。
3. `compatibility/uc-mobile-proto` 保持零内部 workspace 依赖的纯协议叶子，只保存确定性编解码和协议数据，绝不放入设置、认证、持久化、上传、接收或运行期流程。
4. P2P 失败、离线或任一恢复失败都不能自动启用或切换到 LAN；用户显式设置和现有 listener 状态仍是唯一前提。

## 必须删除的旧入口

每个领域迁移完成时，以下类型的遗留物必须同一轮删除：

- 对应的旧目录、`mod.rs`、crate 根 `pub mod` 和 `pub use`；
- 指向旧目录的 `facade` 再导出、类型别名、过渡构造器和转发方法；
- Engine 中直接构造或持有旧内部对象的字段、依赖分组、运行期启动和关闭代码；
- Engine 集成测试中直接构造应用层内部类型的辅助函数；
- 只为了保留旧模块路径、旧依赖组或旧生命周期顺序而存在的测试；
- 没有调用者、依赖过时存储布局或只作为“以后可能会用”的实现。

禁止以 `deprecated`、类型别名、重新导出、双写、回退或 feature 开关的方式让旧路径继续编译。源码路径不是持久化数据，不需要迁移层；编译失败应当暴露尚未完成的调用点。

## 分阶段迁移

各阶段可以拆成若干可独立验证的提交，但一个领域的旧路径不能跨阶段保留。每次移动都先改调用方，再在同一变更中删除旧入口。

### 阶段 0：建立约束，不建立空骨架

- 将本规格、ADR-018、`architecture-bible.md` 和 `crates/uc-application/AGENTS.md` 对齐。
- 在现有 `dependency_firewall` 和公开合同检查中加入最终规则的测试位置；检查不能依赖文件名猜测，而应检查模块可见性、外部导入和允许的公开表面。
- 列出每个 Engine 直接持有的应用层对象，并为其确定对应的领域门面动作。
- 不创建空目录，不增加兼容导出。

### 阶段 1：先收口空间与密码适配

- 将 `HmacProofAdapter` 和其测试移动到 `uc-infra`，Engine 从 `uc-infra` 注入 `ProofPort`；删除 `uc_application::proof`。
- 将生命周期、准入、成员名单、成员连通、网络恢复、旧成员升级和群组更新投递收进 `space/`。
- 合并两套准入目录，删除 `trusted_peer/` 应用流程和根 `usecases/pairing`、`usecases/presence`、`usecases/setup`。
- 改造 Engine，使空间操作、空间会话和收敛运行期只通过 `SpaceFacade` 及其完整生命周期动作进入。
- 保留 ADR-015、ADR-016、ADR-017 已定义的恢复、准入和完成语义。

### 阶段 2：收口传输

- 将 blob、文件会话、接收就绪、接收尝试核对、启动恢复、超时清理和传输事件移动到 `transfer/`。
- 应用层内部拥有文件传输的启动、恢复、取消和关闭；Engine 不再持有 `FileTransferLifecycle`、接收就绪协调器或会话对象。
- 删除根 `file_transfer/`、`receive_reconciliation.rs` 和 `usecases/blob_transfer/`。

### 阶段 3：收口剪贴板与搜索

- 将捕获、写入、历史、恢复、入站、出站、重发、活动状态、身份写入、资源读取和发送计划一起迁到 `clipboard/`。
- 将实时索引、标签、投影、查询、重建、修复、维护和关闭一起迁到 `search/`。
- 传输领域只向剪贴板提供完整传输结果；剪贴板不能重新掌握接收会话、超时清理或文件恢复。
- 删除根 `clipboard_capture/`、`clipboard_write/`、`sync_planner/`、`entry_identity.rs`、`file_set_query.rs`、`content_tags.rs` 以及所有剪贴板和搜索的旧 `usecases/` 路径。

### 阶段 4：收口设置、共享能力和 LAN 兼容线

- 将设置、升级、配置迁移、存储和诊断实现移动到 `settings/`。
- 将事件总线和无业务语义缓存移动到 `support/`；稳定事件合同保留在 `facade/` 或其 `uc-core` 规范定义。
- 删除 `facade` 对 `AppPaths` 和事件类型的转发别名。
- 把 LAN 同步实现移出默认应用层，并保持 feature 和发布隔离；默认构建的依赖闭包不得变化为包含 LAN 兼容能力。

### 阶段 5：最终收紧和删除检查

- 将 `lib.rs` 收敛为仅公开 `facade` 与 `deps`；清除所有历史根模块和 `usecases/`。
- 将 `facade/mod.rs` 收敛为对外白名单，并逐项审查所有 `pub use`。公开类型必须是调用方需要理解的合同，而非内部实现的方便出口。
- 删除 Engine 内部的旁路依赖组和测试构造器，改为门面级集成测试。
- 更新架构检查，使以后新增非门面导入、根 `usecases/`、领域内嵌套 `usecases/` 或门面实现都会失败。
- 运行完整验收矩阵，更新架构文档维护记录。

## 行为与安全不变条件

目录迁移不得改变以下事实：

- 所有业务负载仍按仓库规则以 MasterKey AEAD 加密后持久化；文件内容和受管缓存文件名的既有例外不扩大。
- 日志不记录剪贴板正文、密码、密钥、完整令牌、文件名或路径。迁移 `HmacProofAdapter` 时保留现有脱敏测试和等价的 HMAC 输入、输出与验证语义。
- 空间加入、移除、重新加入、收敛、重启恢复和完成判定仍由 `WorkspaceConvergence` 负责；配对通道不重新成为独立负责人。
- 剪贴板普通接收、只保存拉取、活动状态、历史维护和文件接收的完整模式不改变；调用方仍不能拼接内部步骤。
- 文件接收在取消、失败、重启和关闭时继续由传输负责人完成清理和终态写入。
- 搜索锁定、解锁、重建、修复和关闭的顺序继续由搜索与空间领域内部负责。
- LAN 兼容仍需用户显式开启，P2P 失败不自动回退。
- `uc-engine` 的公开操作、结果、错误和事件保持兼容；本规格不允许借目录迁移引入行为变更。

## 架构检查要求

在现有 `crates/uc-engine/tests/dependency_firewall.rs` 和公开合同检查中增加下列长期约束：

| 检查 | 通过条件 |
| --- | --- |
| 应用层公开表面 | `lib.rs` 只公开 `facade` 与 `deps`；不存在对业务领域模块的顶层公开再导出 |
| Engine 导入 | `crates/uc-engine` 和其集成测试只导入 `uc_application::facade`、`uc_application::deps`；具体密码适配从 `uc-infra` 导入 |
| 目录删除 | 不存在 `src/usecases/`、`space/roster/usecases/`、根 `runtime/`、根 `membership/` 或迁移表中已删除的旧模块 |
| 门面白名单 | `facade/` 不包含运行期、协调器、会话、缓存、事件总线、投影构建或具体适配；这些类型不出现在公开导出中 |
| Engine 持有 | Engine 不持有领域内部运行期、协调器、会话、用例、事件总线或领域依赖组；后台流程只能由门面级完整动作启动和关闭 |
| LAN 隔离 | 默认 `uc-engine`、`uc-application`、`uc-infra` 依赖闭包不启用 LAN；`uc-mobile-proto` 继续是协议叶子 |
| 删除检查 | 删除任一领域负责人时，业务顺序、恢复和关闭复杂度会重新出现在调用方，说明负责人不是空转发层；若不会，合并或删除该负责人 |

检查可以读取源码和 Cargo 元数据，但必须基于明确的允许列表和禁止路径；不能仅因类型名中含有 `Runtime` 或 `Facade` 就判断架构归属。

## 验收矩阵

| 类别 | 验收内容 | 结果要求 |
| --- | --- | --- |
| 模块结构 | 所有迁移表的目标存在，旧路径和转发层不存在 | 通过 |
| 对外边界 | 非应用层代码不导入应用层内部模块，Engine 不持有内部流程对象 | 通过 |
| 空间 | 创建、解锁、加入、成员操作、移除、重新加入、网络恢复与重启收敛 | 相关单元和集成测试通过 |
| 剪贴板 | 捕获、普通接收、只保存拉取、历史、恢复、重发、活动状态和搜索联动 | 相关单元和集成测试通过 |
| 传输 | blob、文件发布与接收、进度、取消、超时、重启恢复和清理 | 相关单元和集成测试通过 |
| 搜索 | 查询、索引、重建、锁定、恢复和关闭 | 相关单元和集成测试通过 |
| 设置与兼容 | 升级、配置迁移、存储、诊断和显式 LAN 开关 | 相关单元和集成测试通过；默认构建不启用 LAN |
| 安全 | 密文持久化、日志脱敏和证明校验 | 现有与新增安全回归测试通过 |
| iOS、Android、HarmonyOS | 完整 Engine 宿主的核心操作和生命周期 | 实施完成时实际运行；未运行必须记录为“跳过”，不得记为“通过” |

每个迁移阶段至少运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
cargo test -p uc-engine --test dependency_firewall --locked
cargo test -p uc-engine --test public_contract --locked
git diff --check
```

完成所有阶段时，还必须运行领域相关测试，以及：

```bash
cargo test -p uc-application --all-features --locked
cargo test -p uc-engine --all-features --locked
```

任何测试失败都不能用保留旧路径、公开内部对象或跳过架构检查来换取通过。先修正该领域的完整负责人和调用边界，再继续迁移。
