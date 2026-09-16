# Profile 内容密钥的运行期复用设计

状态：实现完成；验证结果及已知失败见文末。日期：2026-09-09。研究源码：`b5de286ef5d9e02e1831e776eaac939fddb40da9`。
本轮已实现 Infra 目录复用与 Engine 生命周期接线；真实 GUI 功能仍在本次范围之外。用户补充的硬要求：未来 GUI 锁定期间，后台仍须接收同步内容并完成密文持久化。

## 1. Overview

V3 搜索结果补全对每条结果调用 `ContentProtection::open`，每次按身份解析都重新读取、解密和校验整个 profile vault，并访问系统安全存储。同一份已验证密钥材料的读取成本被重复支付。

本机 Linux ARM64 开发 daemon 的两轮只读 HTTP 复现如下。所有请求均为 HTTP 200、ready；调查脚本的一秒阈值两轮均失败。

| 场景 | 返回条数 | 第一次 | 第二次 |
| --- | ---: | ---: | ---: |
| 浏览 limit=20 | 20 | 1.713 秒 | 1.737 秒 |
| 浏览 limit=100 | 67 | 5.033 秒 | 4.498 秒 |
| 无匹配 | 0 | 0.205 秒 | 0.224 秒 |

运行 daemon 的旧 debug 二进制已经被磁盘上的构建替换，无法证明其对应上述源码提交；这些数字只证明运行实例的症状，不作为固定版本性能基线，也没有分离钥匙串、磁盘与 AEAD 的耗时。

设计选择：深化现有 `ProfileContentKeyVault`，由它持有受后台安全运行期约束的有界已验证目录；所有 V3 内容读取和搜索根能力共享该实现。GUI 锁定属于交互授权，不能撤销后台密码能力。

## 2. Goals

- 后台安全运行期有效且目录已加载时，V3 `resolve` 和 `search_catalog` 不产生 vault 文件读取或 `SecureStoragePort::get`；条数增长只增加必要的查找、派生与内容解密。
- 同一冷目录的并发读取合并加载；成功加载只读一次外层密钥，同时得到目录与搜索派生根，不再为搜索额外读钥匙串。
- 保留所有已安装保护组及历史 epoch，按密文身份解释历史内容，活动 Space 只决定新写入与网络使用的材料。
- GUI 锁定后网络接收、内容解密验证、密文持久化和所需索引更新继续；GUI 解锁不触发安全会话重建或密钥库重新加载。
- 真正安全会话锁定、运行期关闭、Factory Reset 撤销复用并清除所持材料；并发加载、失败回滚和任务取消不能重新发布已撤销的快照。
- 保留现有持久格式、容量约束、AEAD 完整性和下层 source chain。

## 3. Non-Goals

- 本次不实现 GUI 锁屏界面、口令/生物认证、宿主 IPC 授权或新的 Engine 公开 GUI 接口；本设计给这些未来功能留下明确约束。
- 不改搜索分页、防抖、排序、历史归属和跨设备权限，不靠提高解密并发掩盖 I/O。
- 不重做 MLS、传输密钥、活动 Space catalog，不把它们与 profile 历史目录合并。
- 不引入 TTL、LRU、全进程全局缓存、明文磁盘缓存或密钥回收策略。
- 不承诺本轮解决全部搜索耗时，也不承诺 GUI 锁定等于进程内不存在密钥。

## 4. Current Architecture Context

