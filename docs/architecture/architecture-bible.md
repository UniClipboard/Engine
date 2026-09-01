# UniClipboard 架构圣经

本文是 `UniClipboardEngine` 当前架构的总入口，回答系统为什么存在、如何分层、数据如何保存、设备如何通信，以及各平台应如何接入。

本文描述的是当前仓库事实，不是未来设计。具体接口以 `crates/uc-engine/` 的公共契约为准，安全规则以 `AGENTS.md` 和 `docs/security/` 为准，已批准但尚未实现的方向以 ADR 或规划文件为准。发生冲突时，先修正文档与实现的不一致，不能默认任意一方正确。

## 1. 项目目标

UniClipboardEngine 是 macOS、Windows、Linux、iOS、Android 和 HarmonyOS 共用的端到端加密剪贴板引擎。它本身不是可直接运行的应用，而是由桌面或移动宿主接入的共享核心。

项目要实现五个结果：

1. 所有平台使用同一套设备身份、空间、配对、同步和内容规则。
2. 默认通过经过身份认证的加密 P2P 通道同步文字、图片、文件和活动剪贴板状态。
3. 历史、搜索、文件传输和成员关系可以跨重启恢复，同时业务负载默认只以密文持久化。
4. 平台只提供系统能力，不复制加密、持久化、配对、传输、恢复和迁移流程。
5. 每次发布都能追溯到唯一版本、源码提交、产物校验值和设备验收记录。

### 不做什么

- 不提供产品界面或平台业务页面。
- 不允许外部直接依赖内部 crate。
- 不在 P2P 失败时自动切换到 LAN HTTP。
- 不让宿主自行拼装数据库、加密、发送、重试或恢复步骤。
- 不把移动平台验收宿主发展成另一套产品实现。

## 2. 整体架构

系统采用端口与适配器分层。依赖方向从外向内组装，但业务知识从领域层向外逐级收敛。

```text
桌面宿主       iOS / Android       HarmonyOS
   |               |                  |
Rust API       UniFFI 绑定          N-API 绑定
   \_______________|__________________/
                   |
               uc-engine
          稳定操作、结果、事件、生命周期
                   |
             uc-application
       完整业务流程、顺序、恢复和后台任务
                   |
                uc-core
        领域模型、规则、状态和能力约定
                   ^
                   |
               uc-infra
   SQLite、MasterKey AEAD、搜索、文件、Iroh P2P
```

### 分层责任

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| `uc-core` | 领域模型、业务规则、状态和能力接口 | 数据库、网络框架、操作系统调用 |
| `uc-application` | 一项业务从开始到结束的完整流程 | 对外稳定协议、具体基础设施 |
| `uc-infra` | SQLite、加密、搜索、文件缓存、Iroh 网络等具体能力 | 决定业务顺序和产品规则 |
| `uc-engine` | 组装内部能力，暴露唯一稳定入口 | 复制领域规则或让宿主逐步编排内部流程 |
| `bindings/` | 语言类型、异步运行时和平台调用转换 | 平台专属业务规则 |
| 宿主应用 | 目录、安全存储、系统剪贴板、文件句柄、生命周期通知，以及经用户许可的观测发送和身份保存 | 核心业务状态、事件定义和身份切换顺序 |

### 稳定边界

外部只依赖 `uc-engine`，并只理解以下概念：

- `EngineConfig`：宿主版本和资料空间。
- `HostCapabilities`：目录、安全存储、剪贴板、文件访问，以及可选的脱敏观测能力。
- `Engine::start`：启动引擎并取得事件流。
- `Operation` / `OperationResult`：执行命令或查询并取得稳定结果。
- `EngineEvent`：状态变化、操作终态、传输和刷新通知。
- `EngineError`：稳定编号、错误类别和是否可重试。
- `suspend` / `resume` / `shutdown`：平台生命周期。

内部 crate、数据库表、网络库类型、加密实现和后台任务均不属于稳定边界。

### 运行时所有权

一个跨层功能必须由一个模块负责完整结果。调用方只发起一次操作，不得知道内部的持久化顺序、网络握手、补偿、重试和关闭细节。

删除负责模块时，如果这些知识会重新散落到多个调用方，说明模块有效隐藏了复杂度；如果删除后没有变化，说明它只是转发层，应合并或重新划分。

## 3. 核心模块

### 工作区模块

| 模块 | 主要职责 |
| --- | --- |
| `crates/uc-engine/` | 唯一稳定 Rust 入口；操作路由、结果转换、事件流、生命周期和内部组装 |
| `crates/uc-core/` | 空间、成员、配对、剪贴板、文件传输、搜索、设置、安全和端口定义 |
| `crates/uc-application/` | 空间会话、配对准入、剪贴板出入站、历史、搜索、文件传输、成员收敛和恢复流程 |
| `crates/uc-infra/` | SQLite 仓储、MasterKey AEAD、安全状态、blob、搜索索引、文件缓存和 Iroh P2P |
| `crates/uc-content-hash/` | 跨平台一致的内容身份摘要算法，不依赖其他业务模块 |
| `crates/uc-observability-contract/` | 宿主与核心共享的脱敏观测约定 |
| `bindings/uc-engine-uniffi/` | iOS 和 Android 绑定及 XCFramework、AAR 打包 |
| `bindings/uc-ohos-napi/` | HarmonyOS N-API、ArkTS 声明和 HAR 打包 |
| `compatibility/` | 用户显式选择的 LAN HTTP 兼容线，独立版本和发布 |
| `tests/hosts/` | 三个移动平台的接入验收宿主，不承载产品功能 |

### 应用层目录归属（ADR-018）

`uc-application` 先按业务领域组织：剪贴板、空间、传输、搜索和设置各自保有短动作、持续流程和
测试；跨领域且无业务所有权的小型能力才进入 `support/`，`deps.rs` 只保存组装数据。`use case`
只是短动作的职责，不是目录归属，因此不保留集中 `usecases/`，领域内部也不嵌套同名目录。
短动作直接位于所属领域或有明确业务含义的子域，持续工作也留在所属领域的 `runtime/` 或
`coordinator/` 中。`facade/` 仍是唯一对外业务入口，按领域提供入口而不保存流程实现。运行期、
协调器、会话、内部适配、事件总线、缓存和投影构建不得留在 `facade/`；它们分别归入所属领域或
无业务所有权的 `support/`。稳定事件类型可以由门面公开，事件投递实现不在门面目录。

完整的路径迁移表、Engine 收口规则、删除清单和验收矩阵见[规格 018](../exec-plans/active/018-domain-oriented-application-layout.md)。

### 领域模块

| 领域 | 核心事实 | 完整流程归属 |
| --- | --- | --- |
| 本机设备 | 本机设备编号和显示名 | `uc-application` 的本机设备查询 |
| 空间与加密会话 | 空间身份、设置状态、解锁状态、密钥世代 | `uc-application` 的空间会话与空间切换流程 |
| 配对与准入 | 一次性邀请、加入方、发起方、身份指纹、信任关系 | 配对入站、出站和空间准入流程 |
| 成员关系 | 当前分支成员、可信对等端、同步偏好、在线状态和历史关系 | 成员历史核对与用户决定流程 |
| 剪贴板历史 | 事件、条目、表示、选择、资源、收藏和标签 | 捕获、物化、历史维护、恢复和删除流程 |
| 活动剪贴板 | 当前内容的跨设备收敛值 | 活动状态登记、广播、拉取和应用流程 |
| 文件传输 | 传输身份、时间线、进度、结果和接收尝试 | 文件导入、发布、接收、提交、取消和清理流程 |
| 搜索 | 搜索文档、不可逆词项标签、标签和索引状态 | 实时索引、查询、重建和隐私维护流程 |
| 设置 | 同步、网络、保留、安全和文件偏好 | 设置读取、事务更新和中继诊断流程 |

空间会话是否已解锁是多个应用流程共享的运行期能力，由 `uc-application/src/space/lifecycle/session/` 定义；状态查询、
活动剪贴板、入站处理和配置迁移复用同一接口，基础设施提供实现，core 不保存该应用接口。
当前 Space 是否存在以及会话是否已解锁，由 `uc-application/src/space/lifecycle/query_space_access_state/` 的单一查询
用例组合；`AppFacade` 只转发查询。当前 profile 密钥能否从系统安全存储静默读取，由
`uc-application/src/profile/probe_profile_key_access/` 解释底层探测结果；权限拒绝和暂时不可用返回未授权，
密钥缺失返回未初始化。原加密 Facade 已删除。

### 组装原则

- `uc-engine` 可以知道具体实现如何装配，但不能拥有业务规则。
- `uc-application` 可以依赖 `uc-core` 的能力接口，不能依赖宿主类型。
- `uc-infra` 实现能力接口，不能反向调用应用流程。
- 绑定只依赖 `uc-engine`，不能穿透到内部 crate。
- LAN 兼容线不能进入默认 P2P 路径，也不能读取 P2P 失败信号后自动接管。

## 4. 数据模型

### 主关系

```text
ClipboardEvent
  ├─ 1..n ClipboardRepresentation ──> Blob
  └─ 1..n ClipboardEntry
          ├─ 1 ClipboardSelection
          ├─ 0..n EntryDelivery
          ├─ 0..n EntryFileSetItem
          ├─ 0..n FileTransfer
          └─ 0..1 SearchDocument ──> SearchPosting / SearchEntryTag

Space
  ├─ 1..n SpaceMember
  ├─ 1..n TrustedPeer
  ├─ 0..n MembershipAnnouncement / OutboxBatch
  └─ 1 SpaceKeyEpochState ──> Revocation / UpgradeRecoveryLog
```

### 剪贴板模型

- **事件**记录一次捕获的时间、来源设备和跨设备内容摘要。
- **条目**是本机历史中的稳定记录，保存创建时间、活动时间、大小、内容类别、收藏和是否启用投递追踪。
- **表示**是一份内容的某种系统格式，例如纯文本、HTML、图片或文件列表。小内容可内联，大内容引用 blob。
- **选择**记录主表示、辅助表示、预览表示和恢复系统剪贴板时使用的表示。
- **活动状态**独立于历史排序，表示当前应生效的内容。跨设备身份是内容摘要，本机条目编号不能用于跨设备比较。
- 普通远端接收与只保存拉取是两种完整模式；调用方不能逐项拼装接收能力，也不能用假写入表达只保存语义。
- 普通远端接收由应用层入站运行期完整负责订阅、成员策略、解密、内容类型策略、应用、宿主通知、发送方确认和关闭等待。
- 引擎只提供完整依赖、转换轻量宿主事件并启动或关闭该运行期，不订阅中间通知，也不判断应用结果或推进发送方确认。

### 内容可用状态

| 状态 | 含义 |
| --- | --- |
| `Inline` | 小内容已以内联密文保存 |
| `BlobReady` | 大内容已物化到 blob，并有可读取引用 |
| `Staged` | 已登记，等待后台处理 |
| `Processing` | 后台正在物化 |
| `Failed` | 本轮处理失败，可带内部失败记录 |
| `Lost` | 内容已永久不可恢复 |

### 文件与传输模型

- 文件集合把一个剪贴板条目中的文件或目录展开为有序项目。
- 目录同步使用文件集合清单，不把目录当成单个不可解释对象。
- 文件内容可以以原始字节保存在受管 blob 或入站缓存中。
- 文件名、原始路径、相对路径、根目录名、传输元数据和恢复记录必须加密。
- 文件传输使用事件时间线记录开始、进度、完成、失败和取消；接收尝试另用尝试编号避免旧操作影响新一轮接收。
- 接收方的用户取消一旦取得该尝试的原子取消权，就由应用层取消在途下载、清理已登记的临时目录、写入已取消终态并发出终态事件；普通文件和目录都不能停留在中间取消状态等待其他回调。

### 成员与安全模型

- 签名 V2 成员历史是成员资格的唯一正向事实；正式 `AddDevice` 是新增资格的唯一入口。
- 事件作者资格和验签凭据来自事件的精确父历史，不能从当前 MLS 树、成员投影、可信关系、地址或在线状态推断。
- 在线状态只是可达性观察；可信对等端只是身份关系；二者都不能授予成员资格。
- 移除决定只影响后继权限，验证旧历史所需的身份资料继续保留。
- 未由本机接受的远端移除先加密保存并等待用户；接受后应用，拒绝后只隔离相关设备关系。
- 成员变化与必要的安全世代更新必须可恢复；正式事实保存后不因网络或后续影响暂时失败而回滚。

### Application Space 成员关系（ADR-025，规格 027）

`SpaceFacade` 是唯一公开 Space 业务入口，`SpaceApplication` 一次构造成员用例、历史 endpoint、准入 endpoint 和唯一维护运行期。内部按 `lifecycle/`、`admission/`、`membership/` 和 `connectivity/` 划分责任，外部不得穿透目录取得用例或状态对象。

`membership/ledger` 通过一个条件原子提交边界共同保存历史、关系、分页传输、待执行影响和 revision。所有普通授权范围都由一次已验证 ledger 读取派生；旧成员表、地址、在线状态和安全残留不能作为回退授权来源。

查询、移除、用户决定、历史交换、准入和后台维护各有一个完整入口。成员维护运行期统一处理启动恢复、状态变化、周期检查、设备上线、暂停和关闭，调用方不安排内部步骤或重试。

