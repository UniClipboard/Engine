# 本地就绪与 Space 后台恢复统一责任

## 状态

- **状态**：核心实施完成；实体设备与产品宿主验收待执行
- **日期**：2026-09-19
- **来源问题**：Mobile 冷启动时，Engine 把远端设备连接与成员维护纳入公开启动完成条件，导致本地设备列表在 30 秒超时的整数倍后才出现
- **完整负责人**：Application 的 Space 会话恢复模块负责本地恢复、后台活动交接、暂停、锁定、恢复、失败重试和关闭；成员维护与设备连接继续分别负责自己的远端欠账和连接重试
- **调用方唯一动作**：Engine、Mobile、Desktop、CLI 和 Share Extension 只提交既有启动、解锁、恢复、暂停、锁定或关闭动作，不调用后台恢复步骤
- **成功结果**：本地数据库、密钥、身份、Space 与已知设备资料可读后，公开启动立即返回；后续活动由 Application 内部接管并最终进入运行、等待网络、暂停或明确失败状态
- **失败结果**：本地资料失败继续阻止本地就绪；远端离线或网络失败只延期远端工作；后台本地能力启动失败进入可取消的有限退避，不清空本地设备或把 Engine 退回启动失败
- **重试与重启责任**：Application 会话恢复负责人重试本地后台能力；成员维护和连接运行期重试各自工作；进程重启重新从持久事实恢复，不持久化新的启动状态

## 实施记录

- 2026-09-19：Application 已接管唯一后台激活任务，合并重复请求，并按 1、2、5、10、30 秒封顶重试本地活动启动。
- 2026-09-19：创建、解锁和已保存会话恢复都只提交一次后台激活；锁定和关闭会先取消并等待旧任务。
- 2026-09-19：成员维护不再作为同步启动屏障；会话活动只恢复并非阻塞唤醒它，远端延期仍由原运行期负责。
- 2026-09-19：Engine 的第二步调用、后台任务和布尔防重已删除；公开接口、持久格式和设备间协议未改变。
- 2026-09-19：Application 全部 943 个单元测试中 942 个通过、1 个按原标记跳过；完整仓库门禁见本次交付报告。实体设备与产品宿主验收未执行，保持“跳过”。

# 1. Overview

真实 Mobile 诊断显示，Engine 公开启动曾连续等待远端连接约 30 秒的超时，最终在约 90 至 123 秒后才允许产品读取设备列表；公开启动完成后，同一份本地三设备目录只需约 29 毫秒即可读取。根因是本地资料恢复、一次成员维护和其他 Space 活动被串联为同一个完成条件，远端设备离线、地址失效或网络不可用都会延长公开启动。

当前工作树中的初步修复已经证明：把远端等待移出公开启动后，三设备离线重启可以在 5 秒内返回并立即读到全部本地记录，随后联网恢复仍能完成。但是该实现把完整责任拆成两个调用：Application 返回本地恢复结果，Engine 再判断结果并单独启动 `resume_recovered_session_activity`。后台失败只记录后结束，没有统一重试；恢复与锁定也没有共同的串行边界，可能留下部分活动已开启或旧恢复覆盖新暂停的状态。

本规格保留已验证的产品边界：公开启动只等待本地资料可读。实现上不保留 Engine 中的第二步编排，而是在 Application 内建立唯一 Space 会话恢复负责人。该负责人不亲自实现成员同步或连接重试，只负责将本地会话安全交给现有搜索、接收、连接和成员运行期，并统一管理这次交接的去重、取消、失败恢复和生命周期顺序。

# 2. Goals

- 已有有效本地 Space 时，公开启动耗时不随远端设备数量、离线状态、DNS 失败、地址失败或 30 秒连接超时增长。
- 本地数据库、密钥、身份、Space 与已知设备目录可读后，现有公开启动返回；`ListDevices` 可立即返回完整本地记录。
- 首次使用、资料锁定、密钥损坏、数据库失败与“已有资料但远端离线”保持互不混淆的稳定结果。
- Application 只暴露一次完整恢复意图；删除 Engine 对“本地恢复成功后再启动内部活动”的步骤判断。
- 后台交接只启动一次；重复启动、显式 `RecoverSession`、快速前后台切换和并发唤醒只合并或唤醒当前工作。
- 搜索、接收、设备连接与成员维护的启动具有明确顺序、幂等要求和部分失败处理；一次暂时失败后可自动继续。
- 远端成员维护不再作为会话活动启动的同步屏障；成员欠账、地址恢复、配对恢复和周期维护继续由现有成员运行期负责。
- 锁定、暂停和关闭能够抢占或排在后台交接之前，完成后旧工作不能重新开启任何能力。
- Engine 关闭时等待 Application 恢复负责人实际停止；任务 panic、提前退出和重试失败可观察。
- 保留现有公开 `uc-engine`、UniFFI、HarmonyOS 接口、设备间协议、持久化格式、P2P 默认行为和错误编号。
- Mobile、Desktop、CLI 与 Share Extension 无需新增第二次恢复调用即可受益。

