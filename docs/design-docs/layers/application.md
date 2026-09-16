# Application 设计规范

## 规则 1：依赖只能向内

这是最高优先级规则。

固定依赖方向：

```text
Infrastructure
      ↓
Application
      ↓
Domain/Core
```

也就是说：

```text
domain
    不依赖 application
    不依赖 infrastructure

application
    可以依赖 domain
    不依赖 infrastructure

infrastructure
    可以依赖 application
    可以依赖 domain
```

Rust crate 必须满足：

```text
uc-core
   ↑
uc-application
   ↑
uc-infra
```

严禁：

```text
uc-core        → uc-application
uc-core        → uc-infra
uc-application → uc-infra
```

这是第一条检查项。

---

## 规则 2：UseCase 必须属于 Application

只要代码表达的是：

> 用户或系统发起一个业务动作，然后协调多个能力完成这个动作。

它就是 UseCase。

例如：

```text
RebuildSpaceUseCase
JoinSpaceUseCase
RemoveDeviceUseCase
PairDeviceUseCase
SendClipboardUseCase
```

必须放：

```text
application/
```

不要放：

```text
core/
infra/
```

UseCase 负责：

```text
流程编排
顺序
事务边界
业务级失败处理
调用多个领域对象 / Port
```

UseCase 不负责：

```text
SQL
HTTP
QUIC
iroh
文件系统
操作系统 API
序列化实现
```

---

## 规则 3：Port 归“需要它的人”所有

这是你现在最需要固定下来的规则。

判断：

> 谁提出“我需要这个能力”，Port 就定义在哪一层。

例如：

```rust
RebuildSpaceUseCase
    ↓ needs
RebindSpaceSessionPort
```

那么：

```text
RebindSpaceSessionPort
```

属于：

```text
application
```

因为是 Application 需要这个能力。

反过来：

```rust
ExpirationPolicy
    ↓ needs
Clock
```

如果 `ExpirationPolicy` 是 Domain：

```text
ClockPort
```

属于：

```text
domain/core
```

所以不要按“它最终由 infra 实现”来决定 Port 放哪。

必须按：

```text
谁消费接口
```

来决定。

---

## 规则 4：只有 Domain 真正使用的 Port 才能进入 Core

这条可以作为 code review 的硬检查。

一个 Port 想进入 `uc-core`，必须回答：

> 是否存在 core/domain 中的代码直接依赖这个 trait？

例如：

```rust
pub struct ExpirationPolicy {
    clock: Arc<dyn Clock>,
}
```

那么：

```rust
trait Clock
```

可以在 core。

但如果：

```rust
trait RebindSpaceSessionPort
```

只有：

```rust
RebuildSpaceUseCase
```

在使用：

```rust
pub struct RebuildSpaceUseCase {
    session: Arc<dyn RebindSpaceSessionPort>,
}
```

那么它不能放 core。

必须放 application。

---

## 规则 5：禁止建立“公共 Port 仓库”

不要出现这种结构：

```text
uc-core/
└── ports/
    ├── SettingsPort
    ├── DeviceIdentityPort
    ├── NetworkPort
    ├── RebuildPort
    ├── SessionPort
    ├── ClipboardPort
    └── ...
```

如果这些 Port 实际属于完全不同的 UseCase，这种结构会迅速失去业务语义。

优先：

```text
application/
├── rebuild_space/
│   ├── use_case.rs
│   ├── ports.rs
│   └── error.rs
│
├── join_space/
│   ├── use_case.rs
│   ├── ports.rs
│   └── error.rs
│
└── sync_clipboard/
    ├── use_case.rs
    ├── ports.rs
    └── error.rs
```

即：

> UseCase-specific Port 必须和对应 UseCase 放在同一个业务模块附近。

---

## 规则 6：Domain 不允许出现技术词汇

`core/domain` 里看到以下词汇，要高度警惕：

```text
Sqlite
HTTP
REST
QUIC
Iroh
TCP
Redis
JSON
Tauri
Android
iOS
Windows
Keychain
Filesystem
```

Domain 应该说：

```text
Space
Membership
Device
Identity
Invitation
Peer
ClipboardItem
Authorization
```

例如：

不好：

```rust
SqliteSpaceRepository
HttpDeviceClient
IrohSessionManager
```

Domain 里应该只出现：

```rust
SpaceRepository
DeviceDirectory
SessionBinding
```

具体技术实现全部在 infra。

---

## 规则 7：Application Port 必须描述“能力”，不能描述实现

Port：

```rust
trait RebindSpaceSessionPort
```

可以。

因为它描述的是：

> 重新绑定 Space Session 的能力。

不推荐：

```rust
trait IrohRebindPort
```

因为 `iroh` 是实现细节。

也不推荐：

```rust
trait HttpSpaceApi
```

除非 HTTP 本身就是业务概念，这种情况很少。

---

## 规则 8：Infrastructure 只能实现规则，不能决定业务流程

Infra 可以：

```rust
impl RebindSpaceSessionPort for IrohSessionAdapter
```

但是 Infra 里面不能出现：

```rust
if rebuild_completed {
    rebuild_membership();
    mark_setup_finished();
}
```

如果这表达的是业务流程，它必须回到 Application。

Infra 负责：

```text
怎么完成
```

Application 负责：

```text
什么时候完成
按什么顺序完成
失败后业务上怎么办
```

---

## 规则 9：业务顺序必须能从 UseCase 中直接读出来

