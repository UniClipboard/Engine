# Findings: 全仓关键日志缺口扫描

- 日期：2026-09-29；基线：c7a821b4 + 当前分支未提交改动（模块日志通道任务）。
- 方法：12 个模块切片并行扫描，每条候选由反驳型 agent 核验；只读，未改任何源码。
- 数量：155 候选 → 57 确认 → 去重后 55 条（P0 15、P1 30、P2 10，编号 1-55）。
- 前置事实（已核对）：`DiagnosticTaskKind` 与 `record_task_join_failure` 在
  `crates/uc-observability-contract/src/diagnostics/mod.rs`；`log_safe_errors!` 在
  `crates/uc-application/src/log_safe_errors.rs`；字段白名单在
  `crates/uc-observability-runtime/src/module_log_fields.rs`。这些都属于进行中的
  `2026-09-29-module-log-channel` 任务范围，本任务必须建立在其之上。

# UniClipboardEngine 全仓关键日志缺口报告

## 概览

- 输入 57 条,全部通过核验(`real=true`),被否决 0 条。
- 去重后 55 条:
  - access.rs:1169 与 access.rs:1546 同一模式,合并。
  - wire/mod.rs:499 与 encryption_passphrase_change.rs:74 同一恢复流程,合并,日志放 Infra `recover_locked`。
- 多数结论是"成立但需收窄"。下文各条已按核验意见收窄,被删减的子建议单列在"已收窄或放弃的子建议"。
- 各条优先级沿用原判。核验多次指出严重度偏"低到中",落地顺序建议以 P0 中的"整体失效且无痕"类为先。

## 跨层共性主题

1. **任务退出缺 `record_task_join_failure`**:观测文档要求"等待方"记录任务异常退出,但下列位置未做。修复方式是在 `uc-observability-contract` 的 `DiagnosticTaskKind` 中新增固定变体,共用一次契约改动。需同步 `as_str`、schema 快照与隐私测试。
   - 成员维护循环
   - 活跃剪贴板 worker 监督器
   - 出站进度翻译器
   - Engine 启动任务
   - 移动端 WorkerJoin
2. **吞错降级点未做本地分类**:多处 `Err(_)`、`let _ =`、`.is_err()` 直接丢弃错误。统一做法是在丢弃点写固定字面量消息,加 `error_kind` 和 `io_error_kind(..)`。字段须在 `ALLOWED_TEXT_FIELDS` 内,不得使用 `%e` 或 `?e`。
3. **`log_safe_errors!` 登记**:多处建议带 `error = &e as &dyn Error`,但相关错误类型未必已登记,未登记会渲染为 `<opaque>`。相关类型:
   - `SpaceActivityError`
   - `PeerConnectionError`
   - `KeyEpochError`
   - `MlsGroupError`
   - `ConfigMigrationError`
   - `SearchShutdownError`
   - `LifecycleError`

   实施前先登记,或只用固定 `error_kind`。
4. **错误转换先于日志**:`Err(_)` 丢弃来源可归为 `map_err` slop。优先改成保留 source(`unavailable_from`,或带固定分类的变体),再由负责人记录,不要到处补 warn。
5. **debug 在 Standard 模式被丢弃**:凡是"终止整个流程"的静默丢弃,不能只写 debug。同时依赖模块日志的按点限速。对端可控触发的失败保持 debug 或限速 warn。
6. **字段白名单**:`skip_reason`、`from_phase`、`to_phase` 不在白名单。应改用 `reason`、`previous_phase`、`next_phase`,或先登记新字段。`fingerprint` 在白名单里,但文档与实现不一致,仍不建议记录。
7. **状态迁移只在迁移点记一次**:多处涉及"进入 RecoveryRequired、Deferred、StableFailure"。这类应在提交迁移的完整负责人处记一次,固定 `reason` 枚举。返回给上层的错误不要重复记。

## 需要流程负责人提供不透明观测上下文的位置

以下按现有设计,日志都能在负责人内部以固定字段完成,不需要新增 Engine 暴露。真正可能需要不透明上下文的只有下面几处。

- **Engine 边界日志需要与 Application 负责人的结果关联**
  - member.rs:610(移除成员)、cancel_join_space.rs:21(取消加入)、config_migration/adapter.rs:601。
  - 这些位置由 Engine 或 Infra 边界记录,失败原因来自 Application 负责人。若要与"独立业务记录"(触发、动作、结果)关联,必须由 Application 流程负责人经不透明观测上下文提供。Engine 不得自行拼装阶段。
  - 移除成员目前没有业务完成记录。边界日志只是权宜之计,该业务动作的完整记录应由流程负责人补。
- **恢复类的跨步骤关联**
  - reconciliation.rs:97 与 :176 的恢复结果需要关联到触发它的会话激活。
  - recovery.rs:113 的后台激活循环需要关联到原始 `request_activation`。
  - 这些应由 `SpaceSessionRecovery` 或 `ReceiveReadinessCoordinator` 提供不透明上下文,Engine 只做装饰,不得据此编排。
- **远端父级关联**
  - membership_branch_recovery_adapter.rs:216 可复用 `group_update_adapter.rs` 的"设置远端父级并完成操作"模式。仅在与现有远端父级流程一致时采用。
- 其余条目均为负责人内部的运行诊断,不需要外部上下文。

## P0

### Application:space

