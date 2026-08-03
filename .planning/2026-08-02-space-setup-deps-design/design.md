# Space Setup 依赖收口设计

## 1. 决策

删除当前过宽的 `SpaceSetupFacade` / `SpaceSetupDeps`，改为：

- 对 `uc-engine` 保持一个面向用户动作的 `SpaceFacade`。
- 在 `uc-application` 内部按完整责任拆成四个深模块：
  - `SpaceSessionCoordinator`：创建、解锁、恢复、锁定、重置。
  - `SpaceAdmissionCoordinator`：邀请、入站配对、首次加入。
  - `SpaceTransitionCoordinator`：已有设备切换空间、重加密、崩溃续跑。
  - `SpaceMembershipGossip`：成员传播、确认、重试、在线维护和收敛状态。
- 用私有 `SpaceActivityCoordinator` 统一安排依赖解锁会话的后台能力。
- 用 `SpaceApplicationRuntime` 持有应用层后台任务；对 `uc-engine` 只公开最终 `shutdown`，不公开单项 `pause` / `resume`。
- 共享 Iroh 节点仍由 `uc-engine` 持有，因为它同时服务剪贴板、文件和成员协议。

这不是把 24 个字段换成几个小包。旧依赖按照真正负责结果的模块重新归属；无关模块的测试不再构造它们。

## 2. 为什么选择该方案

评估过三类设计：

1. 单一命令/查询入口：外部最小，但中心枚举会成为新的大入口，独立错误语义也会被压平。
2. 四个公开 Facade：责任最清楚，但 `uc-engine` 容易重新承担跨 Facade 的调用顺序。
3. 一个调用入口、四个内部负责人、一个运行期所有者：既保持调用简单，也把变化集中到真正负责的模块。

选择第三种。它保留第一种的小接口和第二种的内部局部性，同时不让 `uc-engine` 重新成为流程编排者。

## 3. 外部接口

`Engine` 的公开 operation、结果和错误码保持不变。`uc-engine` 内部只通过 `AppFacade` 转发到 `SpaceFacade`。

```rust
pub struct SpaceFacade {
    session: Arc<SpaceSessionCoordinator>,
    admission: Arc<SpaceAdmissionCoordinator>,
    transition: Arc<SpaceTransitionCoordinator>,
    membership: Arc<SpaceMembershipGossip>,
}

impl SpaceFacade {
    pub async fn create_space(
        &self,
        input: CreateSpaceInput,
    ) -> Result<CreateSpaceResult, CreateSpaceError>;

    pub async fn unlock_space(
        &self,
        input: UnlockSpaceInput,
    ) -> Result<UnlockSpaceResult, UnlockSpaceError>;

    pub async fn recover_session(
        &self,
        input: RecoverSessionInput,
    ) -> Result<RecoverSessionResult, RecoverSessionError>;

    pub async fn lock_space(&self) -> Result<(), LockSpaceError>;

    pub async fn join_space(
        &self,
        input: JoinSpaceInput,
    ) -> Result<JoinSpaceResult, JoinSpaceError>;

    pub async fn factory_reset(&self) -> Result<(), FactoryResetError>;

    pub async fn issue_invitation(
        &self,
        input: IssueInvitationInput,
    ) -> Result<IssueInvitationResult, IssueInvitationError>;

    pub async fn cancel_invitation(&self) -> Result<(), CancelInvitationError>;

    pub async fn state(&self) -> Result<SpaceStateView, SpaceStateError>;
    pub async fn migration_progress(&self) -> Result<MigrationProgress, MigrationProgressError>;
    pub async fn membership_convergence(
        &self,
    ) -> Result<MembershipConvergenceStatus, MembershipError>;
}
```

保留独立的错误类型，不改现有稳定错误码映射。开发诊断用的地址列表和指定地址签发只在 `dev-tools` 下提供，不进入生产入口。

调用方不得再执行以下动作：

- 判断首次加入还是切换空间。
- 在解锁或恢复后启动成员传播。
- 在锁定前暂停成员传播。
- 在锁定失败后恢复后台任务。
- 为恢复会话分别调用加密解锁、空间恢复、搜索恢复和接收准备。
- 直接持有或查询 `SpaceMembershipGossip`。

## 4. 状态与并发

`SpaceSessionCoordinator` 保存一个串行状态转换锁，并公开只读状态：

```rust
pub enum SpaceRuntimeState {
    Uninitialized,
    Locked,
    Activating,
    Ready { space_id: SpaceId },
    Switching { phase: MigrationPhaseKind },
    RecoveryRequired,
    Stopping,
    Stopped,
}
```

必须满足：