# 3. Non-Goals

- 不缩短远端连接、握手或成员同步超时来伪造快速启动。
- 不增加第二套公开“Engine 就绪”或“Space 就绪”状态；本地就绪继续由现有启动结果和查询表达。
- 不让 Engine、绑定或产品仓理解搜索、接收、连接和成员维护的内部顺序。
- 不把成员维护、地址恢复、配对恢复和连接管理合并为一个大型 use case。
- 不改变成员资格事实来源、成员历史规则、准入协议、连接在线判定或远端重试算法。
- 不增加持久化恢复阶段、数据库表、磁盘文件或迁移；后台交接状态只属于当前 Application 运行期。
- 不修改设备间消息格式、绑定接口或产品 UI 流程。
- 不把 P2P 失败自动降级到 LAN 兼容线。
- 不顺带重构发送会话、发送取消、剪贴板投递或文件传输恢复。
- 不以实体设备以外的测试替代 Mobile、Desktop 与 Share Extension 的最终产品验收；未执行项必须标记为“跳过”。

# 4. Current Architecture Context

```text
Component: RecoverSpaceSessionUseCase
Path: crates/uc-application/src/space/lifecycle/recover_space_session/
Responsibility: 用已保存密钥恢复当前 Space，并完成本地 readiness。
Relationship: 当前工作树已删除活动恢复；返回后由 Engine 再决定是否启动后台活动，完整责任被拆开。
```

```text
Component: PostSessionReadiness
Path: crates/uc-application/src/space/lifecycle/unlock_space/readiness.rs
Responsibility: 完成资料升级、移动内容回填和成员目录可读检查。
Relationship: 当前工作树已经不再等待成员网络维护；这一“只验证本地资料”的边界应保留并明确命名。
```

```text
Component: SpaceSessionActivity / CombinedSpaceSessionActivity
Path: crates/uc-application/src/space/lifecycle/session/activity.rs
Responsibility: 按顺序恢复或暂停搜索、接收、设备连接与成员活动，并在锁定失败时补偿。
Relationship: 当前 `resume_after_session_ready` 先等待成员准备，再逐项恢复其他能力；中途失败没有统一重试或恢复一半状态的处理。
```

```text
Component: SpaceMembershipMaintenanceRuntime
Path: crates/uc-application/src/space/membership/maintenance/runtime.rs
Responsibility: 合并触发、串行成员维护、处理上线和网络事件、周期重试、暂停和关闭。
Relationship: 它已经是成员远端恢复的唯一负责人；会话恢复只应恢复并唤醒它，不应等待一次远端维护完成。
```

```text
Component: PeerConnectionCoordinator
Path: crates/uc-application/src/space/connectivity/peer_connections/
Responsibility: 管理已知设备连接机会、退避、在线变化、暂停、恢复和关闭。
Relationship: 会话恢复只负责开启这个既有负责人，不复制拨号、超时或重试策略。
```

```text
Component: SpaceFacade / AppFacade
Path: crates/uc-application/src/space/facade/facade.rs
Path: crates/uc-application/src/facade/app_facade.rs
Responsibility: 暴露完整 Space 意图和稳定结果。
Relationship: 当前工作树新增了步骤级 `resume_recovered_session_activity`；目标设计删除该入口，不允许 Engine 拼接恢复步骤。
```

```text
Component: SessionSupervisor / ProductionSession
Path: crates/uc-engine/src/runtime/session_supervisor.rs
Path: crates/uc-engine/src/runtime/session_supervisor/
Responsibility: 组装和持有当前生产会话，处理会话替换、暂停、恢复和关闭。
Relationship: 当前工作树在 Engine TaskRegistry 中另起后台活动并用布尔值去重；目标设计删除该业务步骤，只保留对 Application 完整生命周期动作的调用。
```

```text
Component: Engine operation dispatch
Path: crates/uc-engine/src/runtime/dispatch.rs
Responsibility: 把公开操作投影到稳定 Application 能力。
Relationship: 当前 `RecoverSession` 分支检查内部结果后再启动后台步骤；目标设计恢复为一次调用和一次稳定结果投影。
```

当前工作树流程：