### 成员上线核对与用户决定（ADR-020）

已认证设备上线后，Application 进行有界成员历史核对：历史一致时只确认摘要，存在差异时补齐可验证事件。连续新增可以自动应用；待决定移除阻塞其后继事件。

本机投影尚未知的设备可以提交带准入声明的摘要和有界后缀。Infra 只把 Iroh 远端公钥指纹绑定到声明设备；Application/Core 在原子提交前验证完整签名单父历史、发送者准入事实、当前资格和身份签名。未知声明不得直接取得成员权限，一致摘要和反向历史投递仍只接受本机当前成员。

用户接受移除后双方继续收敛；用户拒绝时保留本机分支并把相关关系标为分叉。分叉、无效资料和版本不兼容是不同状态，均不能伪装成离线或自动选择赢家。

可达性、当前分支成员资格和双方历史关系是独立事实。成员核对、决定、关系门禁、加密保存和重启恢复全部由 Application 负责；产品只提交一次决定并读取完整结果。

### 单一 Space 准入协议（ADR-017、ADR-022，规格 028）

`SpaceAdmissionProtocol` 是加入 Space 的唯一完整负责人。产品和绑定只提交一次加入、取消或查询动作，不接触候选、密码交换、历史分页、安全暂存、重试或恢复步骤。

协议使用一套类型化消息和封闭状态：

```text
JoinRequest → Candidate → Prepared → Commit → Applied → Complete
            → CompleteAck → Settled
```

- `Commit` 是唯一新增成员事实；此前的握手和候选都不授予成员资格。
- 每步先原子保存状态、固定回复和待执行影响，再发送网络消息。
- 重复消息只重放已保存回复；乱序、冲突、错误身份或错误前序失败关闭。
- `Prepared` 后不能由新的 JoinSpace 取代；`Commit` 后取消返回已太晚并继续恢复同一次加入。
- Joiner 只有完成本机 Space 激活并保存 `Active` 后才能进入普通成员运行范围。
- Completion Helper 只能恢复已经提交的同一加入，不能创建成员事实。

Core 保存完整 admission aggregate 和状态转换规则。Application 内部按 Joiner、Sponsor 和 Recovery 三个角色隐藏流程，只持有完成各自结果所需的窄能力。Infra 提供 OPAQUE、OpenMLS、Iroh transport 和加密 repository，不决定业务阶段。Engine 只负责组装和生命周期。

成员账本、admission aggregate 和 OPAQUE credential 都使用 MasterKey AEAD 加密保存：

- `SqliteMembershipLedger` 是成员历史、关系、待执行影响和 revision 的唯一提交边界。
- admission repository 保存完整可恢复协议状态，使用版本凭证防止旧读取覆盖新状态。
- `SqliteSpaceAdmissionCredentials` 保存绑定当前 Space 存储作用域的 OPAQUE setup 与 registration：当前版本首次建 Space 必须在初始化提交阶段整体提升为 active generation，并绑定完整 generation；legacy 作用域只用于识别升级前已经完成设置的旧资料，不能由新初始化继续产生。
- 口令、私密 MLS 状态、continuation credential、文件路径和协议载荷不得进入日志或明文字段。

生产网络只使用 `/uniclipboard/space-admission/1`。完整邀请携带 Sponsor admission route 和随机邀请身份；短码只用于一次性解析同一完整邀请。Iroh handler 完成认证后，每条业务消息只调用一次 Application endpoint。

启动顺序固定为：

```text
构造出站 transport
  → 构造 dormant Space application
  → 安装认证 endpoint 与 credential provider
  → 启动 Iroh Router
  → 启动 Application 恢复运行期
```

任何中断都从同一加密状态向前恢复，不回退旧 pairing session、旧 `SpaceJoinRecord`、旧 outbox 或完成接力通道。跨 Space 切换和 Reset 以活动 Space manifest 的原子替换作为唯一生效点；Factory Reset 是唯一销毁本机 profile 密钥与受管资料的操作。

### 搜索模型

搜索只在本机执行，不是远程可搜索加密协议。

- 每个可搜索条目对应一份搜索文档。
- 词项写入前用搜索专用密钥生成 HMAC 标签，磁盘不保存明文词项。
- 预览、文件名、文件路径、链接等渲染字段整体加密。
- 内容删除时同步硬删除文档、倒排项和标签关系。
- 索引版本变化时全量重建；锁定状态不能查询、更新或重建。
- 生产搜索由应用层搜索运行期一次性完整构造，并由它负责后台重建、修复和关闭；引擎层不单独启动或终止搜索任务。
- 空间锁定前必须暂停并等待搜索后台工作退出，解锁或恢复后由空间会话统一恢复；进程关闭后不得再次开启搜索后台工作。
- 无后台能力的场景必须使用明确的只读搜索模式，不能先创建半成品再运行中补装。

V3 搜索密码边界由 Infra `V3SearchProtection` 独占。profile 搜索根从 `ProfileContentVaultKey` 域分离派生；索引调用只提交规范词项，模块从活动 session 固定保护组并生成 opaque group ref 与组隔离 term tags。查询只接收索引中实际存在的 group refs，经 vault 验证后为每个查询词生成一组跨保护组 alternatives；AND 语义按查询词集合判断命中，禁止把全部组 tags 扁平后按总数计数。搜索 render 保留所属模块的 JSON schema 与实体 AAD，但 V3 AEAD、purpose 和历史 key resolution 委托 `ContentProtection`。当前该目标模块不接 production v11 索引；schema/version 与装配只允许在一次性 profile upgrade 和 clean cutover 时共同切换，不能提前触发用旧 key 的普通重建。

### 持久化安全边界

默认规则是：任何写入 SQLite、磁盘缓存或搜索索引的业务负载都先经 MasterKey AEAD 加密，并通过附加认证数据绑定到所属实体。

持久内容使用不可变保护上下文，而不是当前活动 Space 作为解密上下文。V3 密文以随机且全 profile 唯一的 content key id 作为不透明引用，由 profile 加密 key vault 解析 `ProtectionGroupId`，purpose 由所属持久化 adapter 固定，并认证 protection group、content key id、group epoch、purpose 和业务 AAD；`ProtectionGroupId`、`SpaceId` 与 purpose 不作为新增明文密文头或索引字段落盘。活动 Space session 只决定新写入和网络传输使用的保护组，历史读取从密文引用选择保护组。目标 Space 和 peer 不因本机保存历史 catalog 而获得旧内容或旧网络权限。

`ProfileContentKeyVault` 在 Infra 内是一个自有目录的深模块：调用方只安装完整且已验证的 `SpaceKeyMaterial`，或按 content key identity 与精确 epoch 解析；catalog 规范化、跨组冲突、独立 secure-storage key、AEAD framing、资源上限和崩溃安全原子替换全部隐藏在模块内部。Space security 的 V2 content-key catalog codec 是 session 与 vault 的单一事实来源。Vault 文件只保存整体 AEAD 密文，已有文件缺少独立 key、未知 framing、digest/epoch 冲突或密文损坏都失败关闭；Factory Reset 同时擦除 vault key 和 profile 数据目录。

`ContentProtection` 是 Infra 内 V3 持久业务负载的唯一密码深模块：所属 adapter 在构造时固定 purpose，调用方只能执行 `seal_for_active` 或 `open`。新写入上下文来自活动 session；历史读取严格从密文的 key identity 与 epoch 经 profile vault 解析所属保护组，不读取当前 Space。purpose HKDF、完整上下文 AAD、V3 envelope 校验和错误分类都由该模块隐藏。Engine 已组装 profile vault，但 `ContentProtection` 尚未接入 production repository 或 V3 manifest promotion；V2 正常路径及 session 历史 catalog 保持不变，直至全部 at-rest adapter 能一次性 clean cutover。

V3 持久内容 envelope 使用紧凑二进制 framing；inline adapter 直接保存该 envelope，UCBL store 只增加固定外层 magic/version 并拥有 zstd 压缩。两者不得复制 key 解析、purpose 派生或 AEAD header。active register、文件路径、transfer/receive、directory publish 和搜索渲染等专用字段继续由所属模块拥有业务序列化与实体 AAD，但 clean cutover 时必须把序列化后的字节统一委托 `ContentProtection`，不得为每种字段保留独立 V3 密码格式。当前 V3 inline/UCBL 类型只作为升级 target 与最终 production adapter 准备，不在 profile upgrade gate 前接入正常运行路径。

持久 inline payload 的 `BlobCipherPort` 只允许调用方提交 payload 与业务实体 AAD，不接收 `ActiveSpace`、key id、epoch 或 purpose。活动写入上下文和密文读取上下文属于具体密码 adapter；Application decorator 不构造占位 Space，也不能选择保护域。当前 production adapter 仍保持 V1/V2 wire compatibility，V3 writer 与 reader 只能在 profile storage upgrade gate 完整接线后 clean cutover，不得为提前启用某一类 payload 而在正常路径增加双 reader。

`ActiveSpaceSecuritySession` 是正常 Space security runtime 安装目标材料的唯一 Infra 边界。它串行执行归属验证、profile vault 耐久 catalog 安装、活动 session 切换和失败恢复；调用方不能分别决定两次写入的顺序。已取得完整 material 的路径保持 vault-first；从同一 MasterKey 加密 repository 恢复时，该模块在互斥区内临时装入目标密钥以读取 material，再验证、安装 vault 并完成 session，repository、vault 或 session 失败均恢复旧 snapshot。已耐久但尚未被活动状态引用的 catalog 作为安全的幂等准备结果保留。Engine 只构造一次 profile vault 并注入 Space access adapter；Legacy 无 material 恢复允许只切换 session，不生成虚假 catalog。成员加入、epoch/revocation、legacy bootstrap、Sponsor/Helper 准入和 membership branch recovery 的当前 material 推进也必须经该边界；repository 已提交后的安装失败由原恢复流程幂等重试，并以保留 source 的 `SecurityState` 稳定分类向上传播。临时 validator 只校验候选 material，不得写 vault；旧 CrossSpace transition 的 target session 由规格 033 后续 control-generation 切换整体删除，不在本切片机械改造。

V1/V2 到 V3 的转换只在软件升级时通过独立、原子、可恢复的 profile storage upgrade 执行一次。升级同时把本机历史/搜索/文件数据与 membership、credential、MLS 等 Space 控制面表拆入独立 generation。完成后，切换 Space 复用同一 profile SQLite/blob generation，只替换完整 Space control generation，不得扫描、复制或重加密历史业务负载。旧格式 reader 只能存在于升级模块，正常路径只写 V3。

`ProfileStorageUpgrade` 是该软件升级的唯一 Infra 协调边界；未来 Engine 只调用一次 `ensure_v3()`，不能编排逐表步骤。模块拥有 profile 级进程内串行化、跨进程非阻塞租约和使用 profile 稳定密钥 AEAD 保存的恢复 journal。Journal 显式绑定 V2 source manifest、数据库 revision 及唯一目标 profile data/control generation；`TargetStaged` 前通过 SQLite 一致性快照原子创建两个目标数据库，`StoresSeparated` 再按穷尽的 table ownership registry 清除非所属业务 rows、执行物理 purge 并绑定两个独立 digest。任何未声明表、target 损坏或升级期间 source 再写都失败关闭。Primary conversion 不原地改写 separated target，而是在临时目录中通过正式 V3 adapter 转换 inline/UCBL；随后专用字段转换从不可变 primary output 再构建完整候选，只编排 owner codec，并为搜索 render 写入 opaque group ref。旧单组 postings 不能反推词项，因此 target 清空 postings/tags 并以 blocked `search-v12` 等待 production rebuild。两个候选都必须经正式 V3 reader、row identity、搜索 gate、计数和数据库/blob-tree digest 验证后才 rename 发布；恢复校验不得借 production pool 切换 WAL 或改写 digest。两库当前保留完整技术 schema，production 验证、manifest promotion 与启动接线尚未完成，不能提前接入运行期。

`uc-core` 的 `ActiveRuntimeLayout` 只表达当前 Space、profile data generation 与 Space control generation 的合法组合，不拥有 keyslot、序列化、digest 或密码实现。V3 manifest 的技术格式和校验属于 `uc-infra`；在完整升级路径接线前，生产 store 仍只提升 V2，读取到经过完整校验的 V3 时明确失败关闭，不得把半完成格式激活为运行期。

允许明文保存的例外只有：

- 内容类型分类枚举；
- 文件内容本体；
- 入站文件在受管文件缓存中经安全清理的原始文件名，且只能作为实际缓存文件的 basename。

原始目录路径、数据库字段、搜索字段、日志和其他关联元数据不在例外范围内。

## 5. 网络协议

### 传输基础

默认网络由 Iroh 提供经过身份认证的加密 P2P 连接。业务通道使用不同 ALPN 隔离，防止消息被送入错误处理器。非文件业务内容还使用空间密钥派生的应用层 AEAD；文件内容沿用受校验的 blob 传输格式。

### 逻辑通道

共享 Iroh node 使用独立 ALPN 隔离主要能力：