1. **peer_connections/runtime.rs:209**
   - 问题:周期性 `let _ = self.reconcile().await;` 丢弃结果。`reconcile` 在 scope 读取失败或超时时已 `clear()` 掉全部 peer。
   - 建议日志:调用点写固定字面量 warn,`error_kind = scope_unavailable | scope_timeout`,由 `PeerConnectionError` 变体映射。失败开始时记一次,恢复时可选记一次。不要每 60 秒重复,也不要记设备或地址。
   - 注意:不要沿用第 194 行的 `error.type` 字段名。`local_member_active == false` 属正常,不记。
2. **session/recovery.rs:113**
   - 问题:后台激活循环的失败丢弃 `SpaceActivityError`,重试成功后也无痕。
   - 建议日志:失败 warn 带固定 `error_kind`、`attempt`、`delay_ms`。`attempt > 0` 时成功记 info(`attempts`)。
   - 放弃:取消 info。`JoinError` 如需记录,用 `record_task_join_failure`,不另写 warn。
3. **membership/maintenance/runtime.rs:249**(同类:268、357)
   - 问题:某轮 JoinHandle 返回 Err 后,只 `failure.set` 并 break,循环退出无日志。此后收敛、副作用、群更新投递全部停止,与空闲无法区分。
   - 建议日志:新增 `DiagnosticTaskKind::MembershipMaintenanceRound`,在各 `failure.set` 处调用 `record_task_join_failure`。第 357 行是真正静默的点。
   - 放弃:pause/resume 发送失败(87、99、101)已返回给调用方。取消退出的 info 可选。
4. **membership/handle_history_message/use_case.rs:286**
   - 问题:不可信后缀的校验失败被 `Some(Err(_)) | None => Invalid` 折叠,`MembershipHistoryV2Error` 被丢弃。
   - 建议日志:封闭枚举 `reason`(signature、invalid_operation、credential、other)加 `page_count`。debug 优先,避免对端触发刷屏。
   - 注意:`unbound_sender` 已由 `sender_is_bound` 覆盖。日志要在 `.commit(...)` 之后发,不要放进可能重试的闭包。
5. **membership/handle_history_message/use_case.rs:379**(同文件 345、356、406、441、456)
   - 问题:六条入站拒绝路径直接返回 `AckV3::Invalid`,没有任何日志。
   - 建议日志:一个共用 helper,固定 `msg_kind`(restricted_event、restricted_decision、conflict_evidence)与 `reject_reason`(oversize、author_mismatch、signer_mismatch、signature、diverged、evidence_unusable)。
   - 注意:原因只能来自固定变体,不要来自错误文本。同文件约 170、292 行还有未列出的同类静默返回,应一并处理。级别为 debug 或限速 warn。

### Application:clipboard、transfer、search

6. **clipboard/active/lifecycle.rs:282**
   - 问题:必需 worker(resurface、inbound、peer_online_resync、restore_broadcast)提前返回或 panic 时,监督器只 push 错误并取消整组,只有 `shutdown()` 才暴露。
   - 建议日志:warn,`error_kind = required_worker_stopped`,固定 worker 名(`&'static str`),`panicked | returned`。`JoinError` 分支走 `record_task_join_failure` 并新增 kind。
   - 注意:295-299 行的收尾 drain 同样静默吞 `JoinError`,应一并记录。
7. **transfer/receive/reconciliation.rs:97**
   - 问题:恢复失败只把 `error.to_string()` 存入 `degraded_reason` 并返回 Err,没有日志。接收门保持关闭,无痕。
   - 建议日志:只在 `transfer/file/lifecycle.rs` 的 `ensure_receive_ready` 处写一条 warn,固定 `error_kind = receive_recovery`,附经清理的失败步骤分类,说明接收门保持关闭。
   - 放弃:成功 info 可选。
   - 隐私:不要输出 anyhow 链,可能带路径。`degraded_reason` 里存 `to_string()` 有同类潜在泄露,超出本次范围但应另行跟进。
8. **transfer/receive/reconciliation.rs:176**
   - 问题:重启恢复会改写持久状态,包括丢弃临时接收、强制失败、取消、回滚 artifact,成功时不留任何日志。返回的计数也被调用方丢弃。
   - 建议日志:末尾一条 info 汇总(`reconciled`、`provisional_discarded`、`failed`、`cancelled`、`rolled_back`,均为数字)。每个 attempt 一条 info,带固定 `action`,`entry_id` 与 `attempt_id` 可记。
   - 放弃:各 `?` 路径的 warn,该 Err 已经经 `degraded_reason` 上抛,重复。除非确认上游没有记录。
9. **search/coordinator.rs:529**
   - 问题:`list_entries` 失败后 break,用被截断的列表调用 rebuild,随后记"成功"、置 READY 并 purge。投影跳过只有 debug,标准模式看不到。
   - 建议日志:在 rebuild 函数内加计数器 `listed`、`indexed`、`skipped_projection`、`skipped_pipeline` 和 `list_failed`,写入最终记录。`list_failed` 为真时用 warn。
   - 注意:截断后是否仍置 READY 是行为决策,交负责人,与日志修复分开。不要改 `project_persisted_entry` 的共享签名,在调用点计数即可。

### Infra、Engine、Bindings