```text
Engine 安装会话
  -> Application 恢复本地 Space
  -> 本地 readiness 完成
  -> Engine 检查 unlocked/resumed
  -> Engine TaskRegistry 启动后台 future
  -> Application 依次等待成员准备、搜索、接收、连接和成员恢复
  -> 失败只写日志，任务结束
```

问题边界：

- Engine 知道 Application 内部恢复分段，并负责第二步调用。
- 后台任务成功启动不等于活动最终运行；失败没有自动重试入口。
- 顺序中任何一步失败都可能保留前一步副作用。
- 后台恢复没有与 `lock_space_session` 共用同一个所有权边界。
- `AtomicBool` 只能防止同一时刻重复运行，不能表达暂停、关闭、旧请求失效或待重试。
- 设计文档仍描述恢复会同步等待成员维护，与目标行为和当前工作树均不一致。

# 5. Proposed Design

## Components

### `SpaceSessionRecoveryRuntime`

- **职责**：Application 内唯一管理 Space 会话活动交接的运行期；拥有激活、暂停、恢复、锁定前停用和关闭的串行顺序。
- **输入**：`LocalSessionReady`、`Pause`、`Resume`、`Lock`、`Shutdown` 等内部命令，以及当前运行期取消信号。
- **输出**：本地交接是否已接受；内部活动状态；关闭报告。状态不进入公开 Engine 接口。
- **关系**：调用 `SpaceSessionActivity` 的完整激活或暂停能力；不理解成员维护内部步骤、目标设备、网络地址或连接尝试。

该运行期属于 `ApplicationRuntime`，持有自己的任务句柄和取消能力，并由 Application shutdown 取消和等待。不得把它登记到 Engine 的 `TaskRegistry` 作为业务 owner。Engine 仍通过 Application 完整 shutdown 间接等待它结束。

运行期使用单一受管任务和有界命令通道。命令只表达完整意图，不携带步骤：

- `Activate`：本地会话已可读，确保活动最终运行；重复命令合并。
- `Pause`：取消当前激活或重试，等待活动进入暂停状态。
- `Resume`：等价于当前会话的 `Activate`，但只在未关闭时接受。
- `Shutdown`：拒绝新命令，取消当前工作，等待活动停用后结束。

命令队列不得按请求数量无限增长。`Activate` 和 `Resume` 只保留一个待办；`Pause`、`Lock`、`Shutdown` 优先于激活和重试计时。

### `RecoverSpaceSessionUseCase`

- **职责**：继续作为“从已保存资料恢复 Space”的完整负责人；完成本地恢复后向 `SpaceSessionRecoveryRuntime` 提交一次 `Activate`，不等待远端工作。
- **输入**：当前 Space、已保存会话、安全材料和本地 readiness 能力。
- **输出**：现有 `RecoverSpaceSessionResult { unlocked, resumed }`；本地失败保留现有稳定错误。
- **关系**：调用方只调用该 use case；后台交接是负责人内部动作，不通过 facade 暴露第二步。

`Activate` 的“请求已接受”属于本地恢复成功的一部分：如果 Application 运行期已经关闭或无法接管请求，恢复不能返回一个虚假的可持续会话。请求接受后发生的搜索、接收、连接或远端失败不反写公开启动结果。

### `LocalSessionReadiness`

- **职责**：只证明公开本地就绪边界：版本升级完成、必要内容回填完成、成员目录在当前安全会话下可读。
- **输入**：本地升级、回填和成员仓储能力。
- **输出**：成功或保留来源链的本地错误。
- **关系**：由自动恢复和解锁复用；不得调用成员网络维护、拨号、DNS、地址刷新或远端协议。

现有 `PostSessionReadiness` 应按最终职责重命名，避免名称暗示它包含任意“session 后置活动”。

### `SpaceSessionActivity`

- **职责**：提供幂等的完整 `activate`、`pause` 和失败补偿；隐藏搜索、接收、连接、成员运行期的具体顺序。
- **输入**：无步骤级外部输入；使用已注入的内部能力。
- **输出**：带来源链的活动错误，以及足以让 recovery runtime 决定重试或停止的稳定分类。
- **关系**：只由 `SpaceSessionRecoveryRuntime`、初始化和锁定流程的完整负责人使用；Engine 不持有或调用。

激活顺序固定为：

1. 再次确认安全会话仍可用；失效则结束本次激活。
2. 恢复只依赖本地资料的搜索活动。
3. 确保接收入口可用。
4. 恢复设备连接协调者。
5. 恢复成员维护运行期并提交一次非阻塞唤醒。