| 能力 | 通道 |
| --- | --- |
| Space 准入 | `/uniclipboard/space-admission/1` |
| 剪贴板内容 | `uniclipboard/clipboard-applied/1` |
| 大内容与文件 | Iroh blobs |
| 活动剪贴板与按需拉取 | `active-clipboard` 系列 |
| 可达性 | `uniclipboard/presence/1` |
| 成员历史 | `uniclipboard/membership-history/3` |
| 成员安全更新 | `uniclipboard/group-update/1` |
| 传输进度 | `uniclipboard/transfer-progress/0` |

所有通道都必须在分配和业务解码前校验版本、消息类型、长度上限和认证身份。未知版本、错误通道、超限帧、非法身份或认证失败一律关闭式拒绝，不能降级到旧成功路径。

### 配对与准入

邀请签发、短码解析、OPAQUE 认证和成员加入都属于单一 Space 准入流程。邀请或握手成功本身不表示成员加入；只有已保存的正式成员历史、安全状态和激活结果能够开放普通能力。具体协议顺序见本章“单一 Space 准入协议”。

### 成员关系上线核对协议

已认证设备上线后通过 `/2` 交换完整版本化历史。接收方验证沿革、规范摘要、精确父事件中的发起者资格、
父历史保存公钥对应的签名、操作结果、安全状态绑定和连续事件链，不使用当前安全成员树替代过去授权。
发送方可以缺少接收方已经保存的其他成员决定；合并保留双方决定的并集，但发送方只能新增自己签署的决定，
不能携带另一成员尚未由接收方保存的决定。连续新增可自动应用；未由本机接受的移除只保存并等待用户，
不能越过它应用后继事件。

用户接受时应用收到的同一移除并回复签名决定；拒绝时保持本机成员集合，只把相关设备历史关系设为
分叉。分叉设备之间停止普通内容和旧 Space 成员历史传播，各自分支内部继续运行。拒绝不自动产生
移除发起者的事件。

#### 当前成员运行范围

普通设备列表、连接、内容发送、文件传输和恢复任务只能使用 Application 从已验证成员账本一次派生的当前成员范围。成员投影、可信关系、地址、在线状态或安全组记录都不能单独授予资格。

新增成员在正式历史、激活回执、本机激活和待执行影响全部完成前保持关闭；移除一经本机接受并保存便立即退出普通范围。资料不可读、身份矛盾、历史损坏或 Space 锁定时失败关闭，不回退旧成员表。

历史身份资料可以为了验证旧事件继续保存；向已移除设备送达指定决定只能经过受限计划，不能借此开放普通连接或内容能力。

### Space 成员状态产品边界

`QueryDeviceTrustUseCase` 从一次已验证 ledger 读取生成完整设备信任结果。没有当前 Space 时返回明确空状态；
Application 先确定可展示的设备范围，Infra 只为该范围补充成员投影中的显示名称和可达性缓存；这些观察资料不能反向授予成员资格。
Space 已锁定、V2 历史缺失、身份映射矛盾或观察资料不完整时失败关闭，不读取旧成员表补造结果。在线状态
只是显示观察，不参与成员资格或决定。

用户移除设备只提交目标设备；应用层签名并一次保存历史、关系、受限送达计划、Prepared 效果和修订号。
用户处理远端变化时只提交变化编号、接受或保留，以及移除本机时的明确二次确认。重复、过期和并发变化
返回稳定结果；正式事实一旦保存就不因网络或效果暂时失败而回滚。

`membership_ledger` 的加载和条件原子提交是 application 唯一成员持久边界。Infra 实现必须把整个负载按
MasterKey AEAD 加密，并在同一事务比较修订号和历史摘要；不得恢复旧成员仓储、旧准入仓储或第二份产品
修订。所有新加入、跨 Space 切换、分页接收、效果推进和关系确认都经过同一提交能力。

成员分支恢复由 `RecoverMembershipConflictUseCase` 完整负责。恢复包验证并建立 transition 后，维护轮次每次
只调用一次 generation transition 能力并提交一个直接后继阶段。Infra 在 `Promoted` 前先完成来源备份、
recipient MLS 与内容密钥解封验证、目标数据库快照、安全材料和目标成员投影；随后原子提升 active manifest，
重绑数据库、blob root 与内存安全会话，最后清理来源 generation。任何阶段失败都从加密 ledger checkpoint
重试；manifest 提升前不得修改活动数据库，提升后不得回退来源 generation。

旧 profile 不恢复成员资格。重建流程只保留允许的本机资料，清空旧关系和未完成成员工作，再建立新的
单设备 V2 根。任何旧成员表、可信关系、地址、在线状态或安全组资料都不能让旧设备重新进入普通范围。

### 成员候选资料核验

候选资料不等于成员资格。Application 只在 Space、身份、安全历史和直接证明全部一致后把候选用于正式流程；成员资格始终来自签名成员历史。

等待对端或安全资料属于可恢复状态；身份冲突、历史冲突和无效证明必须阻塞或拒绝。自动发现、资料补齐和重试由成员维护运行期负责，产品不保存队列或编排步骤。

### LAN 兼容协议

LAN HTTP 位于 `compatibility/`，使用独立的 `uc-mobile-v*` 版本和发布流程。它只在用户明确启用时运行，不能因为 P2P 不可达、平台进入后台或中继失败而自动开启。

## 6. 同步流程

### 本机内容产生与发送

```text
系统剪贴板变化或显式发送
  -> 检查空间已解锁和是否为自身回写
  -> 读取并规范化多个表示
  -> 计算跨设备内容摘要并去重
  -> 加密保存事件、条目、表示和资源
  -> 更新活动状态和本地搜索
  -> 按信任、在线状态和成员发送偏好筛选目标
  -> 逐目标加密发送并记录投递结果
```

空目标列表表示发送给所有符合条件的可信设备；非空列表只能缩小范围，不能绕过信任和偏好检查。发送结果区分已接受、重复、离线、失败和仍在等待，等待不能伪装成失败。

### 离线投递恢复

`ClipboardSyncRuntime` 是本机剪贴板投递的唯一负责人：本机复制、设备上线和用户手动重发都经由它进入既有发送或接收流程。同步总开关是所有出站、入站、文件投递、恢复和历史恢复广播的总许可；关闭时不做任何同步。总开关开启后，自动同步只控制本机复制的自动发送和离线自动补送；文件同步保持独立，手动重发只选择用户允许的设备且不受自动同步开关影响。自动恢复只选择这台刚上线设备的待送内容。某个既有设备暂时不可达时，现有投递记录保存该事实。对同一设备，新的本机复制会将更早的 `Unreachable` 记录标记为 `Superseded`，因此最多只有最新内容保留自动补送资格。该设备重新上线或应用重启后重新发现其在线时，运行期只发送这一条，发送结果继续由原有投递记录覆盖。

运行期不保存内存队列，也不为新设备生成历史候选。已接受、重复或被新内容替代的内容、远端来源的内容、明确失败或无法重新取得的内容都不会自动补发；内容无法取得时记录为明确失败，停止自动恢复。自动同步关闭时，本机复制不产生发送尝试，既有离线记录也不会自动补发；总同步关闭时，手动重发和入站内容同样被拒绝。一次发送仍不做无限循环重试，用户手动重发保留为独立动作。

### 远端内容接收

```text
收到加密帧
  -> 校验对端身份和消息边界
  -> 检查同步总开关
  -> 检查该成员的接收总开关
  -> 应用层解密并解析内容
  -> 检查内容类型接收偏好
  -> 去重或创建接收尝试
  -> 物化内联内容、blob 和文件集合
  -> 原子提交历史、资源、搜索和接收结果
  -> 按策略写入系统剪贴板并发出宿主事件
```

失败或取消必须清理临时资源，并保持已提交数据的一致性。重启后通过接收尝试、传输时间线和产物日志继续收敛或清理。

应用层只提供两种完整的远端内容处理模式。普通接收必须同时具备系统剪贴板写入、活动状态推进、重复内容重新激活、临时接收终结和进度回报；活动内容按需拉取只负责保存、文件落地、接收记录、搜索和可用性判断，不得提前写入系统剪贴板或推进活动状态。引擎层只能选择其中一种完整模式，不能逐项拼装或用空实现模拟缺失能力。

### 活动剪贴板收敛

活动状态使用最后写入获胜规则。排序键为 `(activated_at_ms, activated_by)`：时间较新者胜出，时间相同则设备编号字典序较大者胜出。

接收一个活动状态时按以下顺序处理：

1. 空间锁定时丢弃，不排队补做。
2. 与当前值相同或更旧时忽略，阻止循环广播。
3. 时间戳超出本机未来五分钟时拒绝，避免错误时钟长期压制真实更新。
4. 检查成员和内容类型接收偏好。
5. 本机已有完整内容时写入系统剪贴板；缺少内容时向报告者拉取、解密并持久化。
6. 只有系统剪贴板写入成功后才推进寄存器，并把同一激活继续广播。

核心不允许出现“寄存器已经前进，但系统剪贴板没有成功写入”的状态。

### 设备重新在线

设备重新可达后，Application 成员维护流程先运行成员历史核对；只有关系允许时才重新发送当前活动
状态。离线只表示暂时不可达，不删除成员、不撤销信任、不改变分叉关系，也不触发 LAN 自动回退。

成员历史按确定顺序分页传输，每页每类记录最多 256 条。接收方按来源设备加密保存已收到的连续页面；
断线或进程重启后，发送方可从第一页重新开始，接收方返回下一缺失页并幂等跳过相同页面。乱序页面只
请求缺失位置，混入另一轮资料或同一位置内容冲突时清除该轮暂存并拒绝。只有全部页面到齐、完整历史
重新验签且确认是发送方历史的合法延续后，才能一次提交正式成员历史并回复成功；提交时必须经过 Core
唯一的面向本机成员接收规则：普通新增推进本机 head，远端移除只保存已验证事件并保持父 head，等待本机
Accept/Reject。未完成页面永远不能改变当前成员资格。

新增成员事件同时签名绑定面向既有成员的群组 epoch 更新。离线旧成员合并多代历史后，Application 的
effect executor 按历史因果深度依次恢复成员事实与安全状态，不能使用事件哈希顺序。历史中仍有效但尚
未直接完成关系确认的成员可出现在 roster 中，通信、拨号和内容发送仍只允许 `usable` peer。

### 成员移除

成员移除作为签名成员历史的一个条目保存。发起设备立即应用自己的条目并停止向目标发送；其他设备上线后先验证和保存该条目，只有本机用户接受才应用。重新准入同一设备会产生新成员实例，不受旧实例移除结果影响。

移除事件在发起设备本机立即生效；其他设备上线后验证并保存该事件，但本机用户从未接受时不得修改
成员集合或安全状态。用户接受后应用同一事件；用户拒绝后保持本机分支，并把与发起设备的历史关系
设为分叉。拒绝不会自动移除发起者。已在另一分支被移除但尚未知情的成员仍可继续自己的分支，这是
明确接受的风险。

本分支移除应用后，Application 使用该分支的有效设备集合约束对外设备名单、实时名单更新、连接刷新和内容接收，不能从原始成员、地址记录或旧连接重新带回被移除设备。原始记录只保留用于成员历史核验；无需额外通知通道。

### 历史重发与恢复

- 重发只允许本机来源且内容仍可用的历史条目。
- 恢复系统剪贴板支持完整格式、纯文本和文件路径三种模式。
- 恢复成功后才更新最近使用状态并按既定规则广播。
- 内容已丢失、模式不适用、记录不存在和真实执行失败是不同结果。

### 切换空间

完成 V3 profile storage upgrade 后，已设置设备加入另一个 Space 只切换活动 Space control generation 与后续新写入的保护上下文。目标 catalog 必须先原子安装到 profile content key vault，active manifest 再复用同一 profile data generation 提升包含成员、凭据、MLS 与安全状态的完整目标控制面；切换不得建立来源最终数据快照、复制 profile SQLite/blob 或重加密历史业务负载。

旧历史继续按自身不可变 `ProtectionGroupId` 在本机读取，不自动加入目标 Space 的 outbox、重发或成员可见范围。用户明确再次分享时创建使用目标保护组的新事件，原记录与原密文不变。来源 Space 的在途发送、接收和目录发布在切换前结束、取消或隔离，不能改挂到目标 Space。

同一空间沿革的邀请必须在进入上述迁移前完成成员核对：相同事件幂等，连续新增只补齐，未确认
移除等待用户，不可比较历史标记相关设备分叉。任何同空间情况都不得准备跨 Space 数据备份或清理
历史 catalog；分叉本身不强制整个 Space 切换。

软件升级如果发现旧密钥无法读取历史，必须在 V3 manifest promotion 前整体失败关闭，保留旧 generation，不得自动删除、跳过、改写或把异常旧密文带入正常 V3 读取路径。普通 Space 切换不遍历历史，因此历史损坏不应被伪装成目标准入失败；读取时按稳定损坏/缺钥分类报告。

## 7. 错误处理

### 公共错误

公共错误只包含：

- 稳定编号；
- 错误类别；
- 是否建议重试。

错误编号只在对应操作的语境内解释，不保证跨操作全局唯一。宿主不能只看一个数字判断含义。

