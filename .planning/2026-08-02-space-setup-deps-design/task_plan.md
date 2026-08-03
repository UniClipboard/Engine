# Space Setup 依赖收口重构计划

## Goal

在保持稳定入口、结果和错误码不变的前提下，将空间会话、加入、切换和成员传播的完整流程收回应用层，让引擎层不再掌握内部顺序。

## Completion Criteria

- 每个空间动作在引擎层只调用一次应用入口。
- 解锁、恢复、锁定和失败补偿由同一个会话负责人完成。
- 首次加入与切换空间由应用层自动选择。
- 成员传播及其后台任务不再由引擎层直接持有或控制。
- 删除旧的 `SpaceSetupFacade`、`SpaceSetupDeps` 和重复生命周期入口，不保留两套实现。
- 稳定 Engine operation、结果和错误码保持兼容。
- 自动化检查通过；未执行的实体设备项目明确记为跳过。

## Current Phase

全部完成

## Phases

### Phase 0：设计与 checkpoint

- [x] 审计当前职责和复杂度外溢位置
- [x] 比较三种设计并确定推荐方案
- [x] 完成依赖归属、状态、错误和迁移顺序设计
- [x] 将成员自动配对成果提交为独立 checkpoint `704471d`
- **Status:** complete

### Phase 1：固定现有行为

- [x] 通过稳定 Engine 入口覆盖创建、解锁、恢复、锁定和重置
- [x] 覆盖首次加入与切换空间的自动选择及稳定错误码
- [x] 覆盖锁定前停止在途活动；确认当前生产锁定动作不会失败
- [x] 覆盖关闭后无遗留任务和 P2P 失败不切换 LAN
- **Status:** complete

### Phase 2：收口会话活动顺序

- [x] 先以可注入失败测试固定锁定失败后的完整活动恢复
- [x] 引入私有 `SpaceActivityCoordinator`
- [x] 将解锁、恢复和锁定的活动顺序及失败补偿移入应用层
- [x] 删除 `runtime/dispatch.rs` 中对成员传播的单项控制
- [x] 以 Phase 1 行为测试确认对外结果不变
- **Status:** complete

### Phase 3：收口应用运行期

- [x] 引入 `SpaceApplicationRuntime` 统一持有空间后台任务
- [x] 保持 Iroh 注册后启动的两阶段装配约束
- [x] 删除引擎层对成员传播对象及其运行期的直接持有
- [x] 固定应用任务先停、共享网络节点后停的关闭顺序
- **Status:** complete

### Phase 4：统一加入与空间切换

- [x] 将首次加入或切换空间的判断移入统一 `join_space`
- [x] 将设备名校验和保存纳入同一完整动作
- [x] 提取 `SpaceTransitionCoordinator`，保持迁移格式和恢复语义不变
- [x] 删除调用方传入的 Fresh/Switch 模式和重复分支
- **Status:** complete

### Phase 5：收口邀请与配对

- [x] 提取 `SpaceAdmissionCoordinator`
- [x] 收拢邀请生命周期、入站任务和 Sponsor/Joiner 两端握手
- [x] 配对完成只通过私有接口通知成员传播
- [x] 保持关系提交、恢复种子和版本兼容语义不变
- **Status:** complete

### Phase 6：整理成员与周边责任

- [x] 将在线维护完整归入成员传播模块
- [x] 将统计身份重置移回诊断或设置责任
- [x] 让无关空间与剪贴板测试不再构造成员传播依赖
- [x] 以新模块接口替换旧的内部测试表面
- **Status:** complete

### Phase 7：切换唯一入口并删除旧实现

- [x] 用 `SpaceFacade` 替换 `SpaceSetupFacade`
- [x] 删除 `EncryptionFacade` 中重复的生命周期入口
- [x] 删除 `SpaceSetupDeps`、旧转发方法、空实现和过渡构造路径
- [x] 确认仓库只剩一套完整流程
- **Status:** complete

### Phase 8：回归与架构验收

- [x] 增加防止职责重新外溢的架构检查
- [x] 运行聚焦测试、全仓检查和三 Engine 自动化场景
- [x] 核对稳定入口、结果和错误码兼容性
- [x] 实体设备未执行项明确记为跳过
- **Status:** complete

## Errors

| Error | Attempt | Resolution |
|---|---:|---|
| 聚焦 lib 测试使用 `--exact` 后匹配 0 个测试 | 1 | 测试名包含模块前缀，改用普通名称过滤重跑，不把 0 tests 计为通过 |
| 一次 `cargo test` 传入两个独立测试名被拒绝 | 1 | 改用共同名称过滤 `membership_gossip::tests::runtime_` 一次运行两条测试 |
| Phase 2 首次编译缺少成员活动类型导出 | 1 | 将既有控制句柄从应用 facade 统一导出后重跑通过 |
| Phase 3 架构测试首次扫描范围过宽 | 1 | 将构造动作移入应用工厂，Engine 只保留应用运行期句柄 |
| 依赖拆分后四个集成测试仍使用旧平铺字段 | 1 | 按会话、配对、迁移三组更新测试装配，避免恢复平铺大包 |
| 成员端到端测试未启用 `dev-tools` 时匹配 0 个测试 | 1 | 确认测试文件受功能开关保护，启用 `dev-tools` 后重跑，不把 0 tests 计为通过 |