| Module / 路径 | 当前职责与关系 |
| --- | --- |
| [sqlite_index.rs](../../../crates/uc-infra/src/search/sqlite_index.rs) | `hydrate_results` 逐条 await `open_render`；它应继续只负责结果补全，不管理密钥会话。 |
| [v3_protection.rs](../../../crates/uc-infra/src/search/v3_protection.rs) | 生成查询/索引 token 和解密 render；多处消费 `search_catalog`。 |
| [content_protection](../../../crates/uc-infra/src/security/content_protection/mod.rs) | V3 持久业务负载唯一密码入口；写取活动 session，读取 vault 历史身份，固定 purpose。 |
| [profile_content_key_vault](../../../crates/uc-infra/src/security/profile_content_key_vault/mod.rs) | 安装完整已验证材料、冲突校验、持久化和身份解析；当前只有写锁，没有内存目录。 |
| [persistence.rs](../../../crates/uc-infra/src/security/profile_content_key_vault/persistence.rs) | 文件读取、系统安全存储、AEAD、原子替换与父目录同步。 |
| [session.rs](../../../crates/uc-infra/src/space/security/session.rs) | 活动 Space MasterKey、当前/历史活动组材料；`clear` 清空，快照支持恢复，`detached_clone` 用于隔离工作。 |
| [active_space_security_session](../../../crates/uc-infra/src/space/security/active_space_security_session/mod.rs) | 已存在的完整安装负责人：vault 持久安装成功后更新 session；repository 恢复期间临时装载目标 MasterKey，失败恢复旧状态。 |
| [lock_space_session](../../../crates/uc-application/src/space/lifecycle/lock_space_session/use_case.rs) | 当前真正的安全会话锁定：先暂停活动，再调用 lock；它不适合未来 GUI 锁定。 |
| [RuntimeSpaceAccessAdapter](../../../crates/uc-infra/src/space/security/access.rs) | 实现初始化、解锁、静默恢复等完整能力；当前 lock 直接调用 session.clear。 |
| [运行期装配](../../../crates/uc-engine/src/assembly/wire/mod.rs) | 创建一个 profile vault 并注入相关 adapter；Engine 只负责构造和生命周期接线。 |

长期语义来自 [033](../completed/033-immutable-content-protection-context.md) 和[密文持久化](../../security/encrypted-persistence.md)：profile 历史目录独立于活动 Space。另有[升级验证测试](../../../crates/uc-infra/src/security/profile_storage_upgrade/derived_payloads.rs)明确在 `session.clear()` 后检验 V3 输出。因此不能把所有历史读取统一改成 `is_ready()` 检查。

## 5. Proposed Design

### Components：唯一负责人及分层

- **历史材料的唯一长期内存所有者：`ProfileContentKeyVault`。** 它隐藏加载、校验、查找、复用、安装一致性和清理。删除它会把这些复杂度重新分散到搜索、inline、blob 和派生持久化调用方。
- **活动安全状态的完整负责人：`ActiveSpaceSecuritySession`。** 沿用它的安装和恢复入口，使成功激活后开始复用；真正锁定时撤销复用。调用方仍只提交完整材料、恢复或锁定，不手动执行加载/刷新缓存步骤。
- **`InMemorySession` 只关联不透明的复用许可。** 不装入 profile 历史 key map，不增加历史查询 Interface。`clear` 能同步撤销许可，即使绕过上层正常路径也不能留下长期目录。
- **Application 保留流程与重启恢复责任。** 新缓存不是授权真相，不决定成员、Space、UI 可见性或重试策略。
- **Engine 只装配与关闭完整能力。** 不读 revision/key id/group，不为缓存或观测编排业务步骤；宿主和绑定不接触缓存。

### Data Model：私有状态

| 私有结构 | 内容与生命周期 |
| --- | --- |
| `ReadView` | 经完整校验的目录、revision、按 ContentKeyId 建立的查找索引、派生的 profile 搜索根。只保留一份目录密钥，索引引用组/条目位置，避免再复制完整 key map。 |
| `ReadState` | `Transient`、`ReusableEmpty`、`ReusableReady`、`Closed`；包含内存 generation 和可选目录。只在 vault 内可见。 |
| `ProfileKeyReadLease` | 不透明、可撤销的复用许可；引用本 vault 和本内存 generation，不携带密钥，不序列化。许可不可克隆；真正 clear 同步移出并析构它，不等待其他 Arc 释放。 |
| 内存代次令牌 `Arc<()>` | 冷加载/安装开始时记录的内存版本，用于阻止陈旧结果回填。它不是持久 revision，也不是观测标识。 |

继续沿用 4 MiB vault 明文、128 组、每组 1024、总计 4096 条目的现有限制。内存容器有额外开销，4 MiB 不是精确 RSS 上限。目录和派生根必须有受控清理；复用现有 `ZeroizeOnDrop` / `MasterKey`，不实现输出材料的 Debug。

外层 vault key 只存在于加载/写入的局部操作，加载时直接派生搜索根，随后清理；不为了读性能把外层 key 另做一个长期缓存。

### API / Interface：保持调用动作稳定