10. **uc-infra/space/security/access.rs:1546**(合并 1169)
    - 问题:`contains_active_member` 失败时 `Err(_)` 丢弃 MLS 错误,吊销直接转入 `RecoveryRequired`,函数仍返回 Ok,无任何记录。
    - 建议日志:在提交迁移处写 warn,固定 `previous_phase`、`next_phase`、`reason = mls_state_unreadable`,每次迁移一条。
    - 注意:`MlsGroupError` 需先登记 `log_safe_errors!`。1148 行 `epoch < previous_epoch` 分支同样静默,应一并覆盖。`epoch_mismatch` 已有显式错误,不算静默。
11. **uc-engine/operations/device/member.rs:610**
    - 问题:`map_remove_space_member_error` 把 8 个变体全部转成 `EngineError`,无日志。`CommittedButPending` 与 `StateChanged` 的错误码和 `retryable` 相同,现场无法区分。
    - 建议日志:仿照 `map_roster_error` 在末尾统一写一条 `operation = remove_member`,固定 variant 字面量、`error_code`、`error_category`、`retryable`。事件文本明示 `CommittedButPending` 是本机已提交、其他成员未确认。
    - 注意:`SelfTarget` 与 `TargetNotFound` 属输入错误,降级或不记。
12. **uc-infra/network/iroh/membership_branch_recovery_adapter.rs:216**(同文件 236、252-256、280-292)
    - 问题:sponsor 侧把所有 Application 错误变成 `Err(_) => rejected()`。`read_request` 的超时、超大帧、解码失败返回 None,`accept_bi` 失败静默。
    - 建议日志:各吞错点写固定 `error_kind`(stage 为 request_read、request_decode、begin、issue),附 `io_error_kind`。从 `Unavailable` 与 `Corrupt` 提取分类。
    - 注意:`Rejected` 是正常对端输入,最多 debug。对端可触发,依赖限速。`accept_bi` 失败噪声最大,价值最低。
13. **uc-infra/security/decrypting_representation_repo.rs:199**
    - 问题:AEAD 解密失败被 `Err(_)` 静默丢弃,把密文行当作表示返回。调用方只看到"没有可粘贴内容"。debug 日志在全失败时都不触发。
    - 建议日志:每次调用聚合写一条限速 warn,固定 `error_kind = inline_decrypt`,`error_kind` 取自 `BlobCipherError` 的 downcast(不用 Display),数字字段 `failed_count` 与 `total`。不含 event_id、representation id 与字节。
    - 注意:把密文当表示返回本身是设计隐患,可保留,但应加日志。
14. **bindings/uc-engine-uniffi/src/runtime/worker_join.rs:49**
    - 问题:worker 线程 panic 被 `.map_err(|_| RuntimeUnavailable)` 吞掉。reaper 线程 spawn 失败也静默。
    - 建议日志:新增 `DiagnosticTaskKind::MobileWorker`,在 panic 分支调用 `record_task_join_failure`,不带 panic 内容。spawn 失败分支仅在 panic 分支修好后顺带处理。
    - 注意:该 binding 的 Cargo.toml 未依赖 `uc-observability-contract`,且 AGENTS.md 规定绑定只依赖 `uc-engine`。需要经 `uc-engine` 再导出该函数,不能新增暴露内部的 facade。
15. **bindings/uc-engine-uniffi/src/runtime.rs:1414**(同 1450、861)
    - 问题:worker 的 runtime 构建失败与 `Engine::start` 失败只通过 `started` 通道回传并被 `let _ =` 丢弃。thread spawn 失败(861)也没有记录。
    - 建议日志:每种终止结果一条 warn,`phase = runtime_build | engine_start | thread_spawn`,`error_kind`、`error_code`、`error_category`、`retryable`。这些字段已在 `log_mobile_query_failure` 使用。
    - 放弃:正常退出的 `worker_exited`,只记异常 Err 退出。

## P1

### Application

16. **membership/worker/history_sync.rs:301**(同 326、311)
    - 问题:导出失败被 `.map_err(|_| ExchangeFailure::Unexpected)` 丢弃,随后 `Deferred` 无限重试,与网络延期无法区分。
    - 建议日志:把 `Unexpected` 改成携带固定分类的变体,在第 242 行统一记录一次,不要在两个 `map_err` 处各写一条。本地不变量类(`InvalidPersistedHistory`)用 warn,对端驱动类(`UnknownParent`)用 debug 或限速 warn。
    - 注意:第 311 行不匹配回复同样由对端触发,升级 warn 需限速。不含成员、设备、digest 字段。
17. **membership/recover_conflict/use_case.rs:316**(同 244-260、350-356、508-532)
    - 问题:`decode_persisted_v2` 的 `Err(_)`,以及 Prepare/Advance Invalid、channel 与 ledger 映射,都折叠为 `StableFailure`,不留原因。`StableFailure` 不会重试。
    - 建议日志:在转换点写固定 `stage`(decode_target、prepare_transition、advance_transition、channel、recipient、ledger)加 `error_kind`。`StableFailure` 与 `Corrupt` 用 warn,`Deferred` 用 debug。只在用例内记一次,不在 worker 重复。
    - 注意:优先覆盖 decode、prepare/advance Invalid、channel Rejected/Invalid 与 ledger Conflict,其余不变量检查可不记。不含 conflict id、transition id。
18. **membership/group_update_delivery.rs:253**
    - 问题:`classify_store_error` 把 `KeyEpochError` 映射为 `Deferred` 或 `Corrupt` 并丢弃原因。
    - 建议日志:只在 `Corrupt` 分支写 warn,带 `error.chain`。worker.rs:420 对 `Corrupt` 已有同类 warn,可对齐。`Deferred` 属瞬时,最多 debug。
    - 放弃:`recipients_unavailable` 已有 `Deferred` 聚合。`ack Ok(false)` 与 `failure_record_partial` 最多各一条低频 warn,价值低于 `Corrupt` 那条。
