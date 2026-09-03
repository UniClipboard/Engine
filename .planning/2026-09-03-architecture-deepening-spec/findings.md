# Findings & Decisions

## Requirements
- Spec 使用中文，仓库路径使用相对路径。
- 必须覆盖五项候选：Clipboard capture、退役 membership persistence、Iroh peer address resolution、SessionSupervisor、Space security mode。
- 必须包含 Goals、Non-Goals、当前架构、拟议设计、接口方向、实施步骤、边界情况、测试、验收、风险与开放问题。
- 不直接实现生产代码。
- 修改仓库内容后更新 `docs/architecture/architecture-bible.md` 的文档维护记录。

## Research Findings
- `docs/PLANS.md` 要求 active plan 写明状态、完整负责人、唯一调用、成功/失败结果、恢复责任和验收条件。
- 当前最大确定性问题是 Clipboard capture 的完整动作被两个 Engine caller 分别编排，行为已分叉。
- membership legacy repositories 需要以“先证明升级路径不读取、再删除 runtime surface”为门禁。
- peer-address resolution 应保持为 Infra 内部 seam，不能提升为 Application 总 provider。
- SessionSupervisor 与 Space security mode 风险较高，适合后置并独立验收。
- 文档系统确认本任务属于 Exec Plan：它回答“如何分步交付、现在到哪里”，完成后才把稳定结论回写长期设计。
- 下一可用编号为 036；仓库内没有现有 `036` 规格。
- 规格 031 已完成 Application 对象图与生命周期深化，但仍明确描述 Clipboard 完整意图应由领域模块拥有；本规格应作为后继收口，不重复 031 已完成工作。
- 规格 035 规定观测按真实装配 seam 推进，禁止为了一个宽泛 Clipboard bundle 改变两阶段对象图；本规格不得借 capture 收口改写 observability seam。
- 规格 034 尚待实施，可为 peer-address resolution 提供 Iroh provider contract 安全网，但 036 不应把自身完成绑定到整个 034 完成。
- Core beliefs 与 ADR-018 明确要求 Application 完整负责人隐藏步骤；Clipboard 切片不是新方向，而是修复 `ApplicationRuntime` 仍暴露步骤级方法的遗留偏差。
- `MembershipCandidateRepositoryPort`、`MembershipAnnouncementRepositoryPort`、`MembershipOutboxRepositoryPort`、`MembershipAppliedSecurityUpdateRepositoryPort` 的非定义引用只出现在 `relationship_store.rs` 测试区和各自 adapter；生产 wire 只构造 member/trusted-peer/peer-address store。
- `RelationshipKind` 与四个旧 payload codec 仍留在共享 encrypted relationship store，因此删除时必须保留通用 reset 对遗留 kind 的清除能力，且不可通过解密/明文迁移恢复。
- Iroh 地址读取至少存在 History、Group Update、Presence、Transfer Progress、Active Clipboard Pull/Dispatch 六类重复；部分把 repository/decode 一并降级为 offline，部分返回 Internal，且日志当前包含 target/device 字段，需按安全规则改为无身份的稳定分类。
- `SessionSupervisor` 保存 factory/session，却调用 `ProductionRuntime::build_session`；`ProductionSession::shutdown` 同样在 `runtime/mod.rs`，形成生命周期 owner 与 implementation 的双向依赖。
- `DefaultSpaceAccessAdapter` 以三个 optional capability 和三个构造器表达 runtime/migration 模式，方法内部大量重复 unavailable 检查；拆分必须保持既有密码、generation、AAD 和恢复语义。

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| 优先描述语义与职责，不冻结最终 Rust 名称 | Spec 应可实施，但命名应在实现时结合现有类型和测试确定，避免凭空增加不合适的稳定接口 |
| 五项分阶段、每阶段可独立停止 | 降低跨层大改的回归范围，并允许价值验证后再继续 |
| 退役 persistence 清理放在 Clipboard 后、peer resolver 前 | 它是删除型工作，先减少死 surface；但先做 Clipboard 能最早消除已出现的行为分叉 |
| Space security mode 最后实施 | 安全与迁移状态复杂，必须在前四个切片稳定后单独验证 |
| Clipboard 输入使用意图枚举而非步骤布尔值 | 防止调用者以新形式继续编排内部步骤 |
| peer resolver 只统一地址读取事实，不统一协议结果 | offline、unreachable、internal 属于各 adapter 已有合同，集中它们会让 Infra helper 获得业务语义 |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| 一次规划补丁因 `Current Phase` 上下文顺序不匹配而失败 | 重新读取当前规划文件，拆分为精确上下文补丁 |
| 组合补丁后续 hunk 失败但前一文件已应用 | 读取实际落盘状态后补齐缺失部分，不重复已完成修改 |

## Resources
- `AGENTS.md`
- `docs/PLANS.md`
- `docs/exec-plans/completed/031-application-dependency-surface-deepening.md`
- `docs/exec-plans/completed/035-space-domain-observability-assembly.md`