第五步只确认成员运行期接受了恢复和唤醒，不等待任何远端设备连接或一轮成员维护完成。成员维护的 Deferred、StableFailure、网络退避和周期重试继续由 `SpaceMembershipMaintenanceRuntime` 负责。

激活中的每一步必须满足以下至少一项：幂等重复、可查询已完成、或失败时有补偿。目标实现优先让 `activate` 可安全重复，并在暂时失败后由 recovery runtime 重跑完整激活。若某一步成功后另一项失败不会造成安全扩大，可以保留成功部分；若可能在锁定状态开放能力，必须立即补偿停用。

### 生命周期调用方

- `initialize_space`：本地事实提交后请求当前会话激活；是否等待激活接受或完成必须与现有稳定结果兼容，不等待远端。
- `unlock_space`：本地安全会话和 readiness 成功后请求激活，不等待远端。
- `recover_space_session`：同上，公开启动以本地 readiness 为边界。
- `lock_space_session`：先通过 recovery runtime 取消激活与重试并完整暂停活动，再锁定安全会话；锁定失败时由同一负责人恢复活动。
- `suspend` / session replacement / shutdown：停止 Application runtime 时取消并等待 recovery runtime；旧运行期结束后才发布后继会话。

## Data Model

后台交接状态只存在内存中，不持久化：

```text
enum SpaceSessionRecoveryState {
    Dormant,
    Activating { attempt },
    Active,
    WaitingRetry { attempt, deadline },
    Pausing,
    Paused,
    Stopping,
    Stopped,
}
```

- `attempt`：当前 Application 运行期内单调增加的诊断计数，不进入日志业务字段、公开接口或持久化。
- `deadline`：使用单调时钟，只控制本进程内下一次本地活动激活重试。
- 运行期实例本身就是会话代际边界。会话替换必须先等待旧 `ApplicationRuntime` shutdown，因此不再需要 Engine 维护另一个 `recovered_activity_running` 或公开代际。

远端成员欠账、准入状态、地址和重试期限继续使用现有加密持久事实；本规格不复制它们。

## API / Interface

内部接口表达完整意图，示意名称如下；实现时遵守仓库命名和可见性检查，不照抄为公共 API：

```text
SpaceSessionRecoveryRuntime::request_activation() -> Result<(), RecoveryUnavailable>
SpaceSessionRecoveryRuntime::pause() -> Result<(), SpaceActivityError>
SpaceSessionRecoveryRuntime::restore_after_failed_lock() -> Result<(), SpaceActivityError>
SpaceSessionRecoveryRuntime::shutdown(deadline) -> Result<(), LifecycleError>
```

要求：

- `request_activation` 只等待请求被当前运行期接受，不等待活动完成或远端结果。
- `pause` 在返回前保证当前激活和重试不能再开启活动，并等待已开启部分进入暂停状态。
- `shutdown` 幂等；首次调用拥有实际收尾，重复等待者取得同一最终结果。
- 暂时活动失败必须保留来源链并映射为固定内部分类；禁止字符串化错误。
- 删除 `SpaceFacade::resume_recovered_session_activity`、`AppFacade::resume_recovered_session_activity` 和 Engine 对应调用。
- 公开 `RecoverSession` 操作和结果类型保持不变。

## Workflow

### 已有 Space 的冷启动

1. Engine 构造 Application 运行期和唯一恢复负责人。
2. Engine 调用一次 `recover_space_session`。
3. use case 恢复安全会话，执行 `LocalSessionReadiness`。
4. use case 向当前 Application recovery runtime 提交 `Activate`。
5. recovery runtime 接受请求后，use case 返回本地恢复结果。
6. Engine 重新开放本地操作；Mobile 可以立即读取设备列表。
7. recovery runtime 在后台激活搜索、接收和连接，并恢复、唤醒成员维护。
8. 远端离线产生延期事实，由成员和连接运行期继续处理，不影响本地可用。

### 首次使用

1. use case 查不到当前 Space，返回现有未恢复结果。
2. 不提交 `Activate`，后台恢复保持 Dormant。
3. setup 查询继续向产品表达尚未建立 Space。

### 暂时激活失败

1. `SpaceSessionActivity::activate` 返回带来源的可重试错误。
2. recovery runtime 记录固定失败分类，进入有限退避。
3. 相同期间的重复 Activate 只唤醒或合并，不创建第二个任务。
4. 退避到期、明确 Resume 或相关本地能力可用时重试。
5. Pause、Lock 或 Shutdown 抢占计时并阻止重试。

退避复用仓库已有的小型有界序列，建议 1、2、5、10、30 秒封顶；加入小幅抖动仅在已有依赖可直接复用时实施。远端设备离线不进入此重试，因为连接与成员运行期已有自己的退避。