19. **session/activity.rs:190**
    - 问题:`pause_for_lock()` 失败后,补偿 `restore_after_failed_lock()` 与 `membership.resume()` 被 `let _ =` 丢弃。成员维护与连接工作可能在空间仍解锁时保持暂停,只有重启才能恢复。
    - 建议日志:两处回滚失败各一条 warn,`stage = restore_after_failed_lock | membership_resume`,固定 `error_kind`,仍返回原错误。
    - 注意:`stage` 必须在 `ALLOWED_TEXT_FIELDS` 内,否则值渲染为 `<omitted>`。197-201 行的 `restore_after_failed_lock` 已返回聚合错误,不是缺口。
20. **clipboard/sync/sync_runtime/recovery.rs:304**
    - 问题:`PayloadLost` 触发的 `stop_automatic_recovery` 永久写入 `Failed{Internal}`,成功路径无日志。通用 `Err(_)` 只有 debug 且丢失变体。
    - 建议日志:永久停止时写一条 info,固定 `reason = payload_lost`。通用 `Err` 提升为 warn,`error_kind = delivery`,附 `ResendEntryError` 变体名。
    - 注意:不要用 `io_error_kind`,该错误类型不带 io source。`RemoteOrigin` 属正常过滤,保持静默或 debug。恢复由在线事件触发,需保持低频。
21. **clipboard/sync/active_state/peer_online_resync_worker.rs:221**
    - 问题:`Err(_) => return` 吞掉 scope 读取失败,peer 上线后不会收到重同步。相邻两条失败分支都有 warn。
    - 建议日志:`warn!(error_kind = "peer_scope_unavailable", io_error_kind = ..., "peer-online resync skipped: current peer scope unavailable")`。可参照 `fanout.rs:73-81` 的同名 `error_kind`,`io_error_kind` 已导入本文件。
22. **clipboard/sync/active_state/apply_inbound.rs:448**
    - 问题:`Io(_)` 丢弃错误,只有固定句子的 warn。
    - 建议日志:绑定错误,加 `error_kind = pull_io` 与 `io_error_kind`。同函数 401 与 469 行的兄弟分支已有此写法。
    - 放弃:`Unreachable` 与 `NotAvailable` 升 info,属可选。对端离线是常态。
    - 注意:要用真实底层错误验证 `io_error_kind` 能穿透 anyhow 层,否则该字段会被省略。
23. **clipboard/history/maintenance_runtime.rs:166**(同 185、197)
    - 问题:三个维护 pass 的 `Err(_)` 只记固定句子,丢失 `error_kind` 与 source。
    - 建议日志:绑定错误,`error_kind = history_reconcile | history_cleanup | history_retention`,附 `io_error_kind`。保留现有汇总行。
    - 注意:`ClipboardHistoryError` 是否已登记 `log_safe_errors!` 未核实。`error_kind` 加 `io_error_kind` 是安全下限。
24. **clipboard/sync/receive_gate.rs:62**
    - 问题:scope 读取失败(`Err`)与真实"不在可用范围"共用 info,`reason = membership_scope_blocked`。存储故障导致所有入站帧被丢时,本地日志看起来像正常策略拦截。
    - 建议日志:`Err` 分支单独写 warn,`reason = membership_scope_unavailable`。真实拦截保持 info。
    - 注意:`CurrentSpaceMemberScopeError` 只有 `RecoveryRequired` 带 anyhow source,`io_error_kind` 仅在链中有 io 错误时才有值。发送侧没有 `Err` 分支,无需改动。
25. **clipboard/outbound/mod.rs:401**
    - 问题:目录集合成员被 planner 丢弃时,直接返回 `Skipped{reason:"file_set_member_unavailable"}`,无日志。兄弟分支有 warn。该原因字符串还与 metadata-unreadable 分支重复。
    - 建议日志:返回前写 warn 或 info,固定 `reason = planner_excluded_member`,数字字段 `file_candidate_count` 与 `extracted_paths_count`。不含路径与文件名。
    - 注意:此时 blob 已发布后被放弃(约 361 行),属另一个问题,不在本次日志范围。
26. **application.rs:522**(`start_runtime`)
    - 问题:三条回滚分支的 `search.shutdown` 与 `space.on_shutdown` 结果只保存在 `ApplicationStartError` 的 Option 字段里,不在 `Error::source` 链上,Engine 侧只读 `admission_failure()`。回滚失败可能遗留运行中的 search 或 space 运行时,而无人知晓。
    - 建议日志:仅在回滚结果为 Some 时写 warn,`rollback_target = search | space | active_clipboard`,以 `dyn Error` 形式渲染。
    - 放弃:成功 info。Engine 已用 `observe_local_result(SessionStart)` 记录,且 history 与 file-transfer 后台 worker 属正常内部启动。
    - 替代:让回滚失败进入 `ApplicationStartError` 的 source 或 chain 访问器,由 Engine 现有记录渲染,不新增 Application 日志。
    - 注意:`SearchShutdownError` 与 `LifecycleError` 是否已登记 `log_safe_errors!` 未核实。
