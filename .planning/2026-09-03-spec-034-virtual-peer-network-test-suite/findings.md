# Findings & Decisions

## Requirements
- 编写 034 implementation-ready spec，主题为 virtual peer network 测试套件。
- 解决 F1～F7 全走真实 Iroh 导致耗时长、失败定位困难的问题。
- 保留真实 Iroh provider 边界与必要 E2E 证据，不把虚拟网络当作 Iroh 实现证明。
- 修改仓库文档时同步更新 `docs/architecture/architecture-bible.md` 的文档维护记录。

## Research Findings
- `docs/exec-plans/active/` 当前没有 034；最新编号 033 已完成，因此新规格应创建为 active 034，并更新 active index。
- Exec Plan 必须明确状态、完整负责人、调用方唯一动作、成功/失败结果、恢复责任与验收条件。
- 文档系统要求 Active plan 完成后才回写长期事实并移动至 `completed/`；本轮不能把待实现方案写成当前架构。
- 固定依赖方向为 `uc-engine → uc-infra → uc-application → uc-core`；port 应由消费能力的业务层拥有，不能泄露 Iroh 细节。
- 虚拟网络测试轨迹与错误输出不得包含设备名、地址、邀请、凭据或业务负载；只能使用不敏感的测试节点标签、协议分类、序号和计数。
- 跨层计时和结果分类属于 Engine decorator；测试套件本身可以生成确定性 trace，但不能成为生产观测入口。
- Spec 030 已完成且明确记录 F0-F7 的真实 Engine/Iroh 验收，包括 F7 430.46 秒；034 必须保留这份历史证据，不能回改为虚拟测试通过。
- Spec 030 已承认大矩阵不适合每次快速测试，并把耗时 Engine/Iroh 场景放到串行验收阶段；034 是验证分层和后续回归策略的改进，而不是否定 030。
- F8-F13 已按责任落在 Core/Application/真实 Infra；虚拟套件的主要迁移对象是 F0-F7 中属于协议拓扑和调度的断言。
- F0-F7 当前断言包含 branch/head 等价类、通信矩阵、group epoch、pending conflict/effect、revision 单调和 exact text；虚拟层不能声称覆盖真实 encrypted content、Iroh endpoint 或 SQLite/control-generation。
- Spec 029 已分别要求确定性多节点模型与真实 Iroh/SQLite/Desktop 证据，支持“快速确定性协议矩阵 + 小型真实 provider 验证”的分层方向。
- `uc-application` 明确禁止依赖 Infra，且 use-case 专属 port 归 Application 业务模块所有；因此不能把 Iroh 抽象类型或虚拟网络实现塞进 Application 生产依赖。
- `MembershipBranchRecoveryChannelPort` 已是窄的认证 peer 两阶段恢复信道，并提供带 source 的 `Unavailable/Rejected/Invalid` 分类；虚拟 adapter 应实现它，不新增全能 transport trait。
- Engine 已有 `DevOperation::SetNetworkPartition`，其语义绑定认证 Iroh endpoint id；虚拟测试不应复用或泛化这个公开 dev operation。
- 当前 Application 测试使用 `PassivePorts`、`RecordingTransport`、`SwitchableTransport` 等各自 fake，缺少可复用的多节点网络调度器。
- `uc-application` 已有 `test-support` feature，但用途和公开面需要继续核对；直接把整个 testkit 放入该 feature 可能扩大生产 crate API。
- 实际 port 所有权并不完全一致：history/group-update/reachability 目前定义在 `uc-core`，branch recovery channel 定义在 `uc-application`。034 应复用现状并明确不在本规格迁移 port 所有权。
- Iroh history adapter 负责地址解析、ALPN、双向流、10 秒超时、frame/version codec、认证 remote id 到 `DeviceId` 的绑定和 endpoint dispatch；这些是 provider contract/真实 Iroh 测试责任，不属于虚拟拓扑语义。
- Iroh branch-recovery adapter 负责两阶段 wire、frame bounds、超时、认证 source 和稳定错误映射；虚拟 adapter 只需实现领域信道语义，Iroh wire 仍由真实 adapter 测试证明。
- Iroh group-update adapter 负责大小上限、ALPN、ACK 与超时；虚拟网络需能投递真实 `PendingGroupUpdate` 语义并返回 Accepted/Rejected/Offline，但不假装验证 QUIC framing。
- 当前 `MembershipTopology` 与 `TopologyAction` 位于 2441 行的 Engine E2E 文件，混合真实 Engine 生命周期、rendezvous、邀请、持久目录、Iroh endpoint id、分区控制、业务动作和最终断言；034 应拆出领域拓扑测试，而不是直接把该 harness 改成 transport switch。
- `IrohNetworkPartitionGate` 基于 `EndpointHooks`：连接前拒绝、握手后拒绝，并主动关闭已建立连接；虚拟 `partition` 不能替代这一真实行为证明。
- Workspace 当前没有共享 testkit crate。独立未发布 testkit 可以依赖 `uc-core` 与 `uc-application`，但若依赖 Application 私有装配会遇到模块可见性；规格必须固定可复用调度器与节点 fixture 的所有权边界。
- Spec 031 的方向是领域 assembly 使用 in-memory/test adapters 做 interface 测试，同时在关键行为追加真实 SQLite、Iroh loopback 和 Engine 双实例证据；与 034 的分层方案一致。
- `SpaceApplication` 与 `build_for_test` 都是 crate-private/`cfg(test)`；外部 workspace testkit 无法直接组装真实 Space Application 节点，除非扩大 Application 的 test-support 公共面。
- `SpaceRuntimeAdapters` 是公开 assembly 输入，但完整节点仍需要大量非网络 adapter；最小改动是在 `uc-application` 内部测试模块建立 virtual topology fixture，而不是先创建跨 crate 公共测试框架。
- 当前 `test-support` feature 只公开 clipboard 的 `ApplyInboundClipboardUseCase` 给 LAN compatibility，并不适合作为 Space 私有 assembly 的默认出口。
- Spec 031 明确要求 Engine 继续拥有 Iroh/Infra adapter 选择，且 Application 白名单不包含内部 use case/session；034 不应为了共享测试工具破坏该已完成契约。
- `SpaceApplication` 已提供 crate-private test builder 和 membership history、branch recovery endpoint；将 topology fixture 置于 `space` 的 `cfg(test)` 子模块即可连接真实 Application endpoint，无需扩大公共 API。
- Membership maintenance runtime 自带 Tokio interval、spawn 和后台 trigger。快速确定性套件若直接启动完整 runtime，仍会受到调度时序影响；应由测试驱动显式执行/唤醒并用 `settle(max_steps)` 收敛，或使用 paused time 控制定时器。
- Virtual topology 的“restart”应销毁并重建 `SpaceApplication`，复用节点的 durable test repositories；它只证明 Application 恢复语义，SQLite 原子性继续由 Infra 测试承担。
- Typed port 已经是 codec 之上的领域边界。因此快速 virtual suite 应传递 typed message；生产 codec、frame size、ALPN 与 QUIC timeout 留在 Iroh provider contract，避免复制 wire format。
- Git 交付审计：当前分支为 `feat/033`，HEAD `9192e74a`；本地 `main` 为 `d8a6c196` 并跟踪 `origin/main`。
- 当前工作树除 Spec 034、active index、架构维护记录和本任务 `.planning` 记录外没有其他未提交变更。
- 只有当前 worktree 使用 `feat/033`；本地 `main` 没有被其他 worktree checkout。
- Fetch 后 `origin/main` 仍为 `d8a6c196`；远端没有 `feat/033` branch。
- `main...feat/033` 分叉计数为 main-only 12、feature-only 36，共同祖先 `df10f98f`；必须创建 merge commit，不能 fast-forward。
- main-only 12 个提交均为 maintenance audit CI 调整；将在 feature 分支先合并 `origin/main`、解决冲突并验证，再让本地 `main` fast-forward 到已验证 merge。

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| 优先复用现有领域 port | 避免为测试引入覆盖全部 Iroh 能力的浅层生产接口。 |
| 不新增 workspace testkit crate | Application 的真实 Space assembly 是私有 seam；独立 crate 要么无法组装，要么迫使公开内部接口。 |
| 虚拟网络使用 typed message | Port 已位于 wire codec 之上，Iroh codec 由 Infra contract 测试负责。 |
| 由 topology driver 显式执行 maintenance round | 避免真实 interval、后台 spawn 和 wall-clock poll 重新引入非确定性。 |
| link mutation 只发生在 action 边界 | V1 不模拟正在传输的 QUIC connection 被关闭；该行为必须由真实 Iroh partition contract 验证。 |
| 虚拟内容断言使用授权矩阵 | exact text、密文和 blob transport 留给真实 Engine/Iroh smoke。 |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| 架构脚本在沙箱内无法由 Node `spawnSync cargo` | 获批后在沙箱外运行相同只读命令，完整 preflight 通过。 |

## Resources
- `docs/design-docs/documentation-system.md`
- `docs/design-docs/engineering-principles.md`
- `docs/design-docs/error-handling.md`
- `docs/design-docs/observability.md`
- `docs/SECURITY.md`
