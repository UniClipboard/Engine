# `uc-core` 设计规范

## 1. 定位

`uc-core` 回答三件事：**什么是允许的、状态怎么变、变了之后必须发生什么**。它不回答“怎么做、按什么顺序做、存在哪里”。

| 层 | 负责 | 本规范中的简称 |
| --- | --- | --- |
| `uc-core` | 领域概念、合法状态转换、不变量、每次转换的效果义务、跨设备规则 | 规则 |
| `uc-application` | 按效果义务调用能力的顺序、事务切分、重试与重启恢复 | 流程 |
| `uc-infra` | 字节格式、数据库、网络、密码学、平台 API | 能力 |

跨层原则见[核心信念](../core-beliefs.md)与[工程与模块设计原则](../engineering-principles.md)；本文只规定 Core 内部怎么写。
当前代码与本文的差距及收口计划见 [Core 边界收口计划](../../exec-plans/active/2026-09-23-core-boundary-remediation.md)。

### 1.1 读 Core 应该能得到什么

只读 `uc-core`，开发者必须能回答：

1. 一个有生命周期的对象有哪些状态？从每个状态收到什么输入，能到达哪个状态？
2. 每次转换之后必须发生哪些副作用？每个副作用必须在保存新状态**之前**还是**之后**完成？
3. 哪些输入会被幂等忽略、哪些被拒绝、哪些使对象进入需要人工或恢复处理的状态？
4. 跨对象、跨设备的不变量是什么，例如“什么情况下不能开始新的准入”“哪台设备有资格收到哪次更新”？

只读 Core 回答不了、也不应回答：副作用由哪个 adapter 执行、事务怎样切分、崩溃后从哪一步重放、数据存在哪张表。
如果上面四个问题中任何一个必须到 Application 或 Infra 里找答案，说明规则泄漏到了 Core 之外，属于缺陷。

## 2. 放什么，不放什么

### 2.1 放入 Core

| 类别 | 说明 | 示例 |
| --- | --- | --- |
| 标识与值对象 | 构造时校验，之后不可变 | `DeviceId`、`SpaceId`、`MemberInstanceId`、`InvitationCode` |
| 聚合与状态机 | 有生命周期、需持久化、接受外部输入推进的对象 | `SpaceAdmissionAggregate`、`SpaceMembershipCandidate`、撤销记录 |
| 不变量与策略 | 纯函数形式的判定 | 冲突裁决、准入是否阻止新准入、更新接收者集合 |
| 领域证明 | 签名或摘要覆盖的规范内容及其验证规则 | `VersionedMembershipHistory` 的规范签名材料 |
| 领域错误 | 类型化、可分类、不含敏感内容 | `SpaceAdmissionAggregateError` |
| Core 领域代码直接消费的 port | 见 §7 | `HistoricalMembershipSignatureVerifier` |

### 2.2 不放入 Core

| 类别 | 归属 | 说明 |
| --- | --- | --- |
| UseCase、流程顺序、重试、恢复调度 | Application | 规则告诉流程“必须做什么”，流程决定“怎么依次做” |
| 只被 Application 使用的 port | Application | 见 [Application 规则 4](application.md) |
| 本地存储格式、格式版本常量、编解码 | Infra | 例外见 §6.2 |
| 设备间协议的线上编码 | Infra | 消息的语义结构与校验规则仍在 Core |
| 数据库、文件系统、网络、平台 API、路径 | Infra | 包括 `PathBuf` 形式的应用目录布局 |
| 密钥物料、KDF 参数、算法调用、随机数 | Infra | Core 只保留领域中性类型，如 `Plaintext`、`Ciphertext`、`Aad` |
| 配置加载、设置文件格式、默认路径 | Infra / Application | Core 可保留业务设置的语义类型 |
| 异步运行时、任务管理、通道 | Application / Infra | Core 不感知并发模型 |
| 已不再支持的兼容线实现细节 | 对应兼容负责人 | Core 只保留仍被主产品规则使用的概念 |

### 2.3 归属判定