27. **settings/relay_configuration.rs:338**
    - 问题:被中断的 relay 凭据事务恢复完全静默。`commit` 中回滚出错会覆盖原始保存错误,原始错误丢失。
    - 建议日志:`recover_locked` 的 `Some(_)` 分支写 info"relay settings transaction recovered",不带 URL 或凭据。仅在 recover 失败即将替换原错误处写 warn,固定 `error_kind`,带已登记的错误链。
    - 放弃:每个回滚点的 warn,原错误原样返回时 facade 已记录。
    - 注意:`RelayCredentialsError` 与 `RelayConfigurationError` 需登记 `log_safe_errors!`。

### Engine

28. **uc-engine/operations/space/cancel_join_space.rs:21**
    - 问题:`State` 变体被 `_ =>` 吞掉,统一映射为 `JOIN_SPACE_FAILED_CODE`,宿主看到"加入失败"。
    - 建议日志:`State` 分支写 `error!`,固定 `error_kind = cancel_join_space`。仿照 `cancel_invitation.rs`。
    - 放弃:`NotFound` 属预期,`join_id` 解码失败属宿主输入校验,均不记。
    - 注意:`io_error_kind` 对 anyhow source 不一定适用,退而用固定 `error_kind`。
29. **uc-engine/runtime/host_clipboard.rs:122**(实际只有 128 行成立)
    - 问题:`encryption.session_ready` 为 false(空间锁定)时直接 `return Ok(None)`,发生在 `observe_local_copy` 之前,所以没有 `ClipboardCopyAndSync` 记录。"复制了但没同步"无法归因。
    - 建议日志:一条限速 info,`reason = space_locked`。
    - 放弃:`session_unavailable`(基本是极窄竞态)与远端来源过滤。
    - 注意:字段名用 `reason`,`skip_reason` 不在白名单。标准模式丢弃 debug,因此必须用 info 并限速。
30. **uc-engine/runtime/host_clipboard.rs:61**
    - 问题:`HostClipboardChange::Closed` 直接 return,无日志。之后监听永久失效且无恢复。
    - 建议日志:warn,`error_kind = change_stream_closed`,固定消息 `host clipboard change stream closed; watcher stopped`。
    - 放弃:`let _ = tasks.spawn(..)` 的布尔结果,注册表只在关闭时返回 false,实际不可达。
    - 注意:`Closed` 路径还跳过了 `changes.shutdown()`,与取消路径不一致,属相关问题,不是日志缺口。
31. **uc-engine/runtime/profile_recovery.rs:461**(同 `enter_admission_recovery`:158)
    - 问题:资料恢复状态机的所有转换只发 `EngineEvent`,没有日志。unlock 失败(379)与 `runtime.shutdown(None)` 的结果(346)被丢弃。密钥丢失类事故没有时间线。
    - 建议日志:`publish_summary` 里一条 info,固定 `recovery_state` 枚举名、`restart_required`、`can_submit_passphrase`。379 行写 `error_kind = profile_recovery` 加 `io_error_kind`。346 行改为 warn。
    - 注意:`enter_admission_recovery` 只在首次进入时记,`mode()` 会重复调用。`recovery_state` 需加入 `ALLOWED_TEXT_FIELDS`。
32. **uc-engine/engine/startup_owner.rs:33**
    - 问题:启动任务在独立 `tokio::spawn` 中,panic 时 oneshot 发送端丢弃,调用方只得 1108,无日志。
    - 建议日志:新增 `DiagnosticTaskKind::EngineStartup`,在 `map_err` 闭包里调用 `record_task_join_failure`,同步更新 `as_str`、合同与隐私测试。
    - 放弃:临时加 `error!(error_kind=...)`,新字段未在合同登记,违反文档规则。
    - 注意:panic 路径同时跳过 `lifecycle.requests.fail_startup` 与 `progress.finish`。
33. **uc-engine/assembly/sync_engine.rs:126**(与 outbound_progress.rs:180-189 同属出站进度翻译器)
    - 问题:翻译器 panic 时 `JoinError` 只在 shutdown 时被包成 anyhow 上下文并入 `LifecycleError`。没有健康记录,`DiagnosticTaskKind` 也无对应变体。
    - 建议日志:新增 `DiagnosticTaskKind::OutboundProgressTranslator`,在 `OutboundProgressRuntime::shutdown` 的 `task.await` 返回 Err 处调用 `record_task_join_failure`,原样返回错误。
    - 放弃:`Closed` 分支 warn。该分支在正常拆除时也会触发,最多 debug。
34. **uc-engine/assembly/sync_engine/outbound_progress.rs:165**(同 135)
    - 问题:广播 lag 只写 debug,标准模式看不到。丢失的帧可能是终态帧,该传输会一直停在"传输中"。shutdown drain 的 lag 完全无记录。
    - 建议日志:提升为 warn,数字字段 `skipped = n`,固定 `error_kind = outbound_progress_lagged`,两处都加。不含传输与 peer 标识。
    - 参考:`apply_inbound.rs:246` 与 `inbound/runtime.rs:138` 已用同样写法(warn 加数字 `missed`)。
    - 注意:容量 256,lag 应罕见,shutdown 处那条价值较小。
