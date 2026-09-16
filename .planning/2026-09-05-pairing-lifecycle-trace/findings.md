# Findings: Pairing Lifecycle Trace

## Requirements

- 一次未中断配对在 Jaeger/PostHog 中可作为一个完整 trace 查看。
- 重试或重启不得复用持久化 TraceId，必须用同一匿名 flow 聚合。
- 不向 Engine 暴露 Application/Core 内部步骤、状态对象或业务标识。
- 不改变配对协议、持久安全检查点、新旧版本提示和旧成员更新。
- 根节点语义归完整 Space Admission 生命周期，不归 Joiner 或 Sponsor 任一角色；两端只挂载现有完整能力。

## Baseline

- 当前真实数据共 17 条 trace、27 个 span。
- 同一配对 flow 下有 8 条 trace：4 条单 span 建链记录和 4 条三 span 消息交换树。
- 每棵三层树为 Joiner transport -> Sponsor transport -> Sponsor 完整 endpoint。
- 改造前的 `scope_space_admission_observation` 只设置 task-local flow，不创建或复用生命周期 root；当前已由中性生命周期句柄取代。
- Application 恢复 owner 分别包裹建链与消息交换，因此没有共同父节点。

## Open Questions

- 生命周期 root 如何跨多次 maintenance invocation 复用并在终态精确结束。
- 取消、明确升级阻塞和暂时延期分别应关闭还是保留当前连续 trace。
- 进程关闭时如何结束仍打开的 root，同时保证重启后只复用 flow。

## Ownership Findings

- `recover_pending` 是 Joiner 待准入恢复的完整 Application owner；它加载持久状态、建立认证通道、交换一条消息并提交回复。
- 当前同一轮分别在建链和交换处调用不透明作用域，所以两个 client span 都成为独立 root；后续 maintenance invocation 也没有可复用父节点。
- Sponsor 的 server transport 和完整 endpoint 已正确继承 client context，三层关系无需重写。
- 现有端到端辅助函数明确要求 client span 没有 parent，并收集至少四个 TraceId；新红测应反转这一合同，而不是只检查 span 数量。
- 观测合同已有固定 operation/role/kind 和 completion 记录；生命周期 root 必须沿用该合同，不引入步骤名或业务字段。
- `SpaceAdmissionProtocol` 已通过 profile 级锁串行所有完整动作，是跨 recovery invocation 保持同一进程期句柄的现有生命周期边界。
- 局部规则禁止把 Recovery/Joiner/Sponsor 作为步骤暴露给调用方；新增能力必须保持在私有完整 owner 内。
- 当前 E2E 性能辅助函数把 client span 无 parent 和至少四个 TraceId 当作合法条件；它必须由红测改成 client 都指向同一生命周期 root 且 TraceId 唯一。
- 正常流程无需修改 Sponsor server -> endpoint 关系；只要 client span 继承同一个 root，跨设备三层会自动进入同一 trace。
- `JoinerAdmissionService` 同时拥有开始、取消、Candidate/Commit/Complete/Settled 和本机激活，适合持有私有生命周期句柄；Recovery 已接收该 owner 引用。
- `SpaceAdmissionProtocol` 根结构按局部规则只保留三个角色 owner 和串行锁，不应新增观测字段。
- `start_join` 在持久提交成功后已有 admission id，可在唤醒 maintenance 前创建句柄；失败提交不得留下句柄。
- `cancel_join` 只保存取消请求并唤醒恢复，不是终态，不能在这里结束生命周期。
- Joiner 本机激活后仍处于 `ActivePendingSettlement`，最后收到并保存 `Settled` 后才是正常连续配对的成功终点。
- 可重试的网络/状态/准备失败应结束当前 trace 为 deferred；下次 maintenance 新建 trace、复用同一 flow，避免把离线等待伪装成一个长 span。
- 首轮明确拒绝、版本升级阻塞和 recovery-required 都应结束当前 trace；不会跨天保留打开的 root。
- 需要保证一次正常流程的多次 maintenance invocation 复用同一 root；成功推进只保留句柄，直到 Settled。
- 首版把句柄放在 `JoinerAdmissionService` 后，真实 E2E 从 4 条收敛到 2 条，分布精确为 3+1。
- 3+1 分界证明本机激活时 Space 运行对象重建，Joiner 私有 map 随旧对象释放，最后 Settled 在新对象中懒建 root。
- 进程全局 registry 不是正确修复：测试中的 Engine 重启仍在同一进程，会错误复用旧 trace；owner 必须跨 Space session 重建但随单个 Engine 实例关闭。
- `SessionSupervisor` 在本机激活时替换 `ProductionSession`，但稳定持有同一个 Application `ApplicationAssembly`；它正好跨 session rebuild、随 Engine 实例结束。
- Engine 不应读取或操作 admission key/outcome；稳定对象必须由 Application 定义为不透明上下文，Engine 只在既有组装中保存并透传。
- Joiner 服务中的本地 map 可以保留为角色内入口，但底层句柄 registry 必须来自稳定 Application assembly，而不是每次 `SpaceApplication::new` 新建。
- `ApplicationAssembly::assemble_network` 是每次新 Session 构造 `SpaceFacade` 的唯一 Application 入口；从自身私有字段把同一个中性 admission registry 传入。
- 这样 Engine 的 `SessionSupervisor` 仍只保存既有 `ApplicationAssembly`，不新增 admission 方法、标识或完成调用，符合“只组装不编排”。
- 正常结构测试在加入 root 完整性断言后等待超时；设备侧 `span_rejection_reason` 明确只允许 flow 出现在 Joiner Client，新的 Joiner Internal root 被拒绝发送。
- Collector 两份配置也有同一 flow scope 限制；必须同时修改两道防线，且只允许 `space_admission/joiner/internal` 的无 parent root。
- 日志仍不携带 `uc.flow.id`；root 完成日志只通过 TraceId/SpanId 关联，不扩大日志字段。