| Interface | 输入、结果与变化 |
| --- | --- |
| `ContentProtection::open(ciphertext, aad)` | 保持；按密文 id/epoch 找到真实保护组，派生固定 purpose key 后验证 AEAD。 |
| `ProfileContentKeyVault::resolve(id, epoch)` | 保持；复用私有目录，返回一条短生命周期材料；未知 id 和 epoch 不符保持不同错误。 |
| `search_catalog()` | 保持 Infra 内部 seam；从同一目录返回现有搜索能力，不额外取系统钥匙串，不返回整个历史目录。 |
| `install_verified_space_material(material)` | 保持完整动作与返回 summary；更新持久化和私有读视图，不要求调用方再 invalidate。 |
| `begin_read_reuse()` → `ProfileKeyReadLease` | 新增 Infra 内部方法，仅由完整安全会话负责人在成功激活时使用；不预读，首次消费时加载。重复激活同一有效 profile 复用已有许可。 |
| `ProfileKeyReadLease` 析构 | 同步撤销并清空长期目录；只获取短时状态锁，不 await 文件或钥匙串操作。 |
| `close_security_session()` | 新增为 RuntimeSpaceAccessAdapter 的完整 Infra 生命周期能力，撤销活动会话及 vault 使用，供现有 shutdown/reset owner 接线；终态不能被 resume 或旧句柄复活。 |

`Closed` 使用 vault 内部明确的状态错误，并由既有 ContentProtection::NotActive 保留为 source，搜索边界转换为 SessionLocked；它不是底层异常，不伪造 I/O 来源。不把它归类为密文损坏，不为关闭失败触发索引重建；如现有补全分支无差别收集错误，需要仅将该生命周期失败向上返回，并为此增加回归测试。

许可放在 session 的独立生命周期元数据中，**不作为 `SessionSnapshot` 所复制的密码状态**。`detached_clone` 不继承可撤销主运行期的许可；临时 validator/staging session 不开启主 vault 复用。普通同 profile 的 `set_master_key_for_space` 不撤销有效许可，真正 `clear` 才撤销。

安装/恢复事务记录 session 生命周期 generation；异步返回后更新或恢复快照必须核对它。`clear` 推进 generation，旧事务不可用 `restore(previous)` 复活已锁定的 MasterKey，也不可重新获取复用许可。取消时通过私有事务 guard 恢复合法旧状态或保持已撤销状态，不留临时目标 MasterKey。该 guard 属于现有完整 owner，不上移为 Application 步骤接口。

### Workflow：冷读和热读

1. 实例默认 `Transient`，维持升级检查和启动阶段既有读取能力；一次调用临时加载并清理，不长期持有历史材料。
2. 初始化、解锁或静默恢复成功后，完整 owner 关联复用许可，进入 `ReusableEmpty`。GUI 可处于锁定状态，不参与这个决定。
3. 首次读取在共享异步 I/O 互斥区内重新检查状态；读文件和系统安全存储，完成 AEAD/格式/目录校验并派生搜索根。
4. 仅当 generation、复用许可和目录更新状态仍匹配，才发布 `ReusableReady`。并发等待者重查后直接命中，不逐个重读。
5. 热读只在短时状态锁内定位并复制本次所需的一条密钥或搜索根；不把整个目录的 Arc 交给搜索请求持有。内容解密在状态锁外完成。
6. 未知 id 或错误 epoch 直接失败；禁止 miss 自动回读磁盘，否则恶意/损坏引用又会制造逐条 I/O。新合法材料必须经过已有安装入口。
7. 冷加载失败保持空状态、保留 source；等待中的一次失败不伪装成功。后续独立调用可重试；不增加后台无限重试或负缓存 TTL。

### Workflow：材料安装与磁盘一致性