35. **uc-engine/assembly/mobile_lan.rs:17**
    - 问题:`MobileLanEndpointUpdater::update` 是 LAN 兼容监听器 Stopped、Listening、BindFailed 状态的唯一入口,无任何日志。
    - 建议日志:info,固定消息,`state = stopped | listening | bind_failed`。`state` 与 `error_kind` 都在白名单。`bind_failed` 可另加 `error_kind = mobile_lan_bind_failed`。
    - 注意:消息应写"reported"或"updated",因为 Engine 只是记录宿主上报,并未真正绑定。`update()` 每次都调用,不是 diff,"每次迁移一条"需要与上次状态比较,否则每次调用一条。不记 `base_url` 与 `reason`。

### Infra

36. **uc-infra/network/iroh/session_generation.rs:181**
    - 问题:`quiesce()` 中 `wait_until_drained`、`join_all(closed)`、`shutdown_handlers` 三个无界 await,既无日志也无 watchdog。而且在 `shutdown_at` 里先于 router watchdog 执行(node/shutdown.rs:72 对 :59)。离开或切换空间、应用关闭时卡住,不知道卡在哪一阶段。
    - 建议日志:对三个阶段各包一个 `timeout_at` watchdog,超时写固定 warn,`phase` 与 `budget_ms`,然后继续 await,保留资源所有权。成功路径不记。
    - 注意:严重度中等,宿主可能有外层截止时间。
37. **uc-infra/network/iroh/peer_reachability_adapter.rs:235**(同 285、304-310)
    - 问题:接受侧四处 `connection.close(...)` 前没有任何日志。这是"peer 上下线抖动"类事故的典型盲点。
    - 建议日志:各处写 warn,固定 `error.type` 字面量(confirmation_missing、confirmation_invalid、capacity、confirmation_failed)。已有 warn 使用 `error.type`,有先例。
    - 注意:不要为此扩展合同。`capacity` 与 `confirmation_failed` 最有价值,`confirmation_missing` 可被探测连接触发,只适合 debug 或依赖限速。`space_left` 与 `replaced_after_admission` 是正常关闭,不记。
38. **uc-infra/network/iroh/persistable_addr.rs:118**
    - 问题:`persist_observed_stable_addr` 对编码失败、时钟越界、`repository.upsert` 错误全部静默。`upsert` 失败会留下陈旧地址,后续拨号失败。
    - 建议日志:仅对 `upsert` 的 Err 写一条 warn,固定分类(用 `PeerAddressError` 的 `diagnostic()` 的 category 与 stage),成功保持静默。
    - 放弃:编码与时钟越界两支几乎不会失败,加日志属噪声。不含地址、设备与 endpoint 标识。
39. **uc-infra/space/security/encryption_passphrase_change.rs:74**(合并 wire/mod.rs:499)
    - 问题:启动时 `recover_pending` 重放被中断的口令变更,整个文件没有任何 tracing。恢复是否发生、失败在哪一步、最终结果都不可见。
    - 建议日志:日志放进 Infra 的 `recover_locked`,不在 Engine 启动调用点。仅当真正找到 journal 时写 info,`trigger = startup_recovery` 与 outcome。失败步骤写 warn,固定标签(registration、profile_prepare、install_material、profile_finish、clear_journal)。`NothingPending` 保持静默。
    - 注意:`complete` 也被用户主动的 `apply_encryption_passphrase_change` 使用,步骤 warn 需区分触发来源,或只放在 `recover_locked`。"rolled_back" 属推测,`complete` 只会向前滚动,真实结果集是无待处理、已完成、失败。
40. **uc-infra/security/v3_device_management_reset/mod.rs:151**
    - 问题:设备重置 journal 的阶段转换与 `rewind_staged_target` 崩溃恢复路径完全无日志。卡住或重复的 reset 无法判断走到哪一步。
    - 建议日志:进入 `rewind_staged_target` 时一条 info,失败写 warn 带 source 链。promote 时区分 `already_target` 与 `finalize_from_source` 各一条固定字面量 info。字段用 `previous_phase` 与 `next_phase`。
    - 放弃:在 `save_journal` 里逐次记录阶段变化,噪声太大。
41. **uc-infra/clipboard/spool_manager.rs:233**
    - 问题:容量淘汰成功时静默,只有 `remove_file` 失败才 warn。被淘汰的 spool 文件若尚未被 worker 物化,即丢失剪贴板内容,之后的"Representation missing"无法追溯到淘汰。`plan_eviction` 的"max_bytes 太小仍 break"分支也静默。
    - 建议日志:一次写入内淘汰后写一条汇总 info,固定消息 `spool capacity eviction`,数字字段 `evicted_count`、`freed_bytes`、`total_bytes`、`max_bytes`。无法淘汰足够时写 warn。不含 representation id 与路径。
    - 注意:默认 spool 上限 1 GB,淘汰罕见,信号价值高。
42. **uc-infra/config_migration/adapter.rs:601**
    - 问题:`stage_import` 有入口和成功日志,但 `open_archive`、`staging.write` 的失败都无记录。`preview_import` 完全无日志。导出的多数失败路径也无记录。Engine 侧把 `Err(_)` 折叠为 `internal_error`,变体与阶段都丢失。
    - 建议日志:只在 `stage_import` 与 `export_bundle` 边界写。用户原因的 `InvalidPasswordOrCorrupt`、`IncompatibleBundle` 用 warn,io 与 internal 用 `error!`,固定 `error_kind`,附 `io_error_kind`。
    - 放弃:preview 成功日志。preview 是查询,不该升为业务记录,失败日志仅对 io 与 internal 可选。
    - 注意:先确认 `ConfigMigrationError` 已登记 `log_safe_errors!`。