### 锁定

1. 锁定负责人请求 recovery runtime 暂停。
2. runtime 取消当前激活/重试，等待 `SpaceSessionActivity::pause` 完成。
3. 确认活动不会重新开启后，锁定安全会话。
4. 锁定成功保持 Paused；锁定失败由同一负责人请求恢复，不从 facade 或 Engine 拼接补偿。

### 暂停、会话替换和关闭

1. SessionSupervisor 继续调用 Application 的完整暂停或关闭入口。
2. Application runtime 先阻止新的恢复请求，再取消并等待 recovery runtime。
3. recovery runtime 停止活动并返回最终结果。
4. Application 继续关闭其余任务和资源。
5. 旧 Application runtime 完全结束后，Engine 才安装或发布下一会话。

# 6. Implementation Plan

## Step 0：冻结回归并移除错误假设

- **File**：`crates/uc-engine/tests/space_membership_auto_pairing_e2e/automatic_connections.rs`
- **Change**：保留当前“离线成员不阻塞重启、5 秒内启动、三条本地记录立即可读、随后能恢复连接与传输”的端到端测试；先补失败注入，证明当前后台失败不会自动重试，以及当前锁定与后台恢复存在竞态。
- **Risk**：真实网络测试可能受时机影响；失败注入必须落在稳定内部边界，不靠缩短生产超时。

## Step 1：建立 Application 内部恢复运行期的最小闭环

- **File**：`crates/uc-application/src/space/lifecycle/session/` 下按现有模块风格新增恢复运行期文件；更新 `mod.rs` 仅声明和必要导出。
- **Change**：实现单一 owner、受管任务、有界/合并命令、状态转换、取消和 shutdown；先用假活动能力验证 Activate、重复 Activate、暂时失败重试、Pause 抢占和 Shutdown 等待。
- **Risk**：不能建立第二套 Application 总生命周期；该运行期必须由现有 ApplicationRuntime 构造和关闭。

## Step 2：把会话活动改为可重复的完整能力

- **File**：`crates/uc-application/src/space/lifecycle/session/activity.rs`，必要时按职责拆分同目录文件。
- **Change**：将成员维护的同步 `prepare_for_session` 从激活屏障中删除；定义幂等 activate/pause/restore 顺序和稳定失败分类；成员活动只恢复并非阻塞唤醒现有维护运行期。
- **Risk**：搜索、接收或连接若不支持重复恢复，必须先在各自 owner 中补齐幂等合同，不能在 recovery runtime 用布尔值猜测完成。

## Step 3：由恢复 use case 完成后台交接

- **File**：`crates/uc-application/src/space/lifecycle/recover_space_session/`、`unlock_space/`、`initialize_space/` 及 `space/application.rs`。
- **Change**：向完整负责人注入 recovery runtime 的窄意图接口；本地 readiness 成功后提交 Activate；统一初始化、解锁和自动恢复的活动交接语义，保留各自稳定结果。
- **Risk**：不能让首次使用或 keyring miss 提交 Activate；请求未被运行期接受时不能伪报可持续恢复成功。

## Step 4：删除 Engine 的步骤级后台编排

- **File**：`crates/uc-application/src/space/facade/facade.rs`、`crates/uc-application/src/facade/app_facade.rs`、`crates/uc-engine/src/runtime/dispatch.rs`、`crates/uc-engine/src/runtime/session_supervisor.rs`、`crates/uc-engine/src/runtime/session_supervisor/recovery.rs`。
- **Change**：删除 `resume_recovered_session_activity` 转调、Engine TaskRegistry 后台任务、`recovered_activity_running` 和结果分支；Engine 恢复路径重新只调用一次完整 Application 动作。
- **Risk**：必须确认显式 `RecoverSession`、启动安装和 transition 后恢复都覆盖，不能遗漏第二个调用点或保留双路径。

## Step 5：接通锁定、暂停、恢复和关闭

- **File**：`crates/uc-application/src/space/lifecycle/lock_space_session/`、Application runtime 生命周期代码、Engine session 生命周期验收。
- **Change**：所有活动暂停与恢复通过唯一 recovery runtime；锁定等待后台激活停止；Application shutdown 取消并等待恢复 owner；删除旧 facade 中重复的补偿顺序。
- **Risk**：取消等待不能在持有安全会话或仓储锁时发生；截止时间到达后原 owner 仍负责实际收尾，不能把等待超时当作资源已释放。

## Step 6：补齐失败、竞态和产品入口测试