| 类别 | 含义 | 常见处理 |
| --- | --- | --- |
| `InvalidInput` | 输入、分页标记、句柄或内容类型无效 | 修正输入，不自动重试 |
| `InvalidState` | 当前生命周期、锁定或空间状态不允许操作 | 刷新状态后再决定 |
| `Unauthorized` | 口令、身份或宿主权限不通过 | 重新认证或请求权限 |
| `NotFound` | 邀请、成员、历史或资源不存在 | 刷新列表，不盲目重试 |
| `Conflict` | 当前事实与操作冲突 | 展示业务结果或重新查询 |
| `Unavailable` | 网络、索引、宿主能力或临时服务不可用 | 仅在标记可重试时重试 |
| `DeadlineExceeded` | 操作超时或因生命周期期限取消 | 允许用户重新发起 |
| `Internal` | 不能向宿主公开细节的内部失败 | 记录脱敏诊断并提示失败 |

### 业务结果与错误的边界

预期业务分支应返回明确结果，不应伪装成异常。例如：重复内容、对端离线、邀请已过期、没有活动剪贴板、恢复模式不适用、内容已经丢失和取消已太晚。

真正的基础设施失败才转换为公共错误。底层数据库错误、文件路径、网络库错误、密钥信息和用户内容不得进入公共结果或调试输出。

### 内部错误链

Application 可以把依赖失败转换为稳定类别，但转换不能删除原始错误。来自存储、网络、系统、密码能力或其他 Port 的失败必须作为 `source` 保留，并通过目标错误模块的 `From` 实现和 `?` 向上传播；只有需要改变语义类别或增加固定脱敏上下文时才使用 `map_err`。禁止把下层错误转为字符串、忽略原错误或用无来源枚举替代。纯业务判断本身没有下层异常时继续返回明确结果或普通枚举，不伪造错误链。Engine 最终只向宿主公开稳定编号、类别和重试建议，内部 source chain 与 backtrace 只用于受控诊断，且任何层都不得向其中加入敏感负载。

### 操作终态

每个已接受的操作都必须产生一次 `OperationFinished`：成功、失败或取消。生命周期截止时间到达时，未完成操作先被取消并收到终态事件，不能静默消失。

### 重试原则

- 只有明确标记为可重试的错误才允许自动重试。
- 重试必须有边界、退避和取消入口。
- 成员收敛、安全更新和后台恢复可以由负责模块持久化重试。
- 上线核对、缺失历史、待用户决定和分叉关系由 Application 成员维护流程持久化并跨重启继续；离线
  只暂停网络动作，待决定不能自动接受，分叉只隔离相关设备关系。
- 成员历史核对和用户决定由 Application 负责；产品端只展示完整结果，不安排重试或恢复流程。
- 用户提交的发送、恢复和导出操作在暂停后不自动重放，避免重复副作用。
- 数据损坏、身份冲突、版本不兼容和认证失败必须关闭式失败，不能降级绕过。

### 网络会话恢复

应用层的 `NetworkRecoveryFacade` 是网络会话恢复的唯一流程负责人。它只接收脱敏的本机网络恢复、此前在线设备路径已确认耗尽和新鲜拨号成功观察，并只向 Engine 请求“重建当前网络会话”这一完整动作；它不持有 Endpoint、Router 或第三方网络错误。Engine 的 `SessionSupervisor` 是该动作的唯一执行者，负责关闭操作门、等待或取消旧操作、停止完整旧会话、终结活动传输、创建并安装完整新会话；构建失败时保持操作门关闭。

自动恢复必须同时满足当前会话代次的本机换网窗口仍有效、故障设备此前在线、基础设施已经完成受限确认且同一失败周期尚未重建。普通设备离线、冷启动首次连通、过期观察和旧代观察都不能触发重建。任意新鲜拨号成功会清除该失败周期。

基础设施只向 Engine 传递“本机中转恢复”“此前在线设备确认路径耗尽”“新鲜拨号成功”三种脱敏事实。严格窗口内的路径确认使用两轮各最多两秒的拨号；第一轮失败后全局至多一次地重置解析状态并提示当前会话重新检查，第二轮仍失败才上报路径耗尽。LAN 兼容模式不创建该观察，也不得自动切换到 LAN。

自动和手动请求共享同一轮恢复，任何时刻只允许一个重建动作。临时失败按固定的 `1s`、`2s`、`5s`、`10s`、`30s` 间隔重试，连同首次共最多六次；全部失败后状态明确为可重试失败，关闭后取消等待且不得重新启动。Engine 后续只负责安全替换完整运行会话，并把稳定状态和结果转换给产品及绑定层。

### 日志与隐私

日志不得包含剪贴板内容、密码、密钥、完整令牌、文件名、文件路径、设备备注或可恢复这些内容的派生值。需要排障时记录稳定编号、阶段、计数、耗时和脱敏身份。

跨层业务链路的持续性能观测归 Engine 组装层所有。`crates/uc-engine/src/assembly/observability/` 按业务领域保存实现 Application port 的具体 decorator；Application 继续只编排流程，不接触 `Instant`、tracing target 或观测字段。每个领域通过一个主要装配入口集中选择 observation policy 并包装真实能力；port 返回的后续能力也由同一领域继续包装，例如准入 transport 返回的 authenticated exchange。

该范式只复用“在组装边界装饰 port”的结构，不建立跨领域万能观测框架。每个领域分别拥有固定操作枚举、明确降噪策略和稳定事件 schema；禁止 `Observed<T>`、通用 phase 字符串注册表，以及要求业务调用方传入开始时间、成功布尔值或可选字段的记录函数。Decorator 不得改变业务结果、错误 source、重试或调用顺序，字段仍服从本节隐私边界。

成员资料交接日志记录排队、发送开始、接收确认和重试四个阶段；每条只包含脱敏目标身份、资料数量、批次数、单批大小、单批上限、尝试次数和稳定失败类别。重试还记录下一次尝试时间。设备名、地址原文、安全资料和它们的摘要都不得写入日志。

移动绑定在系统日志（OSLog / logcat）之外叠加按天滚动的文件层，写入宿主 cache 目录的 `logs/` 子目录，文件名为 `engine.YYYY-MM-DD.txt`，只接收 `info` 及以上级别；系统日志层不加过滤。日志目录由宿主能力提供，创建失败时降级为仅系统层，不影响启动。

连接刷新成功（每次拨号或恢复）后，核心记录仍在线且经中继连接的对端本次最终选择的中继地址；每条只包含稳定设备标识与中继地址，快照查询失败只记录脱敏失败类别，不改变刷新结果。中继地址是用户配置的连接端点，不包含访问令牌。

### 剪贴板同步诊断

同步阶段可以写入脱敏结构化延迟日志，用于区分本机准备、连接、传输、接收处理和远端提交耗时。日志不得包含内容、设备名、文件名、路径或可还原它们的字段；未知网络路径必须明确标记未知。

Engine 不初始化或依赖外部遥测发送器。宿主自行决定是否保留或转发诊断日志，逐阶段延迟不进入产品分析事件。

## 8. 状态机

### Engine 生命周期

```text
Running -> Quiescing -> Quiesced -> Suspended -> Running
   |           |           |           |
   +-----------+-----------+-----------+-> ShuttingDown -> Stopped
```

- `Running`：唯一接受新操作的状态。
- `Quiescing`：停止接收新操作，等待在途操作。
- `Quiesced`：在途操作已结束或已取消。
- `Suspended`：节点、会话任务和连接已释放，但实例与事件流保留。
- `ShuttingDown`：关闭全部运行资源。
- `Stopped`：事件流关闭，实例不可恢复使用。

### 配对邀请

```text
Pending -> Consumed
   |----> Revoked
   +----> Expired
```

每次邀请只有一个 Sponsor 生成的 256-bit 内部身份，同时表现为可手输的短码和可用于二维码、链接或直接文本的完整邀请。完整邀请以版本化编码携带内部身份、Sponsor 不透明地址和有效期，不携带口令、私钥或 Space 内容。完整邀请可在本地解析；短码只是云端或局域网中的查询别名，查询结果必须返回同一份完整邀请。Sponsor holder 以内部身份和短码共同指向同一邀请，任一入口消费、过期或撤销都使另一入口同时失效。两种邀请原文均不进入 Debug 或日志。

云端在短码首次查询时立即废弃别名，不等待配对完成。Joiner 因此先密文保存 Ready，再原子提交 Started 并从记录中删除短码，之后只调用一次解析。成功响应必须先替换为 Resolved 完整邀请，之后才可连接 Sponsor。如果响应丢失、超时、完整邀请保存失败或进程从 Started 重启，本次加入稳定结束并要求 Sponsor 签发新邀请，不得再次查询原短码。完整长邀请不经过该状态，本地验证并保存后可按同一地址重试连接。

邀请只允许使用一次。撤销和过期都是终态，不能重新激活。

### 内容物化

```text
Inline

Staged -> Processing -> BlobReady
             |-------> Failed
             +-------> Lost
```

失败可以由明确流程重试；永久丢失只能作为稳定结果展示，不能伪造空内容。

### 接收尝试

```text
Receiving -> Committing -> Completed
    |            |
    |            +-> Failing -> Failed
    +-> Cancelling -> Cancelled
    +-> Failing ----> Failed
```

每轮接收都有独立尝试编号。取消、提交和失败处理通过原子认领决定唯一胜者，过期请求返回“已被新尝试取代”或“已经太晚”。取消权取得后，下载线程与用户取消入口可以先后完成同一个“已取消”收尾；相同尝试、无新增产物的重复取消收尾视为成功，不得把已取消误报为失败。不同尝试或其他终态仍按冲突处理。

### 文件传输时间线

```text
Started -> Progress* -> Completed
                    \-> Failed
                    \-> Cancelled
```

`Started` 只能出现一次；终态后不能追加进度或第二个终态。文件进度是可重复事件，不是独立传输身份。

接收方通过应用层的文件传输会话完成整段流程。创建会话时会在同一个入口登记接收上下文并写入开始状态；同一传输编号在进程内只对应一个活动会话。进度、完成、失败和取消都由该会话串行处理，重复提交同一种终态不会重复写入，不同终态并发时只有一个可以成功。

Blob 拉取和移动流式上传都持有或复用同一个会话，不再分别调用开始、进度和结束步骤。正常暂停会在网络停止后取消仍活动的会话，最终关闭会停止接受新会话；跨重启遗留仍由已有的启动恢复处理。`AppFacade` 不公开文件传输内部对象或逐步状态入口。

移动流式上传由移动同步应用入口内部的单一负责人管理。它生成并登记对外不可解析的上传句柄，持有临时写入、字节统计、进度节流和文件传输会话，并统一处理追加、完成、取消和关闭。追加、进度或完成失败会在同一处清理临时写入并记录失败；显式取消和关闭会记录取消。关闭会等待已经开始的操作退出，再终结其余上传。引擎只转换四个稳定用户动作的输入、结果和错误，不保存上传表或临时文件对象。

### 历史维护运行期

历史维护由应用层历史功能内部的单一运行期负责。启动时立即执行一轮，之后按固定间隔复用同一条核对、文件清理、保留策略流程。核对失败会跳过本轮两个删除步骤，文件清理失败仍继续保留策略，任一单轮失败都不终止后续定时维护。运行期只记录不含内容和路径的汇总结果；关闭会立即结束定时等待，并等待已经开始的一轮完成。引擎只启动和关闭该运行期，不掌握维护步骤、间隔、失败分流或日志。

### 应用总入口

活动运行期只接收完整构造的 `AppFacade`。Application 内部对象保持私有，Engine 只能调用面向用户动作的顶层方法，不能取得内部用例后重新编排流程。

Space、成员维护、接收、搜索、恢复和同步能力必须在唯一装配点完整提供。需要网络 endpoint 的 Space application 先 dormant 构造，Router ready 后再启动后台任务；运行中不补装能力或保留半成品 facade。

### 成员候选收敛

```text
Pending -> WaitingForPeer / WaitingForUpdate -> Verifying -> Ready
    |                 |                         |
    +-----------------+-------------------------+-> Blocked / Rejected
```

`Ready` 才表示候选具备完整、连续且可验证的安全历史。等待对端和等待更新是可恢复状态；身份冲突、安全历史冲突和无效证明必须阻塞或拒绝。

### 工作空间成员变化状态（ADR-020）

```text
reconciliation: Idle -> Comparing -> FetchingHistory -> Consistent -> Idle
                              |              |
                              |              +-> PendingRemovalDecision
                              +-----------------> Diverged / Invalid
```

每个设备同时公开三个独立维度：可达性 `Online/Offline`，本分支成员资格 `Active/Removed`，以及
双方历史关系 `Unknown/Consistent/PendingRemovalDecision/Diverged/Invalid`。
待决定移除可以让已知事件头领先已应用事件头；后继事件不能越过它应用。接受后可回到
`Consistent`；拒绝后进入 `Diverged`，只隔离相关设备关系，不停止整个 Space。

本机发起移除时，Application 在同一个加密 membership effect payload 中保存已签名移除事件和当时的保留接收者集合；可重启 effect executor 是成员事实、可靠 MLS revocation、激活三阶段的唯一推进者。Infra 只在该显式本机发起 payload 上创建撤销并推进 epoch，远端收到普通移除事件时不得重复发起撤销。统一 group-update 维护入口聚合 Space 普通欠账与 revocation stage 欠账，投递成功后直接确认原撤销事务，不复制确认状态。可靠 group update 到达前，保留成员继续因旧 epoch 关闭式拒绝正文；到达后其 epoch 必须与发起分支精确一致。已移除设备不再要求当前成员 observation 存在，Application 将缺省观测投影为 `Offline`；Active 设备缺少观测仍返回失败。