- 读加载与写安装共用一个异步 I/O 互斥区。命中热读无需等待磁盘写入；同一旧身份的 key 不可变，写入期间使用旧已验证目录仍正确。
- 先构造完整候选并沿用 merge/validate/容量规则，再原子持久化；持久化成功后，在同一写入序列内发布新视图，安装返回后新读必须见到新 revision。幂等安装不推进 revision。
- 不能原地修改正在使用的目录；候选未提交时不可参与读。新增组/epoch 后更新搜索保护组视图。
- 写入尝试开始前设置私有失效 guard。失败或取消时清除缓存：现有文件替换后仍可能父目录同步失败，不能据 Result::Err 推断磁盘仍是旧版本。
- 成功持久化但 session 激活失败时，历史 catalog 的追加可以保留并幂等重用，沿用现有 033 规则；不回滚已确认的持久事实。
- 清理或 close 与写入并发时，候选可以按照既有持久提交责任收尾，但不得恢复复用或活动会话；下一次合法运行期从磁盘认证恢复。

### Workflow：GUI 锁定与真正安全锁定

| 动作 | GUI 交互 | 后台安全会话 / 密钥复用 | 接收同步及密文写入 |
| --- | --- | --- | --- |
| GUI 锁定 | 隐藏内容并撤销交互访问 | 保持 | 继续 |
| GUI 解锁 | 经宿主认证恢复交互 | 保持，不重新读 vault | 继续 |
| 显式安全会话锁定 | 不授予交互访问 | 暂停活动、clear、撤销复用 | 按现有安全锁定流程停止 |
| 同 profile 切换 Space / epoch | 按交互策略处理 | 保留 profile 历史目录，安装后追加新材料 | 按现有切换门禁恢复 |
| Engine 暂停 | 按宿主生命周期处理 | 不自动等同安全锁定；按现有 suspend 契约 | 按现有暂停策略处理 |
| shutdown / Factory Reset | 关闭交互 | 封口并清理，终态不可复活 | 停止；reset 在此后删除密钥 |

未来 GUI 锁定由宿主的**完整交互授权模块**负责，而非仅隐藏 WebView。它需要在所有交互命令、查询/预览/导出响应和含内容事件出口验证 UI 会话；GUI 锁定推进交互 generation，锁定前发起的查询不得在锁定后送回 GUI。后台接收以已有可信后台执行路径继续，不能借用 GUI 客户端权限，也不能设置一个由外部请求自报的 bypass 标志。

这份设计不新建 Engine GUI 状态，不让 `ContentProtection` 判断请求来自 GUI 还是同步。未来 GUI 负责人只调用自己的交互锁定动作，**不得调用 `lock_space_session`、`session.clear` 或网络暂停来实现 GUI 锁定**。主界面、Quick Panel、通知与导出入口必须由同一交互策略覆盖。

### 清理的精确定义

撤销时同步删除 vault 长期目录并推进 generation；已启动加载即使完成也不能回填。已复制到正在执行密码操作的单条材料，在该操作完成/取消并 drop 时清理；不能宣称 clear 可擦除其他任务已持有的副本、已返回明文或操作系统 swap。

`Closed` 拒绝新读取/安装，不退回 Transient。shutdown/reset 的完整生命周期入口先封口，随后按既有有界关闭机制等待/取消活动任务并清理；超时返回也必须已封口。Factory Reset 必须在删除安全存储前完成这一阶段。普通 GUI 锁定不触发上述动作。

真正安全会话 clear 后保留既有 Infra 临时历史读取语义（Transient），不借此次性能改动宣称新的全局历史读取授权。宿主用户访问由既有/未来授权边界限制；离线升级验证继续可用。若需要“显式安全锁定后所有内部历史读取也拒绝”的新能力，应另立授权设计并保留受限维护路径。

## 6. Implementation Plan

| 步骤 | 修改位置与动作 | 风险 / 完成信号 |
| --- | --- | --- |
| 1 | 在 vault/content_protection/search 真实测试 seam 增加安全存储计数和可控故障；先跑出现有线性 get 次数。 | 区分安装、冷加载和热读取计数，不能用 sleep 门禁代替因果证据。 |
| 2 | vault 的 model/catalog/persistence 增加已验证读视图、加载时派生根、私有状态与共享 I/O 序列。 | 目录复用与写入/取消一致性测试通过；旧格式与 source chain 不变。 |
| 3 | session 和 ActiveSpaceSecuritySession 关联不透明许可及事务 generation；覆盖 clear、恢复、临时 session、rebind。 | clear 与快照回滚不能复活材料；同 profile 切换不重复装载历史目录。 |
| 4 | 将 [运行期关闭](../../../crates/uc-engine/src/runtime/dispatch.rs) 与 [Factory Reset stopper](../../../crates/uc-engine/src/runtime/mod.rs) 接到完整关闭能力。 | 不因保留 Engine 句柄、supervisor factory 或关闭超时留住缓存；Engine 不解析缓存状态。 |
| 5 | 补充跨调用方和 GUI 锁定场景契约测试；GUI 宿主尚未实现时用明确标注的授权 gate 宿主模拟。 | 模拟不冒充真实 GUI 验收；后台接收须走真实 Engine/应用接收持久化链。 |
| 6 | 固定源码/二进制版本重测并更新稳定设计与架构圣经。 | 明确冷/热、数量、构建模式、成功与跳过；本提案实施完成后归档。 |

