# Task Plan: Observability Assembly Pattern Promotion Spec

## Goal
按规格 035 实现 Space admission/membership 观测装配收敛：Application 拥有领域 adapter bundle，Engine 对每个真实 Space seam 只执行一次同型 bundle 装饰，保持业务结果与 source chain，并完成事件合同、架构门禁、文档收口和提交。

## Next Step
已完成；等待后继规格选择下一个真实观测 seam。

## Current Phase
Complete

## Phases

### Phase 1: Requirements & Discovery
- [x] 确认现有 observability 模块的 interface、implementation、seam 与责任边界
- [x] 对比其他 assembly 模块，识别重复装配和适用/不适用范围
- [x] 记录仓库约束与发现
- **Status:** complete

### Phase 2: Planning & Structure
- [x] 确定推广目标、非目标与模块设计
- [x] 确定 Spec 文件编号和文档关系
- **Status:** complete

### Phase 3: Implementation
- [x] 按 `to-spec` 结构编写实现 Spec
- [x] 同步 `architecture-bible.md` 文档维护记录
- **Status:** complete

### Phase 4: Testing & Verification
- [x] 校验路径、设计与验收项可执行
- [x] 运行非行为改动交付检查
- **Status:** complete

### Phase 5: Delivery
- [x] 复核 diff 不覆盖用户已有工作
- [x] 交付结论、文件路径与验证结果
- **Status:** complete

### Phase 6: Implementation Baseline & Contract Tests
- [x] 核对所有 SpaceRuntimeAdapters 字段的真实消费者与 bundle 归属
- [x] 固定 admission 现有行为、membership schema/policy 和 source 透明 contract
- **Status:** complete

### Phase 7: Application-owned Space Bundles
- [x] 新增 SpaceAdmissionAdapters 与 SpaceMembershipAdapters
- [x] 将 SpaceRuntimeAdapters 和 SpaceApplication wiring 一次性切换到分组结构
- **Status:** complete

### Phase 8: Engine Observability Cutover
- [x] admission 改为同型 bundle 单入口并删除镜像类型/独立 transition 入口
- [x] 新增 membership 私有 decorator 与单入口
- [x] sync_engine 只提交已装饰 bundle
- **Status:** complete

### Phase 9: Tests & Architecture Gates
- [x] 完成事件 schema、policy、透明性与敏感哨兵测试
- [x] 增加架构负向 fixture 与 production shape 检查
- **Status:** complete

### Phase 10: Documentation & Full Verification
- [x] 更新稳定观测设计、架构圣经与规格验收证据并移动 completed
- [x] 运行规格全部测试和仓库交付检查
- **Status:** complete

### Phase 11: Review & Commit
- [x] 对完整 diff 做代码审查并修正发现
- [x] 创建单一原子提交
- **Status:** complete

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| 先分析推广语义，再确定目标模块 | 避免只复制目录结构，确保新模块在 interface 深度和 locality 上真正降低调用者负担。 |
| 规格编号使用 035 | active 目录已有 034，035 是下一连续编号。 |
| V1 只在 Space 内从 admission 推广到 membership | Clipboard adapter 分属进程 assembly 与 network binding 两个时点；先证明修正后的单入口模式，不为跨时点依赖制造假 bundle。 |
| 不为 Application 内部纯计算阶段建立 observation port | 这类 port 只有单一日志实现，会形成浅 module 并泄漏内部对象图。 |
| 仓库推广按真实装配 seam，而非宽泛业务领域强制单入口 | Clipboard/Blob 跨进程期与网络期装配；035 只实现同一 seam 完整的 Space admission/membership。 |

## Errors Encountered
| Error | Resolution |
|-------|------------|
| 首次从错误的用户级路径读取 `to-spec` | 使用仓库本地 `.agents/skills/to-spec/SKILL.md`。 |
| 首次 Application bundle 编译发现两个 sibling 模块仍穿透 `application::SpaceRuntimeAdapters`，且移动 struct 后遗留 imports | 改为从 `space`/`adapters` owner 导入，并收紧 `application.rs` imports；再运行 formatter。 |
| Admission 耗时转换的首个大 patch 因 formatter 已改变函数上下文而未应用 | 拆成 import、机械替换、helper 三个精确 patch；未产生部分修改。 |
| Rust 不允许把 `pub(super)` 函数扩大重导出为 `pub(crate)` | 函数标记为 `pub(crate)`，其所在领域 module 保持私有，只通过 `observability` 精确重导出，实际 interface 仍只有两个入口。 |
| 规划状态 patch 的上下文顺序与文件不一致 | 读取当前计划后按实际段落顺序重新应用；失败调用未修改文件。 |
| Membership source 透明测试误用不存在的 `DeviceId: From<&str>` | 改用仓库稳定 constructor `DeviceId::new`。 |
| 首次架构门禁运行拒绝新 `space/adapters.rs` | 将该 Application-owned interface 文件加入 Space 根级批准清单；保持 child module 私有、只精确导出类型。 |
| “第二 admission 入口”负向 fixture 未命中只检查精确目标行的门禁 | 同时统计该领域全部 crate-visible re-export，要求恰好一个且必须是批准入口。 |
| Admission exchange contract 首次用 `assert_eq!` 比较包含无 `PartialEq/Debug` 成功值的 `Result` | 改用 `matches!` 只验证稳定错误 variant。 |