成员分支恢复的 Iroh 服务端写完响应并 `finish()` 后，必须等待对端确认完整接收再结束 handler，避免较大的 GroupInfo 或恢复包随连接生命周期被截断。Application 的冲突恢复用例负责完整 transition：单轮内按顺序推进所有可成功执行的阶段，每一步先独立持久化；只有依赖暂不可用时才交回后台重试，固定维护周期不参与内部流程编排。

摘要发现双方处于同一 lineage 的 sibling 分支时，普通后缀反熵不会尝试合并或应用远端历史。Application 改用一次有界的双向冲突取证往返：双方分别发送完整分页签名历史，只在本机验证分支关系和证据发送者，不把远端历史应用到当前分支；每一端都在同一次加密 ledger CAS 中写入唯一冲突记录并把对应 peer 标记为 `Diverged`。取证完成后普通反熵隔离该 peer，用户只能通过统一设备组选择入口推进恢复。Core 负责历史验证和 sibling 规则，Application 的 ledger 事务负责完整结果，Iroh 只传输追加在既有判别值之后的 typed message。

### 空间切换与重置

跨 Space 加入先准备并验证包含成员、凭据、MLS、安全与恢复状态的独立目标 Space control generation，原子提升 active manifest 时复用当前 profile data generation；替换前失败继续使用旧 Space，替换后只能恢复并完成同一目标控制世代。Reset 必须按自身数据保留语义单独设计，不能借 CrossSpace 恢复已删除的 payload rewrap。旧 control generation 清理由可重试后台工作负责，不参与授权判断；历史 content key catalog 在没有引用证明时不得自动清理。

## 9. 跨平台差异

平台差异只能影响宿主能力、运行时限制和语言边界，不能改变空间、身份、配对、加密、协议和数据语义。

| 平台 | 接入方式 | 主要差异 |
| --- | --- | --- |
| macOS / Windows / Linux | 直接依赖 `uc-engine` | 宿主实现系统剪贴板、目录、安全存储和文件句柄；Linux/Wayland 可能提供一次性快照 |
| iOS | UniFFI + XCFramework + Swift 绑定 | Keychain、App Group、后台限制；主应用、分享扩展和键盘扩展各有短生命周期实例 |
| Android | UniFFI + AAR + Kotlin 绑定 | 启动前安装 application context；Keystore；`content://` 句柄；后台和剪贴板访问受系统限制 |
| HarmonyOS | N-API + HAR + ArkTS 声明 | 通过线程安全回调提供宿主能力；64 位文件偏移和大小以十进制字符串跨边界 |

### 共同要求

- 所有平台使用同一个 `v*` Release 中的同版本 Engine 和绑定。
- 移动端进入后台调用 `suspend`，回到前台调用 `resume`，退出调用 `shutdown`。
- 进程被系统终止后重新 `start`，不能复用旧内存实例。
- 事件必须持续消费；收到 `RefreshRequired` 后重新查询真实状态。
- iOS、Android 和 HarmonyOS 必须为成员移除公开相同的完整状态、稳定错误、提交入口、当前查询和变化事件。
- 产品只通过 Engine 的 `QueryDeviceGroupChoices` 与 `ChooseDeviceGroup` 处理设备组选择。Application `AppFacade` 是完整流程的唯一协调入口：它把待定成员变更与 sibling branch 冲突投影为同一批选择，并以 ledger revision 拒绝过期操作，再路由至内部决定或分支恢复用例。查询同时保留完整设备信任快照；远端分支成员在恢复前不可证明时以 `members_complete = false` 表达，不伪造名单。iOS/Android 共用 UniFFI 薄映射，HarmonyOS 使用同版本 N-API JSON 映射；绑定不解析问题类型或编排恢复步骤。
- iOS 和 Android 的绑定将一次新增、修改或删除中继节点及其访问令牌交给 Engine 的原子设置操作；绑定先读取当前节点列表并只合并该次变更，令牌只经宿主安全存储使用，绑定不得将其持久化或返回给产品界面。
- 文件必须通过不透明句柄分块读写，不能把路径伪装成句柄。
- 平台限制不构成自动切换 LAN 的理由。

### 移动端产品分析边界

iOS 和 Android 绑定提供两个启动方式：原启动方式不启用产品分析；宿主明确选择带产品分析的启动方式后，才向核心提供事件发送、分析身份保存能力和一份启动时固定的平台信息。核心负责事件名称、脱敏属性、固定平台字段和身份切换顺序，移动宿主负责用户许可、供应商发送、失败重试，以及匿名身份和空间成员身份的持久保存。

公共观测约定定义 `$os`、`os`、`os_version`、`$device_type`、`arch` 和 `app_channel` 六个固定字段。移动端和桌面端都使用同一份定义；宿主在启动时提供实际值，发送层将它们加到每条事件，业务事件不能覆盖。移动宿主收到事件后应快速放入自己的发送队列，不能让第三方网络请求阻塞核心流程。事件属性只允许来自核心定义的脱敏字段，不得追加剪贴板正文、设备名、密码、密钥、令牌、文件名或路径。事件发送失败只影响观测；身份保存失败必须明确返回失败，由核心现有流程决定本次业务操作是否继续。Engine 和绑定不依赖 PostHog、Sentry 或任何 OTLP 发送器，具体供应商由产品宿主选择；剪贴板逐阶段延迟仅作为本地结构化诊断日志保留，不进入产品分析事件。

### iOS 多进程约束

主应用、分享扩展和键盘扩展不能共享内存实例。它们可以通过 App Group 目录和 Keychain Access Group 访问同一份受保护状态，但同一时刻只应有一个进程持有 P2P 运行会话。扩展启动时应根据安全存储可访问性决定是否允许恢复解锁。

### 剪贴板监听差异

变化监听是可选能力。宿主若已经从系统回调取得完整快照，下一次核心读取必须优先消费这份快照，避免 Wayland 等一次性数据源在二次读取时丢失。核心暂停或锁定期间的变化不排队补发。

## 10. 开发规范

### 修改前

1. 先确定唯一负责完整结果的模块。
2. 写清调用方唯一需要做什么、成功和失败返回什么、重启或重试由谁负责。
3. 判断是否新增持久化字段、文件、日志字段、网络消息或公共契约。
4. 新增持久化内容默认按敏感数据处理；明文例外必须有明确批准。
5. 外部能力只能通过 `uc-engine` 提供，不能让产品仓依赖内部 crate。

### 修改中

- 项目文档和代码注释使用中文；代码标识符和提交信息使用英文。
- 保持单一事实来源，不长期保留新旧两套实现。
- 生产代码禁止 `unwrap()`、`expect()`、`println!()` 和 `eprintln!()`。
- 日志和 `Debug` 输出必须脱敏。
- 文档中的仓库路径使用相对路径。
- Rust 命令从仓库根目录运行。
- 业务流程不能由宿主按内部步骤逐一调用。
- 绑定只能做转换，不能承载业务规则。
- P2P 失败不能自动进入 LAN 兼容线。

### 架构文档同步规则

任何 Agent 修改仓库内容时，必须在同一次交付中检查并更新本文：

1. 修改目标、分层、模块职责时，更新第 1 至 3 节。
2. 修改持久化、加密、搜索或迁移时，更新第 4 节。
3. 修改消息、通道、握手、兼容策略时，更新第 5 节。
4. 修改业务顺序、后台恢复或重试时，更新第 6 节。
5. 修改错误、结果或日志规则时，更新第 7 节。
6. 修改状态或转换规则时，更新第 8 节。
7. 修改绑定、宿主能力、最低平台或系统限制时，更新第 9 节。
8. 修改维护和交付规则时，更新第 10 节。
9. 即使确认没有架构语义变化，也必须在下方维护记录中增加一行，写明修改范围和“无架构变化”。

不能只更新维护记录而漏掉受影响正文，也不能为了让文档通过检查而写入未经代码验证的描述。

### 交付检查

不涉及行为改动时，至少运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

涉及行为改动时，在此基础上运行相关单元、集成和端到端测试。涉及发布时还必须运行：

```bash
node scripts/release/verify-release-bundle.mjs <产物目录>
```

未执行的设备验收只能记为“跳过”，不能记为“通过”。

### 评审检查表

- 负责完整结果的模块是否唯一？
- 调用方是否只需一个主要入口和少量明确结果？
- 敏感数据是否在所有成功、失败、迁移和恢复路径中保持密文？
- 公共错误、事件和状态是否稳定且不泄露内部细节？
- 重试、取消、暂停、恢复和关闭是否有唯一所有者？
- 平台差异是否停留在宿主和绑定边界？
- 删除负责模块后，复杂度是否会重新散落到多个调用方？
- 本文正文和维护记录是否已同步？

## 文档维护记录

本节只记录会改变长期架构理解的修订，不记录单个测试、重命名、格式化或中间切片。实现过程应进入提交历史、规格实施清单或任务记录。