- **File**：Application session lifecycle 测试、Engine host contract、`space_membership_auto_pairing_e2e`，必要时现有移动验收宿主。
- **Change**：覆盖第 8 节矩阵；用可控阻塞证明 Pause/Lock/Shutdown 优先，用连续失败证明自动重试和单实例，用真实本地目录证明列表不消失。
- **Risk**：不要用 sleep 猜时序；测试应使用屏障、通知或故障注入确认具体边界。

## Step 7：同步架构与调用者契约

- **File**：`docs/design-docs/space-application.md`、`docs/architecture/architecture-bible.md`，若公开启动语义已有专门接口文档则同步更新。
- **Change**：把 `PostSessionReadiness`、`RecoverSpaceSessionUseCase` 和成员维护的旧同步等待描述改为最终责任；写明本地就绪与后台远端恢复的边界，删除 Engine 持有 Application 内部活动的描述。
- **Risk**：文档只能记录已经落地并验证的行为；实施前本计划保持“待实现”。

## Step 8：分层验证与下游验收

- **File**：无生产修改。
- **Change**：先跑 Application 定向测试，再跑 Engine 定向和真实三设备测试，最后执行 workspace、格式、Rust 风格、仓库结构和 diff 检查；用标准 Engine 来源构建 Mobile 做实体设备矩阵。
- **Risk**：本机测试不能代替实体设备；构建、安装和启动必须分别记录结果。

# 7. Edge Cases

```text
Scenario: 首次使用没有当前 Space。
Expected behavior: 返回未初始化；不启动任何 Space 后台恢复。
Implementation: 只有本地恢复得到 unlocked/resumed 后才提交 Activate。
```

```text
Scenario: 已有三台设备，其他设备全部离线或地址失效。
Expected behavior: 本地启动与设备数量、连接超时无关；列表立即包含全部本地记录，远端工作进入延期。
Implementation: LocalSessionReadiness 不调用网络；连接和成员运行期各自记录待办并重试。
```

```text
Scenario: DNS、网络接口或中继暂时不可用。
Expected behavior: Engine 保持本地可用；不会出现 30 秒倍数的公开启动等待；网络恢复后自动继续。
Implementation: 网络错误留在连接 owner，不上升为本地 readiness 失败。
```

```text
Scenario: 搜索恢复成功，接收恢复暂时失败。
Expected behavior: 状态明确进入 WaitingRetry；重复激活安全，不留下无法继续的半恢复状态。
Implementation: activate 步骤幂等；需要安全回退的步骤立即补偿，其余由下一轮继续。
```

```text
Scenario: 连续两次本地活动激活失败，第三次成功。
Expected behavior: 只有一个恢复 owner；按有界退避自动重试，最终进入 Active。
Implementation: 单任务状态机持有 retry deadline，重复请求只合并或提前唤醒。
```

```text
Scenario: 后台激活期间执行 LockSpaceSession。
Expected behavior: Lock 取消并等待激活，所有活动暂停后才锁定；旧激活不能随后重新开启能力。
Implementation: Lock 和 Activate 进入同一 owner，Pause/Lock 优先并使旧工作失效。
```

```text
Scenario: 快速 suspend/resume/suspend 或重复 Resume。
Expected behavior: 不产生并行激活，不丢失最终暂停意图；恢复后只存在当前运行期的一份活动。
Implementation: 合并命令和明确优先级；Application runtime shutdown 后拒绝所有旧命令。
```

```text
Scenario: shutdown 与重试期限同时到达。
Expected behavior: shutdown 优先，不开始新一轮激活；实际任务被等待并结算。
Implementation: select 和状态转换固定停止优先，复用现有共同截止时间语义。
```

```text
Scenario: Application 后台任务 panic 或提前退出。
Expected behavior: 失败进入生命周期报告，不能继续表现为 Active；后继策略由 owner 明确决定。
Implementation: 持有并等待 JoinHandle，保存稳定失败；禁止丢弃永久任务句柄。
```

```text
Scenario: 本地数据库、密钥或成员资料损坏。
Expected behavior: 本地 readiness 失败关闭，不显示为空列表，不启动远端恢复，也不无限重试。
Implementation: 保留现有稳定分类与来源链，损坏不降级为网络延期。
```

```text
Scenario: 旧版本调用者认为 start 返回代表所有远端在线。
Expected behavior: 不新增兼容等待；调用者改用现有设备在线状态判断具体远端是否可用。
Implementation: 审计 Mobile、Desktop、CLI、Share Extension；只有真实依赖时更新其契约和测试，不在 Engine 恢复同步等待。
```

# 8. Testing Strategy

## Unit Test