例如正确：

```rust
pub async fn execute(&self) -> Result<()> {
    let prepared = self.transition.prepare(...).await?;

    self.session.rebind(...).await?;

    self.membership.rebuild(...).await?;

    self.transition.commit(prepared).await?;

    self.setup_status.mark_completed().await?;

    Ok(())
}
```

只看这段代码，就应该能理解：

```text
prepare
↓
rebind
↓
rebuild membership
↓
commit
↓
mark completed
```

不要写成：

```rust
self.rebuild_service.run().await?;
```

然后所有业务流程实际上藏在 infra 或某个巨大 service 里面。

否则 Application 失去存在意义。

---

## 规则 10：Port 不应该只是为了 mock 而创建

不能因为：

> 这个东西测试时我要 mock。

就创建 Port。

必须存在一个真正的架构边界。

例如：

```rust
trait Clock
```

合理，因为时间是外部世界。

```rust
trait RandomGenerator
```

可能合理。

但是这种：

```rust
trait MembershipValidatorPort
```

如果 `MembershipValidator` 其实只是纯业务逻辑：

```rust
fn validate(membership: &Membership) -> Result<()>
```

那它应该直接作为 Domain Service / function，而不是 Port。

原则：

```text
纯业务逻辑 → Domain code
跨边界能力 → Port
```

---

## 规则 11：Entity / Value Object 不允许依赖 Port，除非这是明确的 Domain Service

通常 Entity 应尽量保持：

```rust
Space
Membership
DeviceId
SpaceId
```

纯净。

不要：

```rust
impl Space {
    async fn rebuild(&self, repo: &dyn SpaceRepository)
}
```

除非你经过明确设计认为这是 Domain Service 的职责。

更常见的是：

```text
Entity / Value Object
        ↑
Domain Service
        ↑
Application UseCase
```

而不是让 Entity 到处拿 Port。

---

## 规则 12：Composition Root 是唯一知道具体实现的地方

最终：

```rust
let session = Arc::new(IrohSessionAdapter::new(...));

let use_case = RebuildSpaceUseCase::new(
    session,
    ...
);
```

这种 wiring 必须集中在最外层。

例如：

```text
runtime/
bootstrap/
desktop/
mobile/
daemon/
```

这里可以同时知道：

```text
Application trait
+
Infrastructure implementation
```

Application 自己不能：

```rust
IrohSessionAdapter::new()
```

否则依赖方向被破坏。

---

## 规则 13：Application Error 和 Domain Error 分开

Domain Error 表达领域规则失败：

```rust
MembershipError::DeviceAlreadyMember
SpaceError::InvalidState
InvitationError::Expired
```

Application Error 表达 UseCase 执行失败：

```rust
RebuildSpaceError::PreparationFailed
RebuildSpaceError::RebindFailed
RebuildSpaceError::CommitFailed
```

Application 可以包装 Domain Error：

```text
Domain Error
    ↓
Application Error
```

反过来绝对不允许：

```text
Domain Error
    ↓ depends on
Application Error
```

---

## 规则 14：不要按“技术层”组织 Application，按业务能力组织

不要：

```text
application/
├── services/
├── ports/
├── DTOs/
├── errors/
└── usecases/
```

项目大了以后很难追业务。

推荐：

```text
application/
├── rebuild_space/
│   ├── use_case.rs
│   ├── ports.rs
│   ├── error.rs
│   └── model.rs
│
├── join_space/
│   ├── use_case.rs
│   ├── ports.rs
│   └── error.rs
│
└── remove_device/
```

这是 vertical slice。

---

## 判断表

以后遇到任何新代码，可以按这个表决定：

| 问题                                    | Yes                 | No       |
| --------------------------------------- | ------------------- | -------- |
| 它是纯业务概念/规则吗？                 | Domain              | 继续     |
| 它描述一个完整业务动作吗？              | Application UseCase | 继续     |
| 是 UseCase 为完成任务需要的外部能力吗？ | Application Port    | 继续     |
| 是 Domain 本身需要的外部能力吗？        | Domain Port         | 继续     |
| 是 SQL/网络/文件/OS/第三方 SDK 实现吗？ | Infrastructure      | 继续     |
| 是实例创建、依赖注入、wiring 吗？       | Composition Root    | 重新检查 |

---

## 示例：`RebuildSpaceUseCase` 的 Port 所有权

以下这组能力不能因为都以 `Port` 结尾就统一放入 Core：

```rust
SettingsPort
LocalIdentityPort
DeviceIdentityPort
SpaceRebuildTransitionPort
RebindSpaceSessionPort
SpaceMembershipRebuildPort
ClockPort
SetupStatusPort
```

不能全部因为叫 `Port` 就放 core。

你应该逐个问：

```text
谁直接使用它？
```

如果都是：

```rust
RebuildSpaceUseCase
```

直接使用，那么默认应该是：

```text
application/rebuild_space/ports.rs
```

例如我初步会这样判断：

```text
SpaceRebuildTransitionPort      → Application
RebindSpaceSessionPort          → Application
SpaceMembershipRebuildPort      → Application
SetupStatusPort                 → Application
SettingsPort                    → 看消费者
LocalIdentityPort               → 看消费者
DeviceIdentityPort              → 看消费者
ClockPort                       → 看 Domain 是否直接依赖
```

不要因为 `ClockPort` 看起来“很底层”就自动放 core。

**消费者在哪一层，它就优先属于哪一层。**
