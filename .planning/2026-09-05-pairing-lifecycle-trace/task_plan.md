# Task Plan: Pairing Lifecycle Trace

## Goal

让一次未中断的双设备配对在可视化平台中表现为一个完整 trace；重试或重启产生新 trace，但沿用同一匿名 flow，并保持 Engine 不感知内部步骤。

## Next Step

无。实现、真实页面验收、性能对比、文档和全量门禁均已完成；等待用户决定是否提交。

## Current Phase

Completed（含可读动作名称与准确时间条修复）

## Phases

### Phase 0: Ownership and lifecycle baseline

- [x] 确认完整 owner和调用边界
- [x] 记录当前一轮配对的 trace/span 真实结构
- [x] 冻结不落盘、不泄露步骤的生命周期合同
- **Status:** complete

### Phase 1: Failing end-to-end contract

- [x] 增加正常配对只有一个 TraceId 的红测
- [x] 增加重启后新 TraceId、同 flow 的红测
- [x] 确认测试非零执行且因现有拆分而失败
- **Status:** complete

### Phase 2: Opaque lifecycle root

- [x] 在观测合同中实现不可读取的进程内生命周期句柄
- [x] Application 完整 owner 跨 Session 重建复用句柄
- [x] Infra/Engine 现有完整能力自动成为子节点
- **Status:** complete

### Phase 3: Outcome and recovery boundaries

- [x] 成功、取消、拒绝、升级阻塞和延期结果准确结束
- [x] 进程关闭不持久化 TraceId，重启建立新 trace
- [x] 旧成员更新保持原业务行为
- **Status:** complete

### Phase 4: Visual and regression verification

- [x] 真实双设备配对写入本地 Collector
- [x] Jaeger 实际展开一个完整 trace
- [x] 全量回归、性能、隐私和架构门禁通过
- [x] 更新稳定文档与架构圣经
- **Status:** complete

## Decisions Made

| Decision | Rationale |
| --- | --- |
| 正常连续配对一个 trace | 满足一次点击查看完整配对的排障体验 |
| 重试/重启新 trace、同 flow | 不持久化或伪造跨进程 TraceId |
| 根归 Application 的完整 Space Admission owner | Joiner/Sponsor 都只是挂载者，不向 Engine 泄露步骤、状态或业务标识 |
| 不为观测改变配对协议 | 将可视化改造与 038 性能改造分开 |

## Dirty Worktree Boundary

- `AGENTS.md`、profile storage upgrade、Cargo 构建目录检查及两套对应 planning 属于既有其他工作，禁止回退或纳入本次修改。
- `docs/architecture/architecture-bible.md` 与 `scripts/architecture/check-engine-repository.mjs` 含既有混合修改，若本次更新必须逐 hunk 保持边界。

## Errors Encountered

| Error | Attempt | Resolution |
| --- | --- | --- |
| 正常配对 trace 红测得到 4 个 TraceId | 1 | 预期红测，证明现有每轮独立 root；进入生命周期句柄实现 |
| Recovery 无法调用 Joiner 私有 observation 方法 | 1 | 可见范围收窄为 protocol 内部，不导出到 Application 外层 |
| observation 类型本身仍窄于字段可见范围 | 2 | 类型与方法统一限定到 protocol 内部 |
| 首版生命周期句柄仍得到 2 个 TraceId | 1 | 先扩充红测证据，定位提前结束或重建边界 |
| 稳定 registry 接线的模块导出与测试构造参数错误 | 1 | 从 Space 根只向本 crate 重导出；按 Joiner 构造签名修正两个测试实例 |
| 嵌套私有模块无法干净地逐层扩大重导出 | 3 | 停止放宽 protocol 可见性；把跨 Session 上下文移到 Space 应用根目录 |
| 归属移动后两处仍引用旧 admission 路径 | 1 | 统一改为 Space 根的 crate 内部重导出 |
| 重启 E2E 按旧 client-root 结构找不到 trace | 1 | 预期红测；更新证据模型识别生命周期 parent |
| 生命周期 root 被现有 flow scope 隐私规则拒绝 | 1 | 为唯一 Joiner Internal root 增加设备与 Collector 精确允许组合 |
| Collector 自测仍断言旧 Joiner span kind 组合 | 1 | 同步 consistency 规则，继续要求两份配置一致 |
| managed sandbox 拒绝共享 sccache 启动 | 1 | 不清空 RUSTC_WRAPPER；检查缓存工具可写运行方式后再继续 Cargo |
| 独立端口的任务 sccache server 仍被沙箱拒绝 | 2 | 不重复启动；尝试工具支持的无后台模式 |
| sccache 无后台模式同样 Operation not permitted | 3 | 停止重复 Cargo；继续静态/Collector 工作，最终编译等待环境恢复 |
| fmt check 指出本次导入与换行格式 | 1 | 只手动调整本次文件，不批量格式化其他 owner 修改 |
| sccache 官方 server-I/O 降级开关仍被拒绝 | 4 | 确认 managed sandbox 外部阻塞；不绕过缓存，不继续未验证生产修改 |
| 新一轮默认 Cargo 仍被 sccache 权限拒绝 | 5 | 不重复默认路径；尝试从可执行临时目录运行同一缓存工具 |
| unfinished lifecycle 红测辅助代码读取 AnyValue 失败 | 1 | 按日志真实枚举提取 String，再验证行为红测 |
| supersede 红测立即通过 | 1 | 测试新旧 admission id 相同；改成两个不同固定编号后重跑 |
| 截图临时目录直接删除被安全策略拒绝 | 1 | 改为移动到废纸篓并验证原路径消失 |