实现前确认同一 profile 是否被多个活跃 vault 实例/进程共享。缓存要求受管目录的安装全部经过唯一 owner；现有升级/激活短期 lease 不足以证明运行期单写者。若缺乏该保障，应在 Infra 为受管 vault 建立统一的运行期排他 lease，并让全部安装入口遵守；占用失败以带 source 的能力错误返回，不能默默运行互不失效的双缓存。该工作不交给搜索或宿主自行协调。

## 7. Edge Cases

| Scenario | Expected behavior / Implementation |
| --- | --- |
| 首次 profile 没有 vault | 保留现有未安装/KeyNotFound 语义；read 不创建空库或外层 key；合法安装创建。 |
| 多请求冷读 | 一次成功加载，等待者重查；不复制完整目录到每个查询。 |
| GUI 锁定时同步到达 | 保留网络与活动写密钥，完成验证及密文提交；禁止含内容响应/事件流向锁定 GUI。 |
| clear 与冷加载交错 | generation 不匹配则不发布；仅清理局部加载材料，不后台重启复用。 |
| clear 与恢复/安装交错 | 旧 snapshot 不得复活 MasterKey 或许可；已提交 catalog 由既有恢复入口接续。 |
| 改 epoch / 切换保护组 | 旧内容仍按 id/epoch 解密；新写入取当前活动材料；GUI 锁定不妨碍安装。 |
| 文件/钥匙串损坏或不可用 | 冷读保留准确分类和 source，禁止使用未经验证数据；已有效加载的材料按运行期合同继续可用，直到撤销或 owner 更新。 |
| 提交结果不确定 / 任务取消 | 失效缓存，下一次读重新认证；不悄悄覆盖历史或把 I/O 失败当没有数据。 |
| 超大/冲突 catalog | 沿用既有限制与冲突判断；候选被拒绝，不发布不完整视图。 |
| 旧 V1/V2 数据 | 只走现有一次性升级；升级临时读取不依赖活动会话存在。 |
| 网络失败 | 原接收/恢复 owner 处理；密钥缓存不创建重试任务，不改变回执或降级 LAN。 |
| profile generation 不同 / reset 后旧句柄 | 不共享对象或许可；新 generation 重新认证；已关闭旧句柄不能读取或安装。 |

## 8. Testing Strategy

### Unit Test

- 输入已安装历史 A/B 两组：重复 resolve A 100 次及 search_catalog 100 次；冷装载成功一次，热阶段 vault read/get 均为 0；验证每次 key identity 与 epoch。
- 输入有效目录后查不存在 key / 错误 epoch：分别返回原稳定错误，不触发重新读取；修改 purpose/AAD/密文仍认证失败。
- 用 barrier 暂停加载并 revoke/close，释放 barrier：不能发布旧视图；取消写入、rename 前后失败、父目录 sync 失败分别使缓存失效。
- 热目录下追加/幂等安装/冲突安装：成功返回后新材料可见；旧 key 不变，重复安装不改 revision，冲突不污染目录。
- clear 与 snapshot restore / detached clone / rebind 交错：撤销不能被旧操作回滚；销毁 detached session 不清掉主运行期。
- 不安全解引用已释放内存来测试 zeroize；验证所有权释放与已有 ZeroizeOnDrop 类型，检查 Debug 脱敏。

### Integration Test