43. **uc-infra/device/mod.rs:40**
    - 问题:`device_id.txt` 缺失或为空时,重新生成新 `DeviceId` 并落盘,无日志。设备身份被重置,对端视为新设备。`storage.rs:51` 的 rename 回退直接写入也静默。
    - 建议日志:身份重新生成写 info,固定 `reason = missing | empty`。rename 回退写 warn 带 `io_error_kind`。不记 id 值。
    - 注意:仅凭"文件缺失"无法区分首次运行与数据目录被清空,不要声称 `first_run` 就能判定重置。核对该日志在 observability 安装之后才发,否则启动早期事件会丢失。字段名需先在 `ALLOWED_TEXT_FIELDS` 登记。

### Observability 与 Bindings

44. **uc-observability-runtime/src/runtime.rs:362**
    - 问题:远端导出器构建失败被 `.ok()`、`.map_err(|_| ())` 吞掉,宿主只得到 `Unavailable`。此时本地 sink 已存在,来源却无处记录。
    - 建议日志:远端失败时,本地 sink 就绪后发一条 health 事件 `uc.observability.setup_degraded`,带封闭原因枚举(http_client、trace_exporter、log_exporter),并把同一原因写入 `ObservabilityHealth`。不含 endpoint、URL、header。
    - 注意:事件须在 `set_global_default` 之后发,并在合同里登记 health 事件字段与名称,改动不是一行。
    - 放弃:本地目录失败没有本地 sink 可写,只能进入 `ObservabilityHealth` 的原因字段。`LogTracer::init()` 失败噪声大,不单独提。
45. **bindings/uc-engine-uniffi/src/runtime/shutdown.rs:9**
    - 问题:shutdown 超时(22)、请求失败后为重试而重置 `shutdown_pending`(24-28)、lifecycle 通道 Disconnected(46)、join 等待超时(worker_join.rs:63)都无日志。`Drop` 路径(runtime.rs:1396-1399)用 `let _ = join_worker(ZERO)` 吞掉失败的 5 秒 shutdown,泄漏的 worker 线程不留痕迹。
    - 建议日志:固定分类 warn,`phase = request | join`,`error_kind`、`error_code`、`retryable`。`Drop` 路径失败单独写 `drop_shutdown_failed`。
    - 注意:最强的两条是 `Drop` 路径失败和超时。普通 shutdown 超时宿主可见。重试重置并入错误 warn,不单独成记录。成功的 shutdown 保持静默。

## P2

46. **application:device/query_local_device/use_case.rs:28**
    - 问题:`Err(_)` 丢弃设置读取错误,静默回落到 `DEFAULT_DEVICE_NAME`,而该名称会展示给对端。
    - 建议:在现有 warn 上补 `error = e.as_ref() as &dyn Error`(anyhow 用 `e.as_ref()`),记录 `error.chain` 与 `error.root`。不记设备名。
47. **engine/runtime/session_supervisor.rs:844**(实际生产路径为 861、877)
    - 问题:激活清理路径只有泛化 warn。`.is_err()` 丢弃 shutdown 错误类型。844 行在 `dev-tools` feature 下,仅测试用。
    - 建议:改成 `if let Err(error)`,写 `error_kind = prepared_session_cleanup`、`cleanup_after = network_unavailable | activation_failed`。
    - 注意:`io_error_kind` 对 `LifecycleError` 价值有限,需要先确认已登记 `log_safe_errors!`。
48. **engine/assembly/host.rs:258**(`cleanup_import_directory`)
    - 问题:`remove_dir_all(..).is_err()` 丢弃错误,warn 无分类。影响有界:下次启动第 508 行会清空该目录,只有长驻进程(移动端)会累积。
    - 建议:绑定错误,加 `error_kind = clipboard_import_cleanup` 与 `io_error_kind`,对齐 `platform.rs` 的相邻 warn。不记路径。
49. **infra/network/iroh/identity_store.rs:125**
    - 问题:新 iroh 身份密钥生成只有 debug,且带 `fingerprint`。真正的首次生成路径是 `IrohNodeBuilder::bind`(node.rs:798)调用的 `ensure_secret_key`(identity_store.rs:105-112),该路径完全无日志。125、137 行的 debug 对首次运行基本是死代码。
    - 建议:在三处生成点(`ensure_secret_key`、`create`、`ensure`)写一条 info,固定字面量 `origin`,不带 fingerprint,并删除两条带 fingerprint 的 debug。
    - 注意:`fingerprint` 实际在 `ALLOWED_TEXT_FIELDS` 内(module_log_fields.rs:190),与文档"不在白名单"矛盾,原提案关于被渲染为 `<omitted>` 的判断不成立。仍建议不记。
50. **infra/network/iroh/space_admission/server.rs:242**
    - 问题:`CLOSE_BUSY` 关闭(`admission_stopping` 与 `admission_busy`)发生在 `run()` 与 `authenticate()` 之前,不产生任何观测。sponsor 满载(8 个许可,最长占用 120 秒)时,无证据。
    - 建议:仅对 `busy` 写限速 warn,`reason = busy`。`stopping` 属正常关闭,不记。不含 peer id。
