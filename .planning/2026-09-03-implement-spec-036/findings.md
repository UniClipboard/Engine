# Findings & Decisions

## Requirements
- 实现 `docs/exec-plans/active/036-architecture-deepening-clean-cutovers.md` 的五个切片。
- 每个切片先通过已确认 seam 建立失败测试，再最小实现并运行定向检查。
- 项目文档/注释中文，标识符/提交英文；生产代码禁止 unwrap/expect/println/eprintln。
- 持久化默认密文、日志禁止身份/地址/内容/路径等敏感信息。
- Application 下层错误保留 source chain；Engine 只组装，Application 完整拥有业务步骤。
- 修改后同步 architecture bible，最终提交本任务但不提交用户既有 `uc-engine-interface.md` 修改。

## Research Findings
- 规格 036 将实现拆为 Clipboard、retired membership persistence、peer-address resolver、SessionSupervisor、Space security modes 五个 clean cutover。
- 用户工作区已有 `docs/design-docs/uc-engine-interface.md` 与 architecture bible 的接口校正；本任务必须保留且最终分离 staging。
- 规格文档、active index、architecture bible 的规格记录由上一任务创建，属于本次实施交付的一部分。
- ClipboardAssembly 已持有 capture use case、live index、active register dependency；ClipboardSession 在网络启动后持有 outbound/sync，因此完整处理器应由 session 构造并组合进程级与 session 级能力。
- `ApplicationRuntime` 的 owners 保存 ClipboardSession，当前通过 session 的 `capture/live_index/sync` getters 暴露步骤；新入口可直接委托 session owner。
- `AdvanceActiveClipboardPort` 已注入 ApplicationDeps，Engine 额外持有的 `LocalActiveRegisterAdvancer` 仅服务当前 host caller，Slice 1 后应评估删除该 Engine 投影。
- Clipboard explicit send 与 host observation 共用 capture/index/dispatch，但只有 host observation 推进 active register；该差异必须由 intent 映射而非布尔步骤参数表达。
- `LocalActiveRegisterAdvancer` 的写入本来就是 best-effort；完整 processor 不应把 register failure 增加为整体错误，但其现有日志含 snapshot hash/entry id，迁移时必须一并脱敏。
- `ClipboardSyncRuntime::dispatch_local_capture_to_targets` 已是完整 delivery lifecycle，可通过一个 Clipboard-private dispatch port 接入 processor，不需要改变 outbound protocol。
- HostEventBus 已经由 ClipboardAssembly 持有并用于其他 Application 流程；为保持“capture 成功后即通知，即使后续 dispatch 失败”的现有顺序，宿主新内容事件应由 processor 在 dispatch 前发出，而不是等待 Engine 收到成功 completion。
- `Background` dispatch 当前吞掉发送失败但保留 capture/active/event；processor 可仍返回 typed Dispatch error，由 Engine background caller 按既有 policy 吞掉，但事件必须已在 Application 内完成。
- `HostEventBus` 是 Application support module 的稳定 fan-out，ClipboardAssembly 已注入同一实例；Local processor 可直接 best-effort emit `NewContent`，无需新增上层 recorder port。
- HostEventBus 的 `emit_or_warn` 已定义统一 best-effort policy，因此 processor 只需在 `ObservedHostChange` capture 成功并推进 active 后调用一次。
- `DieselRelationshipStateResetAdapter::clear_all_relationships` 对当前 profile 的 `encrypted_relationship` 整表执行删除，因此无需保留旧 kind 解码器也能清理未知历史行。
- 已退役的候选、announcement、outbox、applied-security-update port 与错误类型只服务同名 Infra adapter/legacy store 分支；当前生产装配不再构造它们。
- Slice 2 的 RED 采用架构负向守卫：只要旧 adapter 文件、port 名或 legacy relationship kind 仍可达，仓库检查即失败；随后再以原始未知行测试证明 reset 能力不依赖旧 codec。
- Iroh 至少十个协议 adapter 曾各自读取并解码 `PeerAddressRecord.addr_blob`，其中 history/progress/recovery 路径会把 repository failure 或 codec corruption 合并为 missing/offline。
- `PresenceError::Internal(String)` 无法保留 resolver 的 source chain；改为带 boxed source 的稳定 Internal variant 后，Core 仍不依赖 Iroh/postcard，Infra 可保留具体根因。
- 各 Iroh adapter 保持既有外部结果映射：可降级路径继续映射 offline/unreachable/unknown，但只记录 `repository` / `invalid_encoding` 稳定分类；Presence 与 branch recovery 保留 typed source chain。
- SessionSupervisor 在切片前已拥有 gate、transition、reset、suspend/resume 与 install policy，但 `SessionFactory`、`ProductionSession`、实际 build/shutdown 仍定义在父 `runtime/mod.rs`，并通过 `ProductionRuntime::build_session` 反向调用。
- 把 session storage 也收归 supervisor 后，父 runtime 的投影必须经 supervisor 方法完成；host clipboard 需要一次锁内同时取得 facade/application，避免两次投影跨 session 切换。
- session shutdown 的 history/search 错误日志原先输出底层 Display；迁移时改为固定 `history` / `search` 分类，避免错误文本携带敏感信息。
- Space access 的真实生产构造点都能提供 revocation、legacy bootstrap 与 profile content vault；只有 config migration 的初始化路径真实只需要 key material、current profile 和 in-memory session。
- `RuntimeSpaceAccessAdapter` 的三项安全依赖改由无公开缺失构造的私有 required wrapper 保存；仅 `cfg(test)` seam 可表达历史 fixture 的缺失依赖。`MigrationSpaceAccessAdapter` 只实现初始化 port，不投影运行期安全能力。

## Technical Decisions
| Decision | Rationale |
|----------|-----------|
| TDD seam 以规格定义为用户已确认 | 用户明确要求实施该规格，规格已经列出稳定入口和测试策略 |

## Issues Encountered
| Issue | Resolution |
|-------|------------|
| 探索命令尝试读取不存在的 `crates/uc-engine/AGENTS.md` | 根 AGENTS 已覆盖 Engine，后续只读取实际存在的局部 AGENTS 路径 |
| 规划更新把 Test Results hunk 错放到 findings 文件而未应用 | 读取两个文件后分别按正确 section 更新 |
| 新 ApplicationRuntime 方法最初试图跨 Mutex guard 返回 ClipboardSession 引用 | 改为 clone processor Arc，保证 await 时不持有 owner 锁 |
| 错误路径假定 host event 位于单文件 `facade/host_event.rs` | 实际为目录模块并 re-export support bus；改从 `facade/host_event/mod.rs` 和 support implementation 读取 |
| reset 回归测试最初插入未知 kind，被现有 SQLite CHECK 约束拒绝 | 改为直接插入允许的退役 `candidate` kind 与不透明密文；仍不依赖旧 codec，并验证整表清理 |

## Resources
- `docs/exec-plans/active/036-architecture-deepening-clean-cutovers.md`
- `AGENTS.md` 与相关 crate 局部 `AGENTS.md`
- `docs/design-docs/decisions/018-domain-oriented-application-layout.md`