1. **单次激活**：提交 Activate；断言活动只调用一次并进入 Active。
2. **重复合并**：激活阻塞时连续提交多个 Activate/Resume；断言只有一个活动 future。
3. **失败重试**：前两次返回可重试错误，第三次成功；断言退避顺序、最终 Active 和无并行调用。
4. **不可重试失败**：返回损坏/失效分类；断言不循环重试，状态和失败可观察。
5. **Pause 抢占**：激活等待中提交 Pause；断言旧工作被取消、pause 完成后不再调用 resume。
6. **Shutdown 优先**：retry deadline 与 shutdown 同时就绪；断言不开始新激活并等待任务退出。
7. **部分失败**：搜索成功、接收失败；断言重试或补偿遵守最终合同。
8. **运行期关闭**：关闭后提交 Activate；断言明确拒绝且不创建任务。
9. **错误来源**：每种活动失败转换保留 `source()`，不只断言显示文字。

## Integration Test

1. **自动恢复**：已有本地 Space，成员维护端口永久等待；公开恢复在短期限内返回，设备目录可读。
2. **无 Space**：空 profile 启动；返回未设置且活动调用次数为零。
3. **显式 RecoverSession**：操作只调用一次 Application 完整入口，后台交接由 Application 内部发生。
4. **Lock 竞态**：活动在每个边界可控阻塞时锁定；锁定返回后搜索、接收、连接和成员均保持暂停。
5. **Session replacement**：旧 Application 激活阻塞时替换会话；旧运行期完整停止，新运行期只激活一次。
6. **Application panic/early exit**：恢复 owner 异常结束；shutdown 报告失败且没有孤儿任务。
7. **网络恢复**：启动时无网络，恢复网络后在线状态自动变化并完成一次真实传送。

## Regression Test

1. **三设备全部离线**：现有 `offline_member_does_not_block_another_members_restart` 保持 5 秒公开启动门、三条记录立即可读和后续连接/传送成功。
2. **30 秒阶梯故障**：故障注入让一个或多个远端连接保持真实生产超时；公开启动不得等待单次或多次超时。
3. **部分在线**：一台在线、一台离线；本地列表完整，在线设备正常恢复，离线设备不阻塞。
4. **无网络/DNS/地址失败**：分别注入并重复十次冷启动；启动时长不形成 30/60/90/120 秒阶梯，记录不消失。
5. **快速前后台**：连续暂停、恢复、暂停；无重复任务、无旧活动复活、最终状态正确。
6. **现有创建和解锁**：新建 Space、密码解锁、错误密码、keyring miss、损坏资料保持稳定结果。
7. **发送与接收**：本地就绪后立即发起操作；未激活能力按现有可重试不可用结果返回，活动完成后成功，不崩溃或丢资料。
8. **关闭矩阵**：普通关闭、短截止时间、活动阻塞、任务 panic；实际停止和等待结果符合统一生命周期合同。

## Product / Device Acceptance

1. 使用标准固定 Engine 来源分别构建 Mobile、Desktop 与 Share Extension 相关宿主。
2. 对 Mobile 分别记录构建、安装和实际启动；三者不能互相代替。
3. 真实三设备 Space 中关闭两台，冷启动剩余设备，测量首屏本地列表时间并确认三条记录保留。
4. 在 Wi-Fi、蜂窝、飞行模式、DNS 失败、旧地址和网络切换下各重复启动；恢复网络后确认在线状态和发送。
5. 验证快速前后台、锁屏、系统暂停、Share Extension 冷启动和最终退出。
6. 未执行的平台、设备或网络场景逐项标记“跳过”。

# 9. Acceptance Criteria

