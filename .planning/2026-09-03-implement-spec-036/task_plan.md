# Task Plan: Implement Spec 036

## Goal
按规格 036 完成五个 clean-cutover 切片、测试、文档收尾、代码审查和提交，同时保留用户既有工作区改动。

## Next Step
提交已完成实现并交付。

## Current Phase
Phase 8

## Phases

### Phase 1: Baseline & Seams
- [x] 确认规格 036 全部五个切片均在范围内
- [x] 确认 TDD seam 与仓库硬约束
- [x] 记录当前测试和工作区基线
- **Status:** complete

### Phase 2: Slice 1 - Clipboard complete intent
- [x] red：完整意图行为测试失败
- [x] green：Application 单入口与两个 Engine caller 迁移
- [x] 删除旧步骤入口并通过定向测试
- **Status:** complete

### Phase 3: Slice 2 - Retired membership persistence
- [x] red：opaque retired rows reset 与负向 reachability 测试
- [x] green：删除 ports/errors/adapters/store branches
- [x] 通过 Core/Infra 定向测试
- **Status:** complete

### Phase 4: Slice 3 - Peer address resolver
- [x] red：resolver 四类结果测试
- [x] green：迁移全部 Iroh address consumers
- [x] 通过 Infra contract tests
- **Status:** complete

### Phase 5: Slice 4 - Session lifecycle
- [x] red：父 runtime 生命周期实现负向门禁
- [x] green：生命周期 implementation 收入 supervisor module
- [x] 通过 Engine 定向测试
- **Status:** complete

### Phase 6: Slice 5 - Space security modes
- [x] red：非法组合负向架构 fixture
- [x] green：拆分 runtime/migration adapters 并迁移 callers
- [x] 通过 migration/security 定向测试
- **Status:** complete

### Phase 7: Architecture & Documentation
- [x] 增加五项负向架构门禁
- [x] 更新规格状态、稳定设计和架构圣经
- **Status:** complete

### Phase 8: Review, Full Verification & Commit
- [x] 审查完整 diff 与错误/隐私/所有权契约
- [x] 运行全量测试和仓库门禁
- [x] 仅提交本任务修改并交付
- **Status:** complete

## Decisions Made
| Decision | Rationale |
|----------|-----------|
| 按规格切片顺序做纵向 TDD | 每个行为 seam 可独立 red/green/验收，避免横向铺开半成品 |
| 不使用子代理 | 当前会话规则未授权多代理，实施 skill 也未要求代理协作 |
| 不调用缺失的 code-review skill | 当前 skills 列表没有该能力，最终用独立 diff 审查清单替代 |

## Errors Encountered
| Error | Resolution |
|-------|------------|
| `sed` 读取不存在的 `crates/uc-engine/AGENTS.md` | 确认该目录无局部 AGENTS，继续遵循根约束 |
| Slice 1 首次 green 编译发现测试 snapshot 缺少新增字段 | 改用项目提供的完整 snapshot 构造方式，不在测试复制结构字段 |
| 一次 planning 更新将 Test Results hunk 指向错误文件 | 重新读取 planning 文件并分别更新 findings/progress |
| ApplicationRuntime 首次委托时把 Mutex guard 内引用带出作用域 | 改为从 ClipboardSession clone `Arc<LocalClipboardProcessor>` 后再 await |
| 读取不存在的 `facade/host_event.rs` | 定位到 `facade/host_event/mod.rs` 与 `support/host_event_bus.rs` |
| 全量测试中的 dependency firewall 仍从 `runtime/mod.rs` 查找 ProductionSession | 更新测试从新 owner `session_supervisor.rs` 验证稳定 Application handles，定向与全量重跑通过 |