- 同一时间只能有一个创建、解锁、恢复、锁定、加入、切换或重置动作。
- 只有 `Ready` 可以签发邀请、接受入站配对或执行成员传播。
- `Locked`、`RecoveryRequired`、`Stopping` 和 `Stopped` 下，不得有读取 MasterKey 的后台任务。
- 开始关闭后拒绝新的状态变化动作。
- 关闭幂等；所有应用任务都能等待结束，不依赖仅在 `Drop` 中强制取消。
- P2P 失败不得自动启用 LAN。

## 5. 完整流程

### 5.1 创建、解锁和恢复

统一顺序：

1. 串行化状态变化并进入 `Activating`。
2. 建立或恢复加密会话。
3. 续跑未完成的空间迁移。
4. 确认加密关系存储可读并完成必要迁移。
5. 执行移动端可消费数据补偿。
6. 打开接收准备状态。
7. 恢复搜索和成员传播。
8. 激活在线探测并做一次非阻断预连。
9. 状态变为 `Ready` 后返回。

迁移恢复、关系存储和接收准备失败会进入 `RecoveryRequired`。在线预连失败只形成降级结果和脱敏日志，不回滚已经提交的空间状态。

### 5.2 锁定

统一顺序：

1. 阻止新的配对、迁移和成员传播进入。
2. 暂停成员传播并等待在途操作退出。
3. 停止在线维护，关闭接收和搜索访问。
4. 锁定会话并清除内存密钥。
5. 状态变为 `Locked`。

若第 4 步失败，`SpaceActivityCoordinator` 按反向顺序恢复已暂停能力，并回到 `Ready`。调用方不参与补偿。

### 5.3 加入空间

`SpaceAdmissionCoordinator` 自己读取当前状态：

- 尚未完成 setup：执行首次加入。
- 已完成 setup：调用 `SpaceTransitionCoordinator` 完成带历史迁移的切换。

设备名校验和保存属于同一个加入动作。调用方不再传 `Fresh` / `Switch`，也不在调用前查询状态。

加入或切换成功后，通过 `SpaceSessionCoordinator` 的同一激活流程恢复接收、搜索、在线维护和成员传播。

### 5.4 重置和关闭

重置先静默全部依赖会话的活动，再擦除密钥和关系。密钥擦除失败时保留 setup 状态；成功后可以直接清空密文关系，无需先解密逐条读取。

`SpaceApplicationRuntime::shutdown` 负责停止：

1. 入站配对任务。
2. 成员传播与在线维护。
3. 其他空间应用任务。

随后 `uc-engine::SessionRuntime` 再关闭共享剪贴板、文件任务和 Iroh 节点。应用层内部任务的相对顺序不再出现在 `SyncEngineAssembly`。

## 6. 内部模块职责

### `SpaceSessionCoordinator`

负责空间运行状态、状态转换串行化、创建、解锁、恢复、锁定、重置和失败补偿。它不直接实现配对协议、迁移算法或成员传播。

### `SpaceAdmissionCoordinator`

负责邀请生命周期、Sponsor 入站任务、Joiner 握手、关系提交和配对结果事件。配对成功后只通过私有接口通知成员传播，不直接推进传播步骤。

### `SpaceTransitionCoordinator`

负责四阶段历史重加密、阶段持久化和崩溃续跑。切换入口只供 `SpaceAdmissionCoordinator` 使用；外部只有统一的 `join_space`。

### `SpaceMembershipGossip`

独自负责候选通讯录、公告、差异交换、安全更新、双向确认、正式关系提升、重试、恢复、在线维护和收敛状态。运行期的暂停、恢复和关闭只对 `SpaceActivityCoordinator` 可见。

### `SpaceActivityCoordinator`

它是私有的显式协调者，不使用无序的通用 participant 列表。字段按责任命名，以便固定顺序和失败策略：

```rust
pub(crate) struct SpaceActivityCoordinator {
    membership: Arc<dyn MembershipActivityPort>,
    connectivity: Arc<dyn PeerConnectivityActivityPort>,
    receive_readiness: Arc<dyn ReceiveReadinessPort>,
    search_session: Arc<dyn SearchSessionPort>,
}
```

这些接口都有生产实现和内存测试实现，是实际变化点，不是为减少字段数量虚构的转发层。

## 7. 依赖归属

原有能力不会凭空消失，但只进入真正使用它的模块：

| 责任 | 依赖 |
|---|---|
| 会话 | 初始化、解锁、恢复、锁定、setup 状态、关系存储就绪、活动协调者 |
| 加入配对 | 配对网络能力、邀请目录、证明、身份、关系提交、成员种子、设置、时钟 |
| 空间迁移 | 迁移日志、一次性迁移密钥、历史备份、内容加解密、关系重置、加入执行器 |
| 成员传播 | 候选/公告/待发送工作集、安全更新、成员签名、证明通道、原子关系提升、时钟 |
| 在线维护 | 正式地址、在线能力、本机设备编号、退避状态 |
| 统计 | 移回诊断或设置模块，不再属于空间模块 |