- 真实 SQLite + V3 ContentProtection + 真实 vault 文件 + 计数 SecureStoragePort：构造 0、20、100 条跨保护组/epoch 内容，执行真实 `SearchIndexPort::query`；断言总数、排序、分页、正文正确及热阶段 get=0。
- 首次并发查询在独立冷实例运行，隔离安装计数；真实文件读和钥匙串 get 成功次数不随页数增长。热搜索同时交错 inline/blob/派生负载读取，证明共享 owner。
- 真实安装与查询并发、锁定/解锁、重启、Factory Reset、关闭超时：所有存储继续密文；reset 清理前封口，重启只信任持久状态。
- GUI 契约：真实后台接收同步期间锁定模拟 GUI gate；新内容成功持久化，锁定前后查询/含内容事件不能送入 GUI；解锁后读到新增内容，热目录未被重载。明确此项只是未来宿主契约的模型测试。

### Regression Test

沿用 033 的跨 Space inline/UCBL/render 可读性、错误 source、材料冲突、升级恢复和已清空活动 session 的输出验证。保留真正 lock_space_session 的暂停语义，不把它改成 GUI lock。

性能验证必须重新固定 Engine 与 desktop 源码、实际二进制 SHA-256、dev/release、平台后端与数据规模；同数据测 0/20/整页，分别记录重建运行期后的首次查询和至少五次连续热查询。一秒仅作为原调查信号；必须同时提交 get/read 计数证明消除了线性 I/O，不能只提交一个更快的耗时数字。macOS、移动设备与真实 GUI 未执行时记为跳过。

## 9. Acceptance Criteria

- [x] vault 是 profile 历史材料的唯一长期内存 owner；搜索、Desktop、Engine facade 无独立密钥缓存。
- [x] 热路径 resolve/search_catalog 的文件读取和安全存储 get 为 0；成功冷加载合并，搜索根与目录共用一次外层 key 读取。
- [x] 跨保护组、历史 epoch、活动 Space 切换后的内容、分页、排序和完整性语义不变。
- [ ] 跳过：真实 GUI 锁定尚未实现。架构已明确它不得触发安全会话 clear、缓存撤销或同步暂停；本轮不宣称完成 GUI 接收验收。
- [x] clear、close、reset、取消和失败回滚不会回填旧快照或复活活动材料。
- [x] 安装成功后新读见新 revision；提交结果不确定时重新认证磁盘，旧历史仍可恢复。
- [x] 所有调用方共享同一个受管 vault；多实例/进程一致性前提有已验证的排他机制。
- [x] 无新明文业务持久字段、敏感日志或稳定 Engine/binding 接口泄漏；受影响错误保留 source。
- [x] Infra 全套含离线升级验证通过；GUI 模拟与真实宿主验收均明确跳过。
- [x] 必要相关测试、metadata、workspace all-targets check、fmt、架构检查、diff/link 检查完成；全量失败和设备/GUI 跳过如实记录。

## 10. Risks and Trade-offs

| 备选 | 取舍与决定 |
| --- | --- |
| 搜索单页临时快照 | 改动小，但每次搜索仍重复读系统存储，inline/blob 等其他读取仍慢；不作为统一方案。 |
| 直接读当前活动 content key | 无法解析其他保护组历史密文，违反 033；拒绝。 |
| 把所有历史密钥放进 InMemorySession | 混合活动 Space 与 profile 历史所有权，快照/切换更复杂；只关联不透明许可，不移动目录。 |
| vault 内运行期复用 | 复用现有深模块、调用 Interface 基本不变；代价是清理、并发安装和单 owner 约束必须明确。采用。 |
| GUI 锁定清空全部密钥 | 与用户要求的后台接收及加密写入冲突；GUI 授权与后台能力分开。 |
| TTL/LRU 或后台刷新 | 引入过期时间/淘汰启发式，不能证明锁定清理和更新一致性；现有容量已可有界，不采用。 |

长期明文密钥从“每次瞬时加载”变成“后台安全运行期持有”，这是明确的内存生命周期取舍；后台同步本来也需要活动密钥。GUI 锁定保护交互出口，不提供进程被控制时的密钥隔离。真正需要停用密码能力的动作仍是独立安全会话锁定/关闭。

规则属于 Core/已有安全契约；流程属于 Application；复用、文件一致性和内存清理属于 Infra。本方案没有基于时间或命中率的 heuristic。

## 11. Open Questions