| 日期 | 主题 | 长期结论 |
| --- | --- | --- |
| 2026-09-02 | 定时维护审查 Action | GitHub Actions 每周并行启动五个互不调用的全仓审查 session，再由独立 session 汇总标准 JSON artifact；开发者通过 Actions Summary 阅读去重报告，并可下载各 lane 与汇总产物追溯证据。审查只报告问题，不自动修复、建 Issue 或改变运行时架构。 |
| 2026-09-01 | 文档记录系统重组 | 根与局部 `AGENTS.md` 收敛为短维护地图；长期设计、ADR、产品规格、active/completed 执行计划、生成资料和参考资料分别进入结构化 `docs/` 目录。根 `ARCHITECTURE.md` 成为当前架构入口，架构圣经继续保存详细事实；旧 `docs/adr`、`docs/specs`、`docs/prd`、`docs/diagrams` 和 `docs/migration` 路径不再作为并行入口。 |
| 2026-09-01 | 并行维护审查 Skills | 仓库维护审查拆为五个互不调用的只读专项 skill，由独立 session 并行运行；第六个独立 skill 只汇总标准化 artifact。该机制以现有架构、安全、可靠性和设计文档为规则来源，不改变运行时架构。 |
| 2026-08-29 | 文档压缩 | 圣经只保留当前架构事实、稳定边界和维护规则；移除逐切片流水账与已经失效的实现细节。 |
| 2026-08-29 | Space 成员关系与准入 | Application 是成员关系、设备信任和单一 Space 准入完整流程的唯一负责人；Infra 提供加密存储、密码能力和 Iroh 认证传输，Engine 只组装并控制生命周期。 |
| 2026-08-29 | 设备信任观察适配 | 增加只读 Infra 适配器，为 Application 已验证的设备范围补充成员名称和可达性；依赖失败保留 source chain，不改变成员资格来源。 |
| 2026-08-29 | 成员事实效果 | 成员账本先持久化待执行效果，Infra 再幂等更新本机投影；新增成员更新 roster、可信身份和地址，移除成员先撤销信任，roster 与地址只为认证受限决定暂留并在交付完成后清理。失败保留 source 并由维护运行期重试。 |
| 2026-08-29 | 成员安全效果 | Infra 只应用已绑定到签名成员事件的非空安全更新；决定负载不伪造安全更新，依赖失败保留 source 并等待恢复。 |
| 2026-08-29 | 成员效果激活 | Application 仅在事实和安全阶段完成后开放最终成员 scope；Infra 在移除激活前清除旧可达性连接与缓存。 |
| 2026-08-29 | 受限成员投递 | 同一认证成员历史通道可只携带一个签名事件或决定；接收端验证传输身份与历史作者一致后才合并，不向受限对端发送完整历史。 |
| 2026-08-29 | 移除投影清理 | 被移除成员的 roster 和地址只保留到受限交付计划清空；维护流程随后幂等删除，不能作为普通成员授权来源。 |
| 2026-08-29 | 成员维护网络门禁 | Space 锁定或切换时先阻止新的准入恢复、历史同步和受限投递，再等待当前维护轮次结束；恢复后才允许新请求。 |
| 2026-08-30 | 新准入恢复收口 | `SpaceApplication` 只使用类型化准入 aggregate 与认证 transport 恢复，不再依赖旧 admission outbox delivery。 |
| 2026-08-30 | 当前成员范围出口 | `SpaceFacade` 只向运行期暴露成员账本推导的最终可通信范围，不暴露账本验证、维护或效果推进步骤。 |
| 2026-08-30 | Space activity 启动门禁 | Space 生命周期活动未完成运行期绑定时明确返回 unavailable；不得静默跳过搜索、接收或成员维护的暂停与恢复。 |
| 2026-08-30 | 准入设备名 | Joiner 创建签名准入事实时读取 start-join 已持久化的当前设备名，不使用 daemon 启动期快照。 |
| 2026-08-30 | 准入传输地址编码 | Iroh adapter 负责生成成员投影可保存的认证 endpoint 地址编码，Engine 不复制基础设施序列化格式。 |
| 2026-08-30 | 短码解析端口 | Pairing transport 对 Engine 明确暴露新准入的短码解析能力；Engine 不把旧 session port 冒充新协议端口。 |
| 2026-08-30 | 可达性刷新 | 主动刷新只遍历成员账本开放的当前 peer scope；名单外设备不得因诊断操作被拨号。 |
| 2026-08-30 | Engine 新 Space 装配 | Engine 构造 dormant `SpaceFacade`，先安装认证 admission/history handler，再启动 Router 和成员维护；剪贴板、活跃剪贴板与 roster 共用账本推导的当前成员范围。 |
| 2026-08-30 | 旧收敛依赖删除 | Engine 不再构造或携带 candidate、announcement、outbox、applied-security-update 与 legacy migration recovery 等旧 convergence 运行期依赖。 |
| 2026-08-30 | OPAQUE 存储作用域 | OPAQUE credential 不再要求首次建 Space 预先存在 generation manifest；新布局绑定完整 generation，legacy 布局绑定成员账本 lineage。 |
| 2026-08-30 | 旧事件接线删除 | 删除只服务旧 workspace convergence 广播的 Engine 事件转发器及测试，设备信任变化由新 Space application 出口负责。 |
| 2026-08-30 | 旧准入恢复类型删除 | 删除无生产调用者的 completion-recovery wire 类型和未持久化的 completed join 投影；认证准入恢复只保留新 aggregate 协议。 |
| 2026-08-30 | 旧 outbox 恢复用例删除 | 删除未被 `SpaceApplication` 构造的 legacy admission outbox recovery；成员维护只调用新 aggregate recovery service。 |
| 2026-08-30 | 旧入站 handler 删除 | 删除未装配的 legacy admission message handler 及其 ledger commit seam；认证 Iroh handler 只进入新 aggregate endpoint。 |
| 2026-08-30 | 准入取消构造 | Joiner aggregate 根据当前阶段、已保存前驱证据和 continuation route 构造取消交换；Application 不拼装协议序号或 route。 |
| 2026-08-30 | 准入取消入口收口 | `SpaceAdmissionProtocol` 在 profile 级串行约束内完成当前 JoinId 读取、领域取消、条件提交和维护唤醒；Facade 不再调用 legacy ledger 取消用例，Infra 只提供密文状态原子提交和随机消息材料。 |
| 2026-08-30 | 准入状态与激活收口 | 当前加入状态、待完成查询和最终激活统一由 `SpaceAdmissionProtocol` 读取类型化准入状态并推进；Facade 不再编排 legacy transition 用例。 |
| 2026-08-30 | 成员账本去除准入状态 | membership ledger 只保存成员事实与效果，删除 `admission_records`、`admission_profile` 和旧 admission outbox；准入状态只由独立 MasterKey AEAD 仓库保存。 |
| 2026-08-30 | 旧准入领域模型删除 | 删除 Core `space_join_record`；准入拒绝使用新协议枚举，待投递成员安全更新由 Application 的类型化 preparation 结果表达。 |
| 2026-08-30 | 准入交付验收启动 | 新准入代码切换完成后分别建立自动化、真实基础设施、实体设备和发布证据；未执行的实体设备项目只记录为跳过。 |
| 2026-08-30 | 准入 Engine 生命周期验收 | Engine E2E 已改用正式邀请与加入入口；`SessionSupervisor` 已唯一负责后台准入完成后的 session transition。新设备加入和已有设备换 Space 已贯通，Sponsor/Joiner 成员账本最终激活与重启传输仍在收口，尚不可交付。 |
| 2026-08-30 | 准入成员激活持久化 | Sponsor Complete 在提交协议回复前通过单一激活端口幂等安装安全状态、成员事实与正式成员历史；Joiner 目标 generation 在提升前写入加密 membership ledger。Engine 每次安装 session 都尝试静默恢复，普通冷启动允许保持锁定，准入切换后要求恢复成功。新设备加入、已有设备切换和重启传输 E2E 均已通过。 |
| 2026-08-30 | 准入激活分层 | Application/Core 负责验证成员历史并产出类型化激活计划；Infra 只按已验证计划安装目标安全状态、关系投影和加密成员账本，不重复解释或验证协议历史。 |
| 2026-08-30 | 准入 clean-cutover | 生产运行期只保留 invitation discovery；准入消息只使用 `/uniclipboard/space-admission/1`。旧 pairing ALPN、session/event port、wire 与兼容探测已删除并由架构检查禁止回归；实体设备矩阵仍明确跳过。 |
| 2026-08-30 | Desktop CLI 准入验收 | Desktop 通过本地 `uc-engine` 启动真实 `uniclip`/`uniclipd` 多 profile 后，Joiner 完成准入及冷重启仍保持 session locked；Engine 内存宿主通过不能替代 Desktop secure-storage 与 generation 切换证据，Desktop 接入暂不可交付。 |
| 2026-08-30 | 短码准入恢复闭环 | Application 持久化短码解析结果后立即唤醒维护轮次，再以加密保存的启动上下文和完整邀请重建初始交换，并保留原 AdmissionId/JoinId 推进至 `Initiated`；Iroh transport 统一负责准入路由解码。Desktop 在 generation 切换删除旧准入记录后，以本机成员已激活作为等待命令的成功依据。 |
| 2026-08-30 | 成员历史反熵规格 | 新增规格 029，确认成员传播必须由 Application 单一反熵负责人基于逐 peer 认证水位和加密持久欠账完成；易失 wake、固定单轮预算和关系状态不能代替 ACK、重试、公平调度及多跳 fan-out。当前为待实施设计，生产语义尚未切换。 |
| 2026-08-30 | 成员历史反熵主路径 | Core 提供摘要关系规划、连续后缀和精确 ACK 规则；Application `MembershipHistoryAntiEntropy` 统一负责入站、出站、持久水位、退避、公平游标和有界并发；Iroh 只传输 V3 typed message。旧 V2 全量历史不再进入运行时 wire。Desktop C1 三节点历史收敛和双向正文传输已通过。 |
| 2026-08-30 | 成员历史反熵诊断 | 反熵关键路径新增仅含阶段、稳定分类和计数的 debug 观测，不记录设备、lineage、transfer、digest、地址或错误文本。Desktop C1 证据显示第二代 Sponsor 的成员投影包含新成员，但正式 membership ledger 未提交相应历史，因此反熵摘要仍判定 `noop`；后续修复归 Sponsor Complete/Settled 激活边界。 |
| 2026-08-30 | 未知成员历史证明 | V3 摘要携带发送者准入声明，Iroh 连接只绑定公钥指纹；本机尚未知的发送者只能提交有界后缀，完整签名历史验证通过后才成为当前成员。该边界消除“必须先认识新成员才能接收其准入历史”的循环依赖，Desktop C2 在 Sponsor 离线及双方重启后通过三成员收敛和双向正文传输。 |
| 2026-08-30 | Group Epoch 持久投递 | Application `DeliverPendingGroupUpdatesUseCase` 唯一负责扫描加密安全材料中的欠账、有界调用 Iroh dispatch，并且只在认证 Accepted ACK 后确认删除；失败欠账持久轮转以防多设备饿饿。Engine 只安装 ALPN handler 和注入 port，临时根因日志已删除。 |
| 2026-08-30 | Group Epoch 多跳恢复验证 | 增加旧成员视图尚不认识转运节点时的真实 Iroh 回归边界，验证恢复更新的授权必须来自更新自身的密码学连续性，不能依赖接收端已经拥有待恢复后的成员视图。 |
| 2026-08-30 | Admission 重启诊断覆盖 | 为存储 generation 选择、会话安全材料恢复、Sponsor 安全与成员 ledger 提交、runtime Space transition 重装增加长期结构化 tracing；仅记录阶段、epoch、revision 和数量，不记录身份、路径或业务负载。 |
| 2026-08-30 | 四节点离线历史收敛 | Sponsor 把既有成员群组更新绑定进签名新增事件；effect executor 按历史因果深度恢复成员事实与安全状态。Roster 展示有效但关系待确认的成员，通信门仍失败关闭。Desktop A-B-C-D 中间节点离线场景已通过成员名单、重启和 A/D 双向正文验证。 |
| 2026-08-30 | 五节点树型历史收敛 | Desktop A→B、A→C、B→D、C→E 验证不同 Sponsor 支路保持同一单父历史；中间节点离线时 D/E 可先收敛五成员与安全状态并双向传输，全部恢复后五端名单一致。 |
| 2026-08-30 | 成员分叉选择规格 | 新增规格 030：分叉历史禁止直接合并或自动选主；用户选择一个完整目标分支，Application 通过可恢复 generation transition 切换本机。目标分支已移除本机时必须重新配对。复杂拓扑采用确定性通信矩阵、阶段故障与可重放 chaos seed 验收。当前仅完成设计，生产入口尚未实现。 |
| 2026-08-30 | 成员分叉稳定标识规则 | Core `MembershipConflictPolicy` 从两条已验证且共享激活基线的 sibling 历史生成与到达顺序、transport peer 无关的 conflict/branch id，并只按目标完整历史中的同一成员实例状态返回可恢复或需要重新配对；Same、ancestor 与缺席实例均不可选择。 |
| 2026-08-30 | 成员冲突加密账本 | 冲突记录、多个证据来源、不可变用户选择和 transition id 进入现有 MembershipLedger 整体 MasterKey AEAD 载荷，与 peer `Diverged` 关系共用同一 revision/CAS 提交；诊断只暴露阶段和计数。 |
| 2026-08-30 | 成员冲突选择入口 | Application `ResolveMembershipConflictUseCase` 串行并以 ledger CAS 保存一次用户选择；保留本机分支直接完成，目标已移除本机时只返回重新配对，远端可恢复分支保存 Pending 等待后台 transition。重复相同选择幂等，相反或竞争选择返回 `StateChanged`。 |
| 2026-08-30 | 成员分支切换状态机 | Core `MembershipBranchTransitionV1` 固定七个只前进的持久阶段并绑定 conflict、目标分支及不同 source/target generation；远端 Active 选择以 conflict/target 摘要产生唯一 transition id，随加密 ledger 保存，重复调用不创建第二个 intent。 |
| 2026-08-30 | 成员分支恢复包验证 | Core 恢复包验证重新解码并验签目标完整历史，重算目标 branch，并要求 recipient 与授权者均为目标 Active 成员；包精确绑定 conflict、branch、recipient、expiry 和 nonce，验证失败不返回 MLS 或内容密钥材料。nonce 消费状态由后续 Application ledger CAS 持久管理。 |
| 2026-08-30 | 成员冲突恢复协调 | Application coordinator 是恢复包获取、完整验证和无副作用 `Prepared` transition 计划的唯一流程入口；接受包时在一次加密 ledger CAS 中同时消费 nonce、保存 transition 并推进 conflict。重复执行不再获取包，跨 conflict nonce 重放不产生持久化副作用。 |
| 2026-08-30 | 成员冲突维护接线 | membership maintenance 在 effects 恢复后、group update 和受限交付前统一驱动 conflict recovery，且 peer 上线也会触发；损坏状态阻断后续权限扩展。真实 Iroh recovery 与 generation adapter 接入前，Engine 明确保持 Pending，不借普通反熵或 LAN 兼容线降级。 |
| 2026-08-31 | 分支切换准备 adapter | Infra 从 MasterKey AEAD 保护的 active generation manifest 读取真实 source database generation，并为目标分支生成独立随机 generation；该 adapter 只返回 `Prepared` 计划，不创建目录、不覆盖来源、不提升 manifest。Engine 不再使用 transition deferred adapter。 |
| 2026-08-31 | 分支恢复包签发入口 | Application issuer 从加密 ledger 重新验证目标是本机当前完整分支、认证请求设备对应 Active recipient、签发者仍为 Active；Infra 只能提供已对 recipient 密封的 MLS 与内容密钥材料。issuer 生成短期 nonce 包并使用当前成员凭据授权签名，认证或资格失败时不调用材料能力。 |
| 2026-08-31 | MLS 分支恢复密码学边界 | Active recipient 不接收目标设备的 MLS 私有 snapshot；目标端只导出带 ratchet tree 的签名 GroupInfo，recipient 使用自身既有签名私钥发起 external commit 并替换相同凭据旧 leaf。目标端应用 commit 后双方才共享新 epoch exporter wrapping key，内容密钥目录必须在该阶段密封，因此恢复传输采用认证的两阶段握手；新增恢复错误保留底层 source，同时以脱敏 `Debug` 隐藏协议内部信息。 |
| 2026-08-31 | 分支恢复认证服务端 | recovery ALPN 复用进程唯一 Iroh endpoint，并把连接公钥映射为已知 source device；GroupInfo begin 与 external commit submit 都独立进入 Application issuer 重新验证目标分支和 Active recipient。未知连接、畸形帧、错误方向或资格失败统一拒绝，handler 不持有跨往返授权状态。 |
| 2026-08-31 | 分支恢复 Iroh wire contract | 专用 P2P ALPN 使用有界两阶段帧交换 GroupInfo、recipient external commit 和最终恢复包；两个请求阶段都绑定 conflict、target branch 与 recipient，连接身份仍须由 Application 对目标历史复核。帧和错误 Debug 不输出绑定标识或密码学负载，解码失败保留 source；该协议不允许降级到 LAN 兼容线。 |
| 2026-08-31 | 分支恢复事务持久化 | Application 将 recipient/target 两侧的 staged 密码学状态、external commit 摘要和幂等恢复包收进单一恢复 session 状态机，并随 membership ledger 整体 MasterKey AEAD 加密。session 以 transition id 建索引、绑定 conflict/branch/recipient，只允许单调且幂等推进；Space 重建原子清除未完成事务。 |
| 2026-08-31 | 分支恢复客户端信道 | Infra `IrohMembershipBranchRecoveryChannel` 只向指定认证 peer 执行 GroupInfo 请求和 external commit 提交，负责地址解析、有界帧、超时及稳定错误分类；它不选择 peer、不解释 MLS、不读取恢复 ledger，也不推进 generation。完整两阶段流程及重启续跑仍唯一归 Application coordinator。 |
| 2026-08-31 | 分支恢复客户端编排 | Application conflict recovery coordinator 唯一负责选择确定性 evidence peer、请求 GroupInfo、调用无副作用 recipient MLS preparation、在 external commit 发出前加密提交 staged session、验证并保存恢复包，再原子消费 nonce 和建立 generation transition。重启从已保存阶段续跑，不重新生成 external commit；旧一步式 fetch port 已删除。Engine 已接入真实 Iroh channel，recipient MLS adapter 接入前保持显式 Deferred。 |
| 2026-08-31 | Recipient MLS 恢复 adapter | 现有 `DefaultSpaceAccessAdapter` 实现窄 recipient recovery port：从当前 generation 的加密安全仓库加载 MLS state，使用目标 GroupInfo 生成 external commit，并把 recipient MLS snapshot、共享 wrapping key 与 epoch 编码为单一 staged payload，交由 Application 在发送 commit 前随 ledger 加密保存。Engine 不再注入 recipient deferred adapter。Target 侧禁止直接 apply-and-reply，必须先实现 TargetPrepared/TargetCommitted 持久事务。 |
| 2026-08-31 | 分支恢复事务键 | Core `MembershipBranchTransitionV1::derive_id` 是 conflict 与目标 branch 推导 transition id 的唯一规则；选择方与目标方无需共享额外随机事务标识即可得到相同加密 session 键。Application 不再复制哈希域与字段顺序。 |
| 2026-08-31 | Application 依赖表面审计 | 扫描 `deps.rs` 的 port 定义、实现、装配、生产调用、测试与历史后，确认小 port 原则本身仍成立，复杂度主要由 Application 对象图外移到 Engine、宽 bundle 分发及步骤级编排造成；形成规格 031 的 clean-cutover 收敛计划，并识别旧 admission/pairing、receive projection 与 wiring-only 字段的删除候选。本轮只更新计划，无生产架构行为变化。 |
| 2026-08-31 | Space transition 内部重构规划 | 规格 032 将超大 Infra transition 文件规划为稳定私有门面、普通准入、目标工作区、generation store、密文重包、Reset、成员分支和耐久文件能力；公开构造、四个 Application port、active manifest 唯一生效点及持久密文格式均保持不变。本轮只形成规划，无生产架构行为变化。 |
| 2026-08-31 | Target 恢复提交事务 | Application issuer 将 target material 能力拆成无副作用 prepare 与幂等 commit：先签署 package 并连同 external commit digest、staged security material 保存为 TargetPrepared，随后提交安全状态并标记 TargetCommitted。重试按同一事务键和 commit digest 返回缓存 package；TargetPrepared 重启只续做 commit，不重复计算或签发。 |
| 2026-08-31 | Target 恢复材料 adapter | `DefaultSpaceAccessAdapter` 唯一负责在当前 MLS 快照上应用 recipient external commit、派生下一 epoch 内容密钥、以共享 exporter wrapping key 密封目录与恢复确认，并以无副作用 prepare/幂等 commit 暴露给 Application。staged `SpaceKeyMaterial` 只进入现有 MasterKey AEAD recovery session；commit 重新验证 Space、MLS、目录和单步 epoch 后才持久化安装。Engine 已删除 target deferred adapter，只负责注入该窄端口。 |
| 2026-08-31 | 成员分支 generation 切换 | Application recovery coordinator 继续负责七阶段 transition，每轮只推进并 CAS 保存一个后继；Infra 复用 durable Space transition 组件完成来源备份、recipient 材料解封、目标 SQLite/安全状态/成员投影 staging、manifest 提升、运行期重绑与旧 generation 清理。真实 MLS + SQLite 测试验证目标 manifest、历史和内容密钥 epoch 同时切换，Phase 4 完成。 |
| 2026-08-31 | 统一设备组选择入口 | 将待定成员变更与 sibling branch 冲突合并为 `QueryDeviceGroupChoices` / `ChooseDeviceGroup`；Application 负责一致 revision 校验和内部路由，Engine、UniFFI、HarmonyOS 与移动 probe 删除四个旧操作入口并只保留薄映射。 |
| 2026-08-31 | Phase 6 拓扑验收驱动器 | Engine 集成测试新增声明式 `Start/Create/Join/AssertSnapshot` tracer bullet；驱动器仅通过稳定 `Engine::execute(Operation)` 推进和观察多节点，不读取内部 ledger。本轮没有生产架构语义变化。 |
| 2026-08-31 | Phase 6 成员诊断接缝 | `dev-tools` 增加只读成员诊断 operation，由 Application 一次性返回 branch/head、group epoch、有效成员数及待处理 conflict/effect/transition 阶段；Engine 只做脱敏映射。该入口不进入默认构建和移动绑定，声明式拓扑测试用它验证密码学与恢复状态，不直接读取内部 ledger。 |
| 2026-08-31 | Phase 6 受控 P2P 分区 | 共享 Iroh endpoint 可选安装按认证 EndpointId 工作的测试 gate；它在握手前拒绝新出站连接、握手后拒绝双向连接，并在分区建立时主动关闭已存在连接，因此所有业务 ALPN 使用同一 Partition/Heal 边界。控制入口仅存在于 Engine `dev-tools`，生产组装不安装 gate，诊断输出不暴露 EndpointId。 |
| 2026-08-31 | 成员冲突跨端 contract | Engine 新增完整冲突查询与单次选择两个稳定 operation，统一映射 result/error；iOS、Android 和 HarmonyOS 绑定只转发同版本 Engine contract，并明确结果仅代表本机选择完成。 |
| 2026-08-31 | F0 sibling 冲突发现 | Desktop 五节点确定性拓扑验证共同 head 分区后双 Sponsor 形成两个四成员 sibling 分支；Heal 后 Application 通过一次双向完整签名证据往返让双方各自原子保存唯一冲突并进入 `Diverged`，不应用远端分支、不自动选主，分支间正文继续关闭式失败。 |
| 2026-08-31 | F1 移除与新增 sibling | 本机移除通过可重启 membership effect 调用可靠 MLS revocation，保存保留接收者并推进安全 epoch；Removed 设备缺少当前 observation 时稳定投影为 Offline。Desktop 五节点验证移除/新增 sibling 各自成员语义、epoch、分支内通信和 Heal 后隔离。 |
| 2026-08-31 | F2 双移除验收起点 | Desktop 声明式拓扑增加统一 `ResolveConflict` 动作，只调用 Engine 设备组选择入口；五节点红测从共同 head 并发移除不同叶子，并要求 chooser 的 branch、head、成员数与安全 epoch 精确切换到用户所选分支。 |
| 2026-08-31 | F2 分支恢复闭环 | 修复恢复响应在 handler 结束时被截断，并由 Application 单轮连续推进逐阶段持久化的 generation transition；五节点双移除后明确选择目标分支，最终 branch、head、成员视图与 MLS epoch 精确一致。 |
| 2026-08-31 | F3 相反移除决定验收起点 | Desktop 声明式拓扑新增统一 `Decide` 与复用同一宿主持久化状态的 `Restart`；红测只经 Engine contract 验证同一远端移除被接受/拒绝后的分支、epoch、重启持久性与 exact-text 隔离。 |
| 2026-08-31 | F3 重启恢复诊断 | Session supervisor 在静默恢复失败并保留锁定运行时的决策点记录脱敏 `error_kind`；统一设备组查询以 debug 分类 device-trust 子状态、membership-conflict 与并发变化，使重启 unavailable 可诊断且避免可重试轮询刷屏。日志不包含 Space、设备、路径、正文或密钥。 |
| 2026-08-31 | F3 相反决定传播与投影 | Membership maintenance 在非 PeerOnline 完整轮次先尝试 restricted event/decision，再推进成员 effect；Deferred 不阻塞离线移除，Corrupt 仍关闭式停止。设备信任投影将 Removed 设备映射为不可同步，不再要求其存在于 active peer scope。 |
| 2026-08-31 | F3 restricted event 根因诊断 | 分层脱敏 tracing 排除地址、Iroh、身份、handler 与 ACK 链路，确认 restricted handler 绕过普通 merge 的本机待决定 head 规则；临时探针已清理，后续由 Core 单一远端事件接收入口统一该语义。 |
| 2026-08-31 | F3 相反移除决定闭环 | Core 以唯一“面向本机成员接收远端事件”入口统一完整历史 merge、分页 suffix 与 restricted event：远端移除保存证据但保持父 head，新增正常推进，重复与 sibling 保持既有结果。分页同时使用 sender projection 验证远端目标位置，不能拿本机待决定位置冒充远端声明。Desktop F3 已通过 Accept/Reject、重启持久化和跨分支正文隔离。 |
| 2026-08-31 | F4 单 bridge sibling 隔离 | Desktop 声明式拓扑新增只开放两个认证端点、其余跨区链路保持阻断的 `Bridge` 动作。六节点共同基线分裂为两个各三成员的 sibling history 后，唯一 bridge 只交换冲突证据：两端各记录一个冲突，成员集合与 branch 不变，跨分支正文继续关闭式拒绝，不能联合成伪历史。 |
| 2026-08-31 | F5 环形冲突幂等传播 | Application ledger 对已记录的同来源 conflict evidence 返回现有响应而不重复提交；Desktop 六节点从共同历史形成 E/F 两条 sibling 分支，再把共同成员 A-B-C-D 接成单环。冲突沿 B-C 与 D-A 两个方向传播后，每端只公开一个设备组选择，重复刷新不增加 membership effects。peer 重试账务 revision 可独立推进，不作为 conflict 消息环判据。 |
| 2026-08-31 | F6 深链离线 Sponsor 恢复 | Desktop 从 A→B→C→D→E→F 深链形成两条七成员 sibling，真实停止 B/D 后由 F 选择 E 分支。Target 恢复 prepare 将 external commit 作为持久 group-update 欠账扇出给其他 Active 目标成员；TargetCommitted 在同一加密 ledger 流程中完成 conflict、恢复 recipient 关系，并使旧 sibling evidence 幂等。A/C/E/F 最终 branch、head、MLS epoch 与相邻正文均收敛，恢复不依赖原 Sponsor 在线。 |
| 2026-08-31 | F7 三分支公平反熵 | Desktop 十节点从链式七成员基线并发形成三条八成员 sibling；分组分区只保留组内连接，并在 A–B 单冲突边存在时让落后合法 peer D 重连。D 仍在有界窗口内补齐 A/G/H 分支的 branch、head 与 MLS epoch，证明冲突 peer 不饿死合法反熵。十节点完整有向正文矩阵同时验证分支内通信与跨分支关闭式隔离。 |
| 2026-09-01 | 双设备配对性能观测 | Engine 在 `assembly/observability/admission.rs` 通过 `ObservedAdmissionPorts` 集中装饰恢复状态、认证建链与消息交换、Sponsor 状态、Joiner Candidate、Joiner activation 和 Space session transition port；Application 调用点不接触时钟、日志 target 或观测字段。各 decorator 使用类型化操作与显式 policy，抑制成功空恢复/激活 load，日志不包含邀请、设备、地址、凭据或密钥。Engine `dev-tools` 的一秒热路径门禁继续只从公开 operation 与成员诊断观察完成。 |
| 2026-09-01 | Engine port decorator 观测范式 | 持续跨层观测统一归 `crates/uc-engine/src/assembly/observability/<domain>.rs`（规模增长后可拆同名子目录）：具体 decorator 实现 Application port，领域装配入口集中选择 policy，返回 port 的能力继续包装。禁止跨领域万能 `Observed<T>`、字符串 phase 注册表及业务调用点手工计时；该范式可扩展到剪贴板、成员和其他领域而不共享业务事件 schema。 |
| 2026-09-01 | 配对性能日志语言统一 | `admission.performance` decorator 与性能验收日志使用英文消息和固定结构化字段；本轮不改变准入流程、持久化语义或生产超时。 |
| 2026-09-01 | 033 活动 generation 第一切片 | Core 新增 `ActiveRuntimeLayout`，只固定当前 Space、profile data generation 与 Space control generation 的合法组合；Infra 新增 V3 manifest 的规范 digest、领域映射和只读版本识别。生产 promotion 与运行路径仍保持 V2，合法 V3 在完整升级接线前以不支持版本失败关闭；未修改内容密码 port，也未改变 CrossSpace 行为。 |
| 2026-09-01 | 033 profile content key vault 第二切片 | Infra 新增自有目录的 `ProfileContentKeyVault` 深模块，以独立 secure-storage key 整体 AEAD 保存多个历史保护组目录，完整安装负责规范合并、全 profile key identity 冲突拒绝和原子替换，精确解析不依赖当前 Space。session 与 vault 共用单一 V2 content-key catalog codec；缺钥、未知 framing、篡改均失败关闭，Factory Reset 同时擦除独立 key。当前未接入 production session、V3 manifest promotion 或 CrossSpace。 |
| 2026-09-01 | 033 V3 内容保护第三切片 | Infra 新增未接 production 的 `ContentProtection` 深模块，构造时固定 purpose，并集中拥有 purpose HKDF、规范 AAD、严格 V3 envelope、活动写入和 vault 历史解析。切换活动 Space 后旧密文不读取当前 session，仍按 key identity 打开；session 只新增当前保护组写入 seam，既有 V2 reader 与历史 catalog 暂不删除。 |
| 2026-09-01 | 033 活动安全会话第四切片 | Infra 新增 `ActiveSpaceSecuritySession` 深模块，统一负责目标 material 的归属验证、profile vault 先行耐久安装、session 后切换及失败恢复；Fresh group join 与普通恢复共用入口，Vault 失败保持旧 session，稳定 `SecurityState` 分类保留下层 source。Engine 组装 profile 级唯一 vault；本轮不启用 V3 payload、不删除 V2 reader 历史 catalog，也不机械改造旧 CrossSpace。 |
| 2026-09-01 | 033 当前 material 推进第五切片 | `ActiveSpaceSecuritySession` 新增完整 `install_current_material` 操作，将成员加入、epoch/revocation、legacy bootstrap、Sponsor/Helper 准入和 membership branch recovery 的真实安全状态统一收口为 vault-first、session-second 安装；加密 repository 恢复也收口为带临时密钥访问和完整 snapshot 回滚的单一操作。临时 validator 与待删旧 CrossSpace target session 不写 vault；新增安装错误转换保留稳定 `SecurityState` 分类和完整 source。本轮不启用 production V3 payload 或 V3 manifest。 |
| 2026-09-01 | 033 持久密码 interface 第六切片 | `BlobCipherPort` 删除 encrypt/decrypt 的 `ActiveSpace` 参数，调用方只提供 payload 与业务实体 AAD；四类 inline decorator、旧 migration recovery 和待删 CrossSpace 不再伪造占位 Space。当前 V1/V2 adapter、UCBL、搜索和专用 repository 格式保持不变，不启用 V3 writer 或 production 双 reader。 |
| 2026-09-01 | 033 升级协调第七切片 | Infra 新增 `ProfileStorageUpgrade::ensure_v3()` 单一协调入口，隐藏进程内串行化、跨进程非阻塞租约、source identity 校验和 profile AEAD 加密 journal。V2 与空 profile 首次调用耐久生成唯一目标 data/control generations，重启复用相同字节；锁竞争返回 `Busy`，source 变化、journal 篡改或缺钥失败关闭。本轮不接 Engine、不转换 payload、不提升 V3 manifest。 |
| 2026-09-01 | 033 V3 payload 格式第八切片 | `ContentProtection` 的 V3 envelope 从 JSON 数字数组收紧为紧凑二进制格式，并新增共享该深模块的 V3 inline adapter 与 UCBL store；UCBL 只负责 zstd 与外层 framing。跨 Space 测试证明历史读取不依赖当前 session，AAD transplant、截断、篡改和未知格式失败关闭，错误保留 source。本轮不接 production、不引入双 reader；专用字段在全量升级与 clean cutover 时保留领域序列化/AAD 所有权并统一委托该 envelope。 |
| 2026-09-01 | 033 多保护组搜索第九切片 | Infra 新增未接 production 的 `V3SearchProtection` 深模块，从 profile content vault 稳定 key 域分离生成搜索根，索引结果携带 opaque group ref 与组隔离 tags；查询只为索引实际组生成按词分组的 alternatives，保持正确 AND 语义。搜索 render 的 schema/AAD 留在搜索模块，V3 AEAD 与历史解析委托 `ContentProtection`。重启、Space 切换、未知 ref 与明文探针契约通过；production v11 schema/port/装配留待升级 target 与 clean cutover 同时替换。 |
| 2026-09-01 | 033 升级 target staging 第十切片 | `ProfileStorageUpgrade::ensure_v3()` 在 Detected 后独占 SQLite 一致性 snapshot 与 data/control target 物理布局；两个目标均落盘并验证后才耐久进入 `TargetStaged`，journal 同时绑定 source database revision。恢复期间 target 缺失/篡改或 source 再写均失败关闭，V2 source/manifest 保持不变。此切片不把两份同源 snapshot 冒充最终分库，也未转换 V3 payload 或接入启动。 |
| 2026-09-01 | 033 store ownership 第十一切片 | 升级器新增 `StoresSeparated` 耐久阶段和覆盖全部 production SQLite 表的唯一 owner registry；未知/重复/缺失表失败关闭。profile target 物理清除 control rows，control target 物理清除 profile data 与 coordination rows，两个独立 digest 和不变 source revision 共同约束恢复。完整技术 schema 暂留两库，业务 row 已唯一归属；V3 密文转换与 production 路由尚未接入。 |
| 2026-09-01 | 033 primary payload conversion 第十二切片 | 升级器新增 `PrimaryPayloadsConverted` 耐久阶段：separated target 保持只读，inline 与 UCBL 在独立临时目录中经正式 V1/V2 reader 打开并委托 `ContentProtection` 写成 V3；完整 production-reader 回读、row identity 和数据库/blob-tree digest 验证后才以目录 rename 发布。恢复校验使用不改写 WAL 的直接 SQLite 连接。专用字段、搜索与 production promotion 尚未完成。 |
| 2026-09-01 | 033 专用字段 V3 codec 第十三切片 | active register、file-set path、file transfer、directory publish 与 receive artifact 的所属模块分别保留私有序列化、路径编码和实体 AAD，并新增只委托共享 `ContentProtection` 的 V3 codec；升级器不拥有这些业务格式，也没有万能字段 rewrapper。跨字段 contract 验证 round trip 与 AAD transplant 失败。当前仅准备转换能力，production repository 与升级 journal 尚未切换。 |
| 2026-09-01 | 033 全量 payload conversion 第十四切片 | 升级器新增最终 `PayloadsConverted` 耐久阶段，从不可变 primary output 构建第二个原子候选，只编排 owner legacy/V3 codec 转换 file-set、transfer、active register、directory publish、receive artifact 与 search render。搜索 target 写入 opaque group ref；不可逆的旧 postings/tags 被删除并以 blocked `search-v12` 等待 production rebuild。正式 V3 回读、搜索 gate、计数及数据库/blob-tree digest 全部通过后才发布，重启验证复用；production 与 manifest 仍未切换。 |
| 2026-09-02 | 033 runtime generation 验证第十五切片 | 升级器新增独立 `Verified` 耐久阶段，按最终 profile/control 路由重新打开双库，执行 SQLite integrity、foreign-key、业务 row 唯一归属与 source revision 检查，并把两份规范 schema fingerprint 写入加密 journal。重启会重新验证并比对 fingerprint；promotion 只能从该完整布局证明继续，production 与 manifest 仍未切换。 |
| 2026-09-02 | 033 V3 manifest promotion capability 第十六切片 | Active manifest store 新增 V2 source compare-and-promote 完整操作与显式 V3 loader：写锁内重新认证当前 manifest，仅精确 source 可原子提升，同一 target 幂等恢复，后来状态返回 `SourceChanged`；升级不得改变 Space/keyslot identity。既有 V2 loader 对 V3 仍失败关闭，升级器与 Engine 暂未调用，production 尚未切换。 |
| 2026-08-30 | 目标 Space OPAQUE 凭据 | Joiner 在 Candidate 阶段由本次加入口令预生成目标 OPAQUE 服务端凭据，凭据随加密 transition 计划保存，并在目标 generation 提升前与 manifest 绑定安装。因此新成员重启后可成为下一代 Sponsor，无需从 source Space 复制凭据。 |
| 2026-08-30 | 首次 Space generation 激活 | 当前版本首次初始化在成员、安全状态和 ledger 建立后，通过单一持久化激活入口整体提升 generation，发布 active manifest 后记录 Engine 版本基线；不再写入 legacy current-space identity。旧资料升级仍执行独立化 rebuild 并要求重新配对。 |
| 2026-08-29 | 安全持久化 | 成员账本、准入状态和 OPAQUE credential 均使用 MasterKey AEAD 加密保存，并绑定当前 Space generation。 |
| 2026-08-29 | 网络与运行期 | P2P 使用共享 Iroh node；Space application 先以 dormant 状态构造，认证 handler 和 Router ready 后才启动后台恢复。 |
| 2026-08-29 | 双邀请入口 | 短码和完整邀请指向同一随机邀请身份；完整邀请携带 Space admission 路由，不携带旧配对会话协议。 |