生产装配集中到 `crates/uc-engine/src/assembly/space_application.rs`。只有该文件看见全部模块的构造依赖。

`PairingHandlers` 是一个网络安装动作产生的真实能力组，可以整体交给加入模块。`SpaceAccessPorts` 也可以保留为同一安全实现的多种窄视图。不要新增 `SpacePlatformPort` 之类带大量 getter 的总接口。

## 8. 网络两阶段装配

Iroh 协议必须在节点启动前注册，这一约束不能隐藏成错误的单阶段构造。装配明确分两阶段，并集中在唯一文件：

```text
创建不启动后台任务的应用对象
  -> 把应用协议入口注册到 IrohNodeBuilder
  -> 启动 Iroh 节点
  -> 启动应用后台任务
  -> 返回 SessionRuntime
```

构造器不得直接 `spawn`。若节点启动或应用任务启动失败，`SessionRuntime` 负责关闭此前已创建的资源。

运行期开始后，`runtime/dispatch.rs` 只调用完整业务动作，不再出现成员传播的单独控制。

## 9. 错误与降级

每个用户动作保留独立错误类型。错误只表达用户可理解和调用方可处理的结果：

- 输入或状态拒绝。
- 口令错误或密钥损坏。
- 邀请无效、版本不兼容、连接失败或超时。
- 暂时不可用且可重试。
- 已提交但需要恢复。

底层阶段名和错误文本只进入脱敏日志。

关键规则：

- 持久化前失败：保持原状态。
- 已发生不可回滚改变、后续关键激活失败：进入 `RecoveryRequired`，不得伪装为完全未发生。
- 在线预热等非关键能力失败：返回成功并标记降级，由负责人自动重试。

## 10. 测试策略

### 外部行为测试

继续只通过稳定 `Engine` 验证：

- 创建、解锁、恢复、锁定、首次加入、切换和重置。
- 解锁后成员传播自动恢复。
- 锁定前在途传播已经退出。
- 锁定失败后所有活动自动恢复。
- 加入自动选择首次加入或切换。
- 关闭后没有遗留任务。
- P2P 失败不切换 LAN。

### 模块测试

- 会话测试只替换会话和四个高层活动接口。
- 配对测试只构造配对依赖。
- 迁移测试只构造迁移依赖。
- 成员传播测试继续覆盖真实状态机和本地替身。

删除直接在多个 Engine 测试中构造完整 `SpaceSetupDeps` 的做法。完整网络场景统一使用现有 Engine 测试驱动器。

### 架构检查

增加检查，禁止：

- `runtime/dispatch.rs` 调用成员传播的暂停或恢复。
- `uc-engine` 持有 `SpaceMembershipGossip` 或其运行期控制。
- `uc-engine` 判断首次加入或切换空间。
- `SpaceSetupDeps` 或同等平铺大包重新出现。
- 外部 crate 导入空间内部协调者。

## 11. 迁移顺序

1. 先补 Engine 行为测试，锁定当前成功、失败和稳定错误码。
2. 引入 `SpaceActivityCoordinator`，先接管解锁、恢复和锁定的完整顺序及补偿。
3. 把成员传播运行期移入 `SpaceApplicationRuntime`；删除 Engine 的 `pause_membership_gossip` / `resume_membership_gossip`。
4. 把搜索恢复、接收准备和在线维护接入同一活动协调者。
5. 把 Fresh/Switch 判断和设备名保存移入统一 `join_space`。
6. 提取 `SpaceTransitionCoordinator`，保持迁移数据格式和恢复语义不变。
7. 提取 `SpaceAdmissionCoordinator`，收拢邀请、入站任务和两端握手。
8. 把在线 keepalive 从空间设置移入成员传播模块，把统计身份重置移回诊断模块。
9. 用 `SpaceFacade` 替换 `SpaceSetupFacade`，删除 `EncryptionFacade` 中重复的初始化、恢复和锁定入口。
10. 删除 `SpaceSetupDeps`、旧转发方法、无关测试中的空实现和过渡构造路径。
11. 运行全仓测试、架构检查、三 Engine 自动化和实体设备矩阵。

每一步必须原地替换旧路径，不长期保留新旧两套实现。

## 12. 完成定义

- `runtime/dispatch.rs` 对每个空间动作只调用一次应用入口。
- `uc-engine` 不再控制成员传播的暂停、恢复和补偿。
- `SpaceMembershipGossip` 及其运行期不再被 `uc-engine` 直接持有或查询。
- 首次加入与空间切换由应用层自动决定。
- `SpaceSetupFacade`、`SpaceSetupDeps` 和重复的 `EncryptionFacade` 生命周期入口已删除。
- 新增成员传播依赖不会迫使无关空间或剪贴板测试增加空实现。
- 当前稳定 Engine operation、结果和错误码保持兼容。
- 自动化检查通过；未执行的实体设备项目明确记为跳过。