* [ ] Engine 与本仓绑定已只执行一次既有启动/恢复动作；Mobile、Desktop、CLI 与 Share Extension 产品仓仍待实机验收确认。
* [x] `SpaceFacade::resume_recovered_session_activity`、AppFacade 转调、Engine 后台恢复任务和 `recovered_activity_running` 已删除。
* [x] Application 内只有一个 Space 会话恢复 owner，持有任务、取消、重试、暂停和关闭责任。
* [x] LocalSessionReadiness 不调用任何远端连接、成员传输、DNS、地址刷新或网络超时能力。
* [x] 已有本地 Space 且所有远端离线时，公开启动在 5 秒测试门内完成，设备列表立即包含全部本地记录。
* [ ] 单次及多次生产连接超时不会增加公开启动耗时，不出现 30/60/90/120 秒阶梯。
* [x] 首次使用不启动 Space 后台恢复，产品仍得到“尚未设置”的稳定状态。
* [x] 本地数据库、密钥或成员资料损坏继续阻止本地就绪，不降级为空列表或网络延期。
* [x] 搜索、接收、连接和成员运行期的激活可安全重复，部分失败有明确补偿或续跑规则。
* [x] 可重试的本地激活失败会自动重试；远端离线由现有成员和连接 owner 重试，不形成双重退避。
* [x] 重复 Activate、RecoverSession 和 Resume 不产生并行后台恢复。
* [x] Lock、Pause 和 Shutdown 能抢占激活/重试；返回后旧工作不能重新开启活动。
* [x] 会话替换先完整停止旧 Application 恢复 owner，再激活新会话。
* [x] 后台任务 panic、提前退出和关闭超时进入既有生命周期报告，失败可观察且不包含敏感值。
* [ ] Mobile、Desktop、CLI 与 Share Extension 调用者审计完成；没有调用者继续把 start 返回解释为所有远端在线。
* [x] Application 定向测试、Engine 定向测试、三设备端到端测试和第 8 节可在本仓自动执行的故障矩阵通过。
* [x] `cargo metadata --locked --format-version 1` 通过。
* [x] `cargo check --workspace --all-targets --locked` 通过。
* [x] `cargo fmt --all -- --check` 通过。
* [x] `node scripts/architecture/check-rust-style.mjs` 通过。
* [x] `node scripts/architecture/check-engine-repository.mjs` 通过。
* [x] `git diff --check` 通过。
* [ ] `docs/design-docs/space-application.md` 与 `docs/architecture/architecture-bible.md` 已按最终实现同步。
* [ ] 实体设备的构建、安装和启动分别记录；未执行项明确标记为“跳过”。

# 10. Risks and Trade-offs

## 技术风险

- **现有活动并非全部幂等**：直接重试可能重复开启任务。实施前必须逐个确认搜索、接收、连接和成员 resume 合同，缺口在各自 owner 中修复。
- **取消安全**：丢弃一个等待中的 future 不代表它提交的命令没有执行。恢复 owner 必须使用受管命令和确认边界，不能只在外层 `select!` 后假设下层停止。
- **锁定竞态**：如果 Pause 和 Activate 不在同一 owner，旧活动仍可能在锁定后复活；该竞态是本规格的硬验收门。
- **双重重试**：会话恢复只重试本地活动激活；成员和连接失败必须留给各自运行期，否则会出现重试风暴。
- **启动后立即操作**：公开启动提前返回后，发送、搜索或接收能力可能仍在激活。调用者必须得到现有稳定的暂时不可用结果，并通过既有刷新信号重试，不能重新把远端等待塞回 start。

## 性能影响

- 公开启动减少远端等待，只保留本地读取与一次内存命令交接。
- 新增一个 Application 受管任务和小型有界命令通道，常驻成本固定，不随设备数量增长。
- 后台激活失败会产生有界重试；必须复用现有计时与合并策略，避免空转。

## 维护成本

- 新增 recovery runtime 增加一个内部状态机，但删除 Engine 中的业务步骤、布尔防重和 facade 转调后，总责任更集中。
- 需要一次性迁移初始化、解锁、自动恢复和锁定调用；不保留新旧双路径。

## 替代方案

- **继续使用 Engine TaskRegistry**：实现较少，但 Engine 必须理解 Application 内部步骤，失败、锁定和重试责任仍分裂；拒绝。
- **让 RecoverSpaceSessionUseCase 同步等待全部活动**：责任集中，但重新引入远端阻塞；拒绝。
- **只把成员维护改为 fire-and-forget**：可以缓解当前超时，但搜索、接收、连接的失败和锁定竞态仍无 owner；拒绝。
- **为每个活动建立公开状态和调用**：会形成多套就绪来源并把复杂度泄露给产品；拒绝。

# 11. Open Questions

1. 搜索、接收和连接现有 `resume` 是否都已经满足幂等重复合同，需要在实施 Step 2 中用代码和测试逐项确认；若不满足，只在对应 owner 内补齐，不在恢复运行期维护重复的完成位图。
2. 公开启动返回后立即发起发送时，当前稳定错误和刷新事件是否足以让 Mobile、Desktop、CLI 与 Share Extension自然重试，需要在实施 Step 6 的调用者审计中确认；若某调用者隐含等待 start 代表远端完成，应更新该调用者测试和契约，而不是恢复同步等待。
3. Application 已有生命周期协调器是否能直接承载这组状态转换，还是需要在 `lifecycle/session/` 内新增专属 runtime，需要在最小闭环实现前确认。选择标准是：不建立第二套总生命周期，不让 Engine 持有 Application 内部任务。