```text
它能在没有网络、磁盘、时钟、随机数和 UI 的单元测试里完整表达吗？
  否 → 不是 Core。
它决定“是否允许”“变成什么”“必须发生什么”吗？
  是 → Core，即使目前只有一个调用方。
它决定“先做哪个、失败后怎么办、何时再试”吗？
  是 → Application。
它决定“字节长什么样、存在哪里、通过什么发送”吗？
  是 → Infra。
```

## 3. 依赖纪律

| 依赖 | 结论 | 条件 |
| --- | --- | --- |
| `std` | 允许 | 不得使用 `std::fs`、`std::env`、`std::process`、`std::net` 与系统时钟 |
| `serde`（derive） | 允许 | 仅用于领域快照与规范编码，见 §6 |
| `thiserror` | 允许 | 领域错误 |
| `chrono` | 允许 | 只作时间类型；不得调用 `Utc::now()` |
| `sha2`、`hex`、`arrayvec`、`zeroize`、`uc-content-hash` | 允许 | 用于领域身份、规范摘要和值对象 |
| `postcard` | 受限 | 只能用于 §6.2 的规范编码 |
| `async-trait` | 受限 | 只能用于 §7 允许留在 Core 的 port |
| `tokio`（任何 feature）、`tokio-util` | 禁止 | 通道、锁、任务属于运行时 |
| `anyhow` | 禁止 | 领域错误必须类型化，source chain 由上层保留 |
| `serde_json`、`toml`、`url`、`bytes` | 禁止 | 属于格式或传输 |
| `uuid` 的随机生成 | 禁止 | 标识由调用方传入或由 port 生成；Core 可以解析、校验 |
| 网络、数据库、UI、平台 SDK、密码算法实现 | 禁止 | — |

新增依赖必须在 PR 中说明它为什么是领域概念而非实现细节。

## 4. 值对象与实体

- 字段私有；只通过校验构造器创建，构造失败返回类型化错误。
- 不得 `panic!`、`unwrap()`、`expect()`、`unreachable!()`；外部输入永远可能非法。确属编译期可证明的分支，用类型让它不可表达，而不是运行期断言。
- 不在构造器里生成随机值或读取时钟；需要新标识或当前时间时由调用方传入。
- `Debug` 与 `Display` 不输出剪贴板内容、密钥、完整令牌、设备名、地址、文件名或路径，见[安全架构](../../SECURITY.md)。
- 实体不持有 port，不执行 IO；需要外部结果的规则以参数接收结果。

## 5. 状态机

凡是有生命周期、需要持久化、并接受外部输入推进的对象，都必须按本节建模。这是 Core 最重要的写法约束。

### 5.1 单一转换入口

```rust
pub struct Transition<A, O, E> {
    replacement: A,          // 新状态；调用方保存它，不修改它
    outcome: O,              // 本次输入被怎样处理
    effects: Vec<E>,         // 调用方必须履行的效果义务
}

impl SpaceAdmissionAggregate {
    pub fn apply(
        self,
        input: AdmissionInput,
        now_ms: i64,
    ) -> Result<Transition<Self, AdmissionOutcome, AdmissionEffect>, SpaceAdmissionAggregateError>;
}
```

规则：

- 每个聚合只有一个公开的状态推进方法。构造初始状态用 `start_*`/`from_*` 构造器；其后一切变化走 `apply`。
- 禁止公开 `mark_*`、`set_*`、`advance_*`、`transition_to` 一类逐步修改方法，禁止公开状态字段（`pub phase`、`pub status`）。
- 状态分发写在 `apply` 的一个穷尽 `match` 里，使“状态 × 输入 → 结果”在一处可见。各分支的实现可以拆到按角色或阶段划分的私有函数与文件中；拆分文件不得增加公开入口。
- 允许按角色提供只读包装（如 `JoinerAdmission`、`SponsorAdmission`），但包装只能限制可接受的输入种类，内部仍调用同一个 `apply`，持久化结构仍是同一个聚合。
- 转换必须是纯函数：不读时钟、不取随机数、不访问 port。外部世界的结果（验证结论、对端消息、准备好的材料）作为输入携带进来。

### 5.2 输入