## 相关文档

- `AGENTS.md`：仓库不可破坏规则和交付要求。
- `ARCHITECTURE.md`：当前架构的根级入口与模块地图。
- `docs/references/domain-glossary.md`：统一领域词表。
- `docs/PRODUCT_SENSE.md`：长期目标和边界。
- `docs/README.md`：文档记录系统总索引。
- `docs/design-docs/index.md`：长期设计、稳定契约与 ADR 索引。
- `docs/PLANS.md`：active/completed 执行计划与技术债入口。
- `docs/SECURITY.md`：工程安全知识入口。
- `docs/RELIABILITY.md`：恢复、后台任务与验证层次。
- `docs/design-docs/uc-engine-interface.md`：稳定操作、结果、事件和宿主能力。
- `docs/design-docs/ports.md`：内部能力接口和边界。
- `docs/security/encrypted-persistence.md`：密文持久化规则。
- `docs/security/release-integrity.md`：发布来源和校验规则。
- `docs/design-docs/decisions/011-reliable-member-revocation.md`：可靠成员移除和密钥世代。
- `docs/design-docs/decisions/015-offline-first-member-removal.md`：已由 ADR-020 取代的成员移除决策记录。
- `docs/design-docs/decisions/016-workspace-wide-convergence.md`：已由 ADR-020 取代的工作空间收敛决策记录。
- `docs/design-docs/decisions/017-pairing-as-workspace-admission.md`：配对作为工作空间内部准入通道的责任边界。
- `docs/design-docs/decisions/018-domain-oriented-application-layout.md`：应用层按业务领域收口的所有权和目录边界。
- `docs/design-docs/decisions/019-device-specific-convergence-waiting-status.md`：已由 ADR-020 取代的等待设备状态记录。
- `docs/design-docs/decisions/020-membership-reconciliation-and-user-decisions.md`：设备上线成员核对、未确认移除决定和分叉关系隔离规则。
- `docs/design-docs/decisions/021-workspace-convergence-internal-boundaries.md`：已由 ADR-025 取代的旧渐进整理决定。
- `docs/design-docs/decisions/022-user-initiated-join-supersession.md`：用户明确加入、后台恢复和旧加入安全取代规则。
- `docs/design-docs/decisions/023-legacy-profile-isolation-and-re-pairing.md`：旧资料升级后本机独立化和全部重新配对规则。
- `docs/design-docs/decisions/024-reset-space-as-device-management-reset.md`：用户明确重置全部设备关系并建立单设备空间的决策。
- `docs/design-docs/decisions/025-application-space-membership-one-shot-rewrite.md`：停止渐进迁移并一次性替换旧成员关系的决策。
- `docs/exec-plans/completed/015-offline-first-member-removal.md`：已由 ADR-020 取代的成员移除说明记录。
- `docs/exec-plans/completed/016-workspace-wide-convergence.md`：已由 ADR-020 取代的工作空间收敛说明记录。
- `docs/product-specs/021-device-trust-reconciliation.md`：设备信任完整查询、决定和产品动作边界。
- `docs/design-docs/current-member-runtime-scope.md`：当前成员运行范围、历史身份与普通授权的一致性规则。
- `docs/exec-plans/completed/023-durable-membership-proof-and-admission-activation.md`：历史验证材料、准入正式提交、激活门禁、恢复和旧数据迁移规则。
- `docs/exec-plans/completed/024-workspace-convergence-internal-boundaries.md`：已由规格 027 取代的旧渐进实施记录。
- `docs/exec-plans/completed/025-user-initiated-join-supersession.md`：用户再次明确加入时安全取代旧本机加入的分阶段实施规格。
- `docs/exec-plans/completed/026-legacy-profile-isolation-and-re-pairing.md`：旧资料独立化、关系清理和产品提醒的实施规格。
- `docs/exec-plans/completed/027-application-space-membership-one-shot-rewrite.md`：Application Space 成员关系目标对象、接口、流程、删除清单和验收标准。
- `docs/exec-plans/completed/028-single-space-admission-protocol.md`：全新单一 Space 准入协议、跨层接入、删除清单和完整验收标准。
- `docs/exec-plans/active/029-durable-membership-history-anti-entropy.md`：逐 peer 确认水位、持久传播欠账、公平重试和复杂拓扑验收。
- `docs/exec-plans/active/030-membership-conflict-resolution-and-chaos-validation.md`：成员分叉选择、恢复与确定性复杂拓扑验收。
- `docs/exec-plans/active/033-immutable-content-protection-context.md`：不可变保护上下文、profile 历史 content key vault、一次性 V3 密文升级及无历史重包 CrossSpace 方案。