51. **infra/network/iroh/active_clipboard/pull_serve_adapter.rs:199**
    - 问题:`Internal(_)` 丢弃 source,warn 只有 `peer` 字段,无 `error_kind`。兄弟 warn 都带固定 `error_kind` 加 `io_error_kind`。Application 的 `serve_pull` 把所有失败包成 `Internal` 且不记日志,这条 warn 是唯一记录。
    - 建议:加固定 `error_kind = serve_internal`。用 `io_error_kind` 提取比 `error = e.as_ref()` 更可靠,后者的 anyhow 上下文层会渲染为 `<opaque>`。不记快照 hash 与错误文本。
    - 注意:`peer` 不在白名单,大概率已被渲染为 `<omitted>`。
52. **infra/space/adapters/rebuild_progress.rs:35**(同 57、`re_pairing_state.rs:44`、`current_space.rs:47`)
    - 问题:`load_target`、`clear_target` 等对非 NotFound 的 IO 失败用 `Err(_)` 丢掉 `io::Error`,统一成 `unavailable()`。权限与磁盘错误无法区分。
    - 建议:这是错误转换问题,不是补日志。改用 `unavailable_from(error)`,与同文件 `store_target` 一致。三个错误类型都已有 `unavailable_from`,每处一行改动。之后由负责人用 `io_error_kind` 分类。
    - 注意:未核对上层用例或 Engine decorator 当前是否记录 `io_error_kind`,保留 source 本身就是对的。
53. **infra/space/security/access.rs:357**(迁移初始化的 keyslot 存储回滚)
    - 问题:`store_keyslot` 失败后的回滚 `delete_keyslot` 与 `delete_kek`(358-359)被 `let _` 丢弃。同文件其他回滚点(1896-1931、2291-2295)都写 warn。
    - 建议:照抄既有模式,`if let Err(err)` 后 `warn!(error_kind = "key_material_rollback", io_error_kind = ..)`。本函数需要自己的 `const PATH`,例如 `migration_initialize`。
    - 注意:需要双重故障才触发,严重度低,但与兄弟点不一致,改动很小。
54. **infra/security/profile_key_recovery.rs:264**(同 270、297、330、256)
    - 问题:KEK 缺失或损坏(270)、KEK 无法解开主密钥(264、297,`is_err()` 丢弃错误)都折叠为 `AwaitingPassphrase`。`refresh_after_authentication` 在 330 行 `activate_existing(&kek).is_ok()` 丢弃错误后走到 `create_and_activate(..., true)` 重建 vault,无痕。目前只有 887-905 的自动打开路径记日志。
    - 建议:`prepare_startup` 在异常分支写 info 或 warn,固定 `reason`(kek_missing、kek_corrupt、kek_unwrap_failed),复用 `diagnostic_reason()`。`refresh_after_authentication` 记录走了哪个分支(activated_existing、rewrapped、recreated)。
    - 放弃:`losses_detected`(256 行)已通过 `ProfileRecoveryChanged` 公开,只算部分缺口。正常 Ready 与 `activated_existing` 保持静默。
55. **infra/security/space_control_generation/mod.rs:296**(同 361;同模式还有 `primary_payloads.rs:132`、`derived_payloads.rs:128`、`profile_storage_upgrade/persistence.rs:176`,未核实)
    - 问题:`prepare` 失败分支 `let _ = remove_directory_if_present(&work_directory)` 丢弃清理错误,残留 `.space-control-generation-*.tmp` 目录,含 `control.sqlite` 的副本或重绑定数据。
    - 建议:换成 match,仅清理失败时写 warn,`error_kind = work_dir_cleanup`,附 `io_error_kind`,成功清理不记。可抽一个共用 `remove_work_directory_best_effort` helper。
    - 注意:不记路径与错误体。严重度低到中。

## 已收窄或放弃的子建议(核验意见汇总)

- 取消类 info:session/recovery.rs 的"cancelled",membership maintenance 的正常退出 info。
- 返回给调用方的错误不重复记:pause/resume 发送失败,`ReceiveReadinessCoordinator` 内部每个 `?` 路径,relay 每个回滚点,`start_runtime` 成功 marker。
- 正常过滤与预期结果:`RemoteOrigin` 跳过,远端推送来源,`NotFound`,`space_left`。
- 不可达或噪声:`let _ = tasks.spawn` 的 false,`LogTracer::init()`,编码与时钟越界。
- 字段名修正:`skip_reason` 改为 `reason`,`from_phase`/`to_phase` 改为 `previous_phase`/`next_phase`,`error.type` 与 `error_kind` 统一按文档用 `error_kind`。
- 事实纠正:`ResendEntryError` 不带 io source,不用 `io_error_kind`。`fingerprint` 在白名单内。`identity_store.rs:125/137` 不是首次生成的真实路径。
- 行为决策,须与日志修复分开:搜索重建在列表截断后是否仍置 READY。

## 被否决条目

155 条候选中 98 条在核验阶段被否决（已被上层装饰器覆盖、会泄露敏感信息、违反分层规则或属噪声），未进入本报告。
汇总 agent 只收到通过核验的 57 条，因此原稿中“0 条被否决”的说法不成立，已更正。
完整逐条结果见 workflow `wf_5470612e-e14` 的 journal.jsonl。

## 建议落地顺序

1. 一次性契约改动:新增 `DiagnosticTaskKind`(成员维护轮、活跃剪贴板 worker 组、出站进度翻译器、Engine 启动、移动端 worker),并处理 binding 依赖 `uc-observability-contract` 的再导出问题。
2. 登记缺失的 `log_safe_errors!` 类型。
3. 逐个补 P0 的吞错点。
4. 处理 P1 与 P2,其中 `rebuild_progress` 等"保留 source"的改动不需要加日志。