# 任务计划：Space Application 用例全量梳理

## 目标

只梳理并完成 `uc-application/src/space` 及其 Facade 中所有面向用户或后台结果的行为，明确唯一负责人、入口、输入、结果、失败和恢复责任。Space Application 的用例和 Port 稳定前，不推进 infra；device、profile、clipboard、transfer、search、settings、support 全部不动。

## 当前阶段

阶段 2：逐项评估职责

## 阶段

### 阶段 1：建立全量行为清单

- [x] 扫描 Space 标准 `use_case.rs`、Facade 命令、Runtime 入口和旧负责人公开方法。
- [x] 按生命周期、准入、成员关系、连接与后台运行期分类。
- [x] 区分标准用例、内部流程、后台运行期、共享模块、Facade 和纯转发。
- [x] 为每项行为记录生产调用方与对外可见结果。
- **状态：完成**

### 阶段 2：逐项评估职责

- [ ] 明确谁负责完整结果。
- [ ] 明确调用方唯一动作、必要输入、成功结果和失败结果。
- [ ] 明确重启、重试、并发和通知由谁负责。
- [ ] 执行删除检查，识别纯转发和知识分散。
- [ ] 标记需要提取、合并、重命名或保持现状的项目。
- **状态：进行中**

### 阶段 3：收口 Space 生命周期

- [ ] 评估 initialize、unlock、lock、recover、reset、rebuild、upgrade。
- [ ] 明确 session readiness 与 activity 是内部流程还是独立用例。
- [ ] 删除生命周期旧入口、转发和并行实现。
- **状态：待开始**

### 阶段 4：收口 Space 准入

- [ ] 评估 issue invitation、redeem invitation、cancel invitation、join、cancel join。
- [x] 将握手、durable transaction 和 sponsor runtime 保持为用例内部流程。
- [x] 明确准入恢复与跨 Space 切换的唯一负责人。
- [ ] 删除准入旧入口、转发和并行实现。
- **状态：进行中；邀请行为与 Join 入口已收口，Join 结果、取消加入和恢复仍待处理**

#### 当前实施：拆分加入与待处理空间切换

- [x] `JoinSpaceUseCase` 返回已保存的加入状态和本次加入是否要求关闭当前会话。
- [x] 新建内部 `CompletePendingSpaceTransitionUseCase`，只在当前会话已关闭后推进并恢复已保存的跨空间加入。
- [x] Engine 只负责关闭旧会话和启动新会话，不再重新判断 Application 保存状态的含义。
- [x] 正常加入直接返回；跨空间加入完成切换后再返回最终活动状态。
- [x] 启动时对中断切换的恢复继续使用同一个内部完成入口。

#### 当前实施：删除 ProfileSpaceAdmission 总入口

- [x] 提取查询当前加入、取消加入和恢复完成确认三个标准用例。
- [x] `SpaceJoinFacade` 只组合 profile 范围内始终可用的加入能力。
- [x] `SpaceMembershipFacade` 只组合成员状态查询、发起移除、决定移除和活动 Space 接入。
- [x] 两个门面分别发布加入和成员变化，Engine 统一转换为既有产品通知，不复制状态或保存路径。
- [x] Engine 只依赖两个具体门面，删除 `ProfileSpaceAdmission`。

#### 已完成：拆分 durable flow

- [x] 取消加入迁入 `CancelSpaceJoinUseCase`。
- [x] 待处理空间切换判断迁入独立查询用例，完成推进归入完成切换用例。
- [x] 加入方可靠推进规则和内部接口迁入 `admission/joiner/`。
- [x] 邀请方可靠推进规则和内部接口迁入 `admission/sponsor/`。
- [x] 删除混合两侧全部步骤的 `durable/flow.rs` 和 `WorkspaceAdmissionOwnerPort`。
- [x] 共享事务与重启恢复继续由 `durable/` 所有。

#### 已完成：迁移准入 Ports

- [x] 准入尝试仓储和待发送消息投递接口迁入 `admission/durable/`。
- [x] 完成恢复通信接口迁入 durable 完成恢复模块。
- [x] 跨 Space 切换和安全状态切换接口迁入各自 admission 模块。
- [x] 加入方成员材料接口迁入 `admission/joiner/` 并删除无调用方法。
- [x] infra 直接实现 application 接口，Engine 从 `uc_application::deps` 组装。
- [x] core 删除旧接口，只保留准入模型、规则和成员历史验签接口。

### 阶段 5：收口 Space 成员关系

- [ ] 评估 query status、initiate removal、decide removal 和旧开发工具入口。
- [ ] 明确 membership history、state、runtime 是共享模块或后台运行期。
- [ ] 删除 `WorkspaceMembership` 中已迁出的行为和纯转发。
- [ ] 明确成员资料展示与成员资格事实的边界。
- **状态：待开始**

### 阶段 6：收口 Space 连接与运行期

- [ ] 评估 ensure reachable、network recovery 和 presence refresh。
- [ ] 明确观察输入、用户命令、状态查询与后台重试的归属。
- [ ] 明确 Space runtime 只负责启动、暂停、恢复和关闭哪些负责人。
- **状态：待开始**

### 阶段 7：稳定 Space Application 对外能力

- [ ] 每个用例只依赖必要的 application/core 能力。
- [ ] Facade 只暴露完整用户动作，不编排内部步骤。
- [ ] Engine 只通过 Facade，不直接看到用例。
- [ ] Application Port 的归属、错误和幂等语义固定。
- [ ] 用例级回归覆盖关键成功、失败和恢复边界。
- **状态：待开始**

### 阶段 8：逐步接入 Infra

- [ ] 按已稳定的 Application Port 逐个实现或迁移 Adapter。
- [ ] 删除 infra 中被取代的旧接口实现和转发层。
- [ ] 每次只接通一个最小端到端用例。
- [ ] 完成全量验证和原子提交。
- **状态：待开始**

## 梳理标准

每个候选行为必须回答：

1. 谁对完整结果负责？
2. 调用方唯一需要执行什么？
3. 输入是什么？
4. 成功返回什么？
5. 失败返回什么？
6. 重启、重试、并发和通知由谁负责？
7. 删除该模块后，复杂度会消失还是散回调用方？

## 当前约束

- 只处理 Space；其他 application 领域不分析、不修改。
- 暂不继续 infra 接线或迁移。
- 暂不运行构建、测试或会隐式触发构建的架构脚本。
- 不把文件数量当作完成标准，以行为所有权清晰为准。
- 不为内部步骤建立一一对应的公开接口。
- 不保留新旧两套生产实现。