## Corrected Ownership

- 用户指出“产生位置”不等于“语义所有权”：根在发起设备创建，但代表完整 Space Admission，不应标成 Joiner root。
- 稳定 registry 归 Application 的 admission 领域，类型改为中性的 `SpaceAdmissionObservationRegistry`，文件归 `space/admission/observation.rs`。
- 根节点使用 `role=local`、`kind=internal`；Joiner client 与 Sponsor server/endpoint 都是挂载者。
- Sponsor 不共享内存句柄；认证后通过远端父关系挂入同一 TraceId，这是分布式两端唯一可行的统一挂载方式。
- Engine 仍只保存既有 Application assembly，不读取 registry、flow、业务标识或步骤。
- 中性归属已落到工作树：`space/admission/observation.rs`、`SpaceAdmissionObservationRegistry`、root `role=local`；Joiner/Sponsor 现有角色不变。
- Collector 两份配置已同步允许唯一 local/internal root，Node privacy 5/5；日志仍禁止 flow。
- 当前 managed sandbox 禁止 sccache 编译入口；共享、独立 server、无后台和 server-I/O 降级四种方式均返回 Operation not permitted。按仓库规则不能清空 `RUSTC_WRAPPER` 绕过。

## Final Local Evidence

- 权限恢复后 Application 全目标检查通过；未中断与重启两条真实 E2E 均通过。
- 清空 Jaeger 旧数据并重载 Collector 后，公开双设备配对产生 10 条 trace、28 个 span；其中唯一 admission lifecycle trace 含 17 个 span。
- lifecycle trace `f493720c18cafb4186fa4dbb7ce199e7`：1 个 local root、8 个 Joiner 节点、8 个 Sponsor 节点，operation 为 9 个 `space_admission` 与 8 个 `network_transport`。
- Collector 解码到该 trace 的 17 条完成日志，日志 flow 违规为 0。
- Jaeger 实际页面显示 Total Spans 17、Depth 4、Duration 5.3s，并能在一棵树下看到四次建链和四棵三层消息子树。
- 三轮交错性能样本：远程关闭 `5.594943/5.520490/5.775659s`，健康接收端 `5.411986/4.841041/4.838525s`；中位数约 5.59s 与 4.84s，只证明新增 root 没有造成可见回退。

## Resources

- `crates/uc-application/src/space/admission/protocol/recovery/recover_pending/execute.rs`
- `crates/uc-observability-contract/src/diagnostics/mod.rs`
- `crates/uc-infra/src/network/iroh/space_admission.rs`
- `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`