- 未来 GUI 锁定是否同时禁止自动写入系统剪贴板、通知预览、本机剪贴板采集及用户触发的重发？用户已明确的是“后台接收并持久化继续”；这些其他交互行为须由 GUI 功能规格决定。本次不改变它们。
- GUI 认证方式与跨主窗口/Quick Panel/CLI 的会话授权如何定义？本次只确定它们不能复用安全会话 lock。
- 已建立 vault 统一文件排他租约：可复用实例驻留持有，维护读写临时持有；第二实例明确返回能力错误。多进程同时活跃不是本轮支持目标。
- 确切性能目标及各平台占比尚无一致构建条件的数据；本次可承诺的是移除热路径重复 I/O，耗时指标需固定版本复测。

## 实施记录

- 已完成 vault 私有 ReadView、查找索引、搜索根单次派生、运行期文件排他租约和安装/取消失效。外层 key 未长期缓存，持久格式未改动；新增固定空租约文件不承载业务负载。
- 会话通过不可克隆许可关联复用，事务取消与 clear/close 的代次检查防止恢复旧密钥；维护/升级 session 明确不取得后台许可。
- Engine shutdown、Factory Reset、启动失败/取消和运行期析构均接到完整安全关闭能力；普通 suspend/resume 与未来 GUI 授权不清理此能力。
- 关闭后的内容读取归入 NotActive，搜索返回 SessionLocked，不把关闭当作 render 损坏并安排重建。
- 已补齐原有 LAN 测试缺失的 tracing-subscriber 开发依赖，以运行 workspace 门禁；未改 LAN 业务实现。
- 原 dev-tools Engine 重发测试期望 SynchronizationDisabled，但当前 Application 只有枚举声明，没有返回该值的分支。原测试完整保留；全量命令失败与仅排除此测试的结果分别记录，不冒充全量通过。
- GUI 锁定功能尚未实现，因此不新增只切换 boolean 的模拟来宣称 GUI 验收。主界面、Quick Panel、真实 daemon 二进制的替换后重测、macOS 与移动设备均明确跳过（本轮保留运行中的 dev daemon，性能因果验证使用真实 SQLite/密文 fixture）；本轮交付 Engine 实现和真实存储测试。


### 补充验证记录

- 全量 workspace 首轮因原有重发结果断言失败；排除该测试后的拓扑套件并行运行失败且持续占用资源，主动结束该轮。不能标记全量通过。
- 对其中 F5 ring 冲突传播场景单独串行复跑通过（183.76 秒）。资源竞争是此前并行失败的可能因素，尚未逐项确认 F2/F4/F6 的原因。
- 最终人工审查补齐 clear/rebind 原子性，以及 Transient 冷读期间开始复用的代次边界；均有回归测试。未找到安装的 code-review skill，使用人工 diff/所有权/取消路径审查。
- 补跑 `cargo test --workspace --exclude uc-engine --locked -- --test-threads=2` 完整通过：2851 项通过、7 项忽略；其中 Infra 808 项通过、4 项忽略。Engine 既有单元/契约套件结果见前述全量记录，排除 Engine 的命令不替代 Engine 验收。
- 最终 `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、架构脚本和 diff/link 检查通过。

### SQLite 性能因果验证

Linux ARM64，Cargo test 未优化构建；100 条真实 SQLite V3 密文 fixture，分属两个保护组，SecureStorage 使用内存计数替身。每页与未复用参考结果逐字段相等，覆盖 20/100 条浏览、分页搜索和无匹配；五轮合计外层 key get 为 1（首次冷读），后续为 0。

| 场景 | 未复用参考 | 复用（第 0–4 轮范围） |
| --- | ---: | ---: |
| 浏览 20 条 | 15.218 ms | 1.734–3.648 ms |
| 浏览 100 条 | 69.615 ms | 12.358–14.888 ms |
| 搜索分页 20 条 | 18.339 ms | 3.340–12.487 ms |
| 无匹配 | 1.030 ms | 0.389–1.322 ms |

这些耗时只描述测试 fixture，未使用系统钥匙串，不与旧 daemon 的 HTTP 秒级数据直接计算倍数。关键验收是单次冷加载、热路径不读 vault/安全存储以及结果一致；运行中的旧 daemon 尚未替换，因此产品端性能改善待宿主升级后二次验收。