- 输入按调用方语义命名，一个变体代表一次完整的外部事实：收到某条消息、某项能力完成、用户取消、到达期限。不要按内部步骤命名。
- 输入携带转换所需的全部材料。Application 准备材料（例如签好的历史、已暂存的安全状态），Core 校验材料与当前状态是否一致。
- 新增输入前先回答：它是对端资料、验证结论、本机能力结果、用户意图还是时间信号？答不清不得实现。

### 5.3 结果

- 重复、乱序、过期、已被取代等**正常业务情况**用 `outcome` 表达（如 `Duplicate`、`Stale`、`Unchanged`、`ExactReply`），不得用错误表达。
- 错误只表达“这个输入在当前状态下不合法”或“记录已无法安全推进”，并通过 `category()` 给出稳定分类，供流程决定拒绝、等待或进入恢复。
- 终态在 `apply` 内守卫，任何输入都不能把终态改回可推进状态。

### 5.4 效果义务

效果是 Core 与 Application 之间**有约束力**的契约，不是提示：

```rust
pub enum AdmissionEffect {
    // 必须在保存 replacement 之前成功完成；失败则不得保存 replacement。
    BeforeCommit(AdmissionPrerequisite),
    // 保存 replacement 之后执行；必须可幂等重放，未完成时由流程负责人登记恢复。
    AfterCommit(AdmissionFollowUp),
}
```

- 每个效果必须声明它与保存新状态的先后关系。正确的先后关系是规则的一部分，因为它决定崩溃后世界处于什么状态。
- Application 必须**恰好**履行返回的效果：不得遗漏，不得自行增加 Core 没有声明的业务副作用，不得调换 `BeforeCommit` 与保存的先后。
- 效果描述“必须达成的业务结果”（激活安全状态、发布成员变化、登记逐设备传播义务），不描述实现步骤（写哪张表、调用哪个 adapter）。
- 转换不产生效果时返回空列表。测试断言每次转换的完整效果列表。
- Application 的流程负责人必须有测试证明：对每种效果，都存在唯一的履行位置，且履行顺序与声明一致。

### 5.5 跨对象不变量

有些规则跨越多条记录，例如“同一时间最多一个阻止新准入的准入”“一张邀请只能被领取一次”。

- **判定**写在 Core：以纯函数表达，输入是相关记录集合或其摘要，输出是允许、拒绝或冲突。
- **原子性**由持久化负责人保证：Infra 仓储在同一事务里读取相关记录、调用 Core 判定、写入结果。
- Infra 不得自行拟定判定条件，只能调用 Core 判定并执行它的结论。

### 5.6 时间、期限与随机

- 当前时间以 `now_ms` 参数传入。期限的计算规则（何时过期、宽限多久）在 Core；何时唤醒检查在 Application。
- 标识、nonce 等随机值由调用方生成后传入，Core 只校验格式与唯一性约束。

### 5.7 模块入口

每个状态机模块的 `mod.rs` 顶部用 doc comment 写清：

1. 状态列表与一句话含义；
2. 输入列表；
3. 效果列表及各自先后关系；
4. 不变量与终态。

详细的时序图、跨负责人的收尾关系写在对应的设计文档中（例如[配对生命周期](../pairing-lifecycle.md)），并链接回代码入口。模块文档只写规则，不写调用方和实现。

## 6. 持久化与编码边界

### 6.1 默认规则

Core 不定义本地存储格式。聚合提供快照与恢复：

```rust
impl SpaceAdmissionAggregate {
    pub fn snapshot(&self) -> SpaceAdmissionSnapshot;
    pub fn restore(snapshot: SpaceAdmissionSnapshot) -> Result<Self, SpaceAdmissionRestoreError>;
}
```

- 快照是纯数据，可以派生 `serde`；它不带格式版本号，不包含编解码函数。
- `restore` 必须重新校验全部不变量，拒绝不一致的快照；它是除构造器与 `apply` 外唯一产生聚合的入口。
- 字节布局、格式版本、加密封装、旧格式升级都属于 Infra。格式演进遵守[持久化格式演进](../engineering-principles.md#持久化格式演进)。

### 6.2 例外：规范编码

当一段字节被**签名或摘要覆盖**、并且多台设备必须对它得出相同结论时，这段编码就是领域证明的一部分，可以放在 Core：

- 例如成员历史的规范签名材料、准入消息用于证据比对的规范摘要输入。
- 模块文档必须声明“本编码受签名/摘要覆盖”，并有字节稳定性测试。
- 只放规范编码本身；存储它的外层记录格式仍在 Infra。

### 6.3 设备间协议

- 消息的语义结构、字段约束、与状态的匹配校验在 Core。
- 线上帧格式、分片、传输协议版本在 Infra。
- 消息若属于 §6.2，规范编码在 Core。

## 7. Port

一个 port 只有在 **Core 领域代码直接调用它**时才能定义在 Core；否则放在使用它的 Application 业务模块旁边。
判定与示例见 [Application 规则 4](application.md)，分类与粒度见 [Port 定义](../ports.md)。

留在 Core 的 port 的 doc comment 只能写领域契约：它在领域里做什么、输入输出含义、幂等性与原子性、错误语义、调用前后的不变量。
不得出现：

- 上层模块、UseCase、facade 的名字；
- 协议、路由、传输名（如 iroh、HTTP 路由）；
- 具体实现（SQLite、Argon2、OsRng、keychain、adapter 类型名）；
- 调用场景或调用顺序（“用于 X 流程”“先调 A 再调 B”）。

自查：删掉所有提到调用方、协议和实现的句子，剩下的内容还能让一个不了解本项目的人理解契约吗？换一个调用方，这段注释需要改吗？

## 8. 错误

- 使用 `thiserror` 定义类型化错误；不使用 `anyhow`。
- 每个错误类型提供稳定分类（如 `category()`），流程按分类决策，不解析错误文本。
- 错误文本不包含 §4 列出的敏感信息。
- 错误处理与跨层 source chain 规则见[错误处理与转换](../error-handling.md)。

## 9. 注释与命名

- Core 的行内注释与 doc comment 只使用中文；标识符与提交信息使用英文。
- 命名：实体、值对象用名词；port 用 `*Port`；错误用 `*Error`；输入用调用方语义名词；outcome 与事件用过去式或结果名词；策略用 `*Policy`。

## 10. 测试

- **转换矩阵**：每个“状态 × 输入”断言结果状态、outcome 与完整效果列表；非法组合断言被拒绝且原状态不变。
- **不变量**：终态不可回退、重复与乱序幂等、过期不复活、`restore` 拒绝不一致快照。
- **规范编码**：§6.2 的编码有字节稳定性测试。
- 测试只通过构造器、`restore` 和 `apply` 构造状态，不为测试开放额外写入入口。

## 11. 自查清单

提交涉及 `uc-core` 的变更前逐项确认：

- [ ] 新代码能在无 IO、无时钟、无随机数的单元测试中完整表达。
- [ ] 没有引入 §3 禁止或受限条件之外的依赖。
- [ ] 有生命周期的对象只有一个公开推进入口，状态字段不公开。
- [ ] 每个转换返回的效果声明了与保存的先后关系，并有测试断言完整效果列表。
- [ ] 跨对象判定写在 Core，Infra 只保证原子性。
- [ ] 没有新增本地存储格式、格式版本或编解码；若新增规范编码，已声明并有字节稳定测试。
- [ ] 新 port 确实被 Core 领域代码直接调用，注释只写领域契约。
- [ ] 生产代码没有 `panic!`、`unwrap()`、`expect()`、`unreachable!()`。
- [ ] 注释为中文，`Debug`、`Display` 与错误文本不含敏感信息。

## 12. 相关文档

- [Application 设计规范](application.md)：流程负责人、port 所有权、UseCase 组织。
- [Infra 设计规范](infrastructure.md)：能力实现、格式与迁移。
- [Port 定义](../ports.md)：port 分类与粒度。
- [配对生命周期](../pairing-lifecycle.md)：准入状态机与跨负责人收尾的当前时序。
- [成员历史职责](../membership-history-ownership.md)：成员历史规则在 Core 与 Application 之间的细分。
