# Issue #34 第二切片审查与处理

状态：本切片审查修复与验证完成；最新全套保留一项成员场景失败及其隔离通过结果，不声称全套无失败。按 `$implement` 的 code-review 流程分别执行 testing、context、change-size、breaking-changes 四个独立只读审查，不发布 GitHub 评论。
下列位置是发现时的源码位置；修复后的实现以链接文件为准。重复发现合并，但保留全部实质问题和覆盖建议。

1. **P1，兄弟流完成取消已接收且等待准入的流**（testing/context）：[receiver.rs](../../../crates/uc-infra-p2p/src/network/clipboard/receiver.rs)，原 87 行。已固定并 pin 接受 future，兄弟完成不重建或重置 deadline；可控准入阻塞回归先失败后通过。
2. **P1，准入重检与 ACK 写入无限占位**（testing/context）：同文件原 185、265 行。已把准入纳入读帧绝对 deadline，并限定 ACK 写入时间；两个独立挂起回归先失败，修复后释放额度且下一连接的健康内容仍能结算。
3. **P2，空闲连接提前占用全局活动交换额度**（testing/context）：[connection.rs](../../../crates/uc-application/src/connectivity/connection.rs)，原 103 行。已先按连接有限接收，再领取全局额度，未取额度前不读负载。会话额度 1 的跨 peer 回归先失败后通过。
4. **P2，地址/成员能力异常丢失 source chain，缺少 ErrorClass**（context）：[error.rs](../../../crates/uc-application/src/connectivity/error.rs)，原 46 行。已区分 AddressLookup，地址/准入读取异常映射带 source 的 Io；固定分类穷举，并验证原 io 来源可追溯且公开文本不泄露内容。已诊断的纯建立失败保留原 Offline 契约。
5. **P1，旧请求借用新 epoch 连接**（breaking-changes）：[manager.rs](../../../crates/uc-application/src/connectivity/manager.rs)，原 166、218 行。每份出站租约保留自己的 attempt，额度等待之后重检请求快照；受控移除再添加竞态先失败后通过，有效新连接不被误关。另补排队期间撤销重检。
6. **P2，新 ALPN 绕过既有拒绝拨号入口**（breaking-changes）：[runtime/dispatch.rs](../../../crates/uc-engine/src/runtime/dispatch.rs)，原 912 行。已将现有 DevOperation 拒绝列表同时登记 /2、/1，仍使用原稳定入口之外的既有开发故障能力。
7. **P1，presence 测试误装 clipboard handler**（change-size）：[inbound_peer_rejection_diagnostics.rs](../../../crates/uc-infra-p2p/tests/inbound_peer_rejection_diagnostics.rs)，原 204 行。已恢复 presence 自身 handler；集成目标编译与拒绝诊断测试通过。
8. **P2，结构验收读取已删除旧 receiver**（change-size）：[inbound_peer_single_owner.rs](../../../crates/uc-infra-profile/tests/inbound_peer_single_owner.rs)，原 60 行。已迁移到真实 Uni receiver 与 Iroh admission，保留身份唯一性和日志敏感字段检查，五项结构测试通过。
9. **P2，机械迁移与复杂切换合为一个审查单元**（change-size）：[trace_context.rs](../../../crates/uc-infra-p2p/src/network/trace_context.rs) 与 manager。交付按纯 trace helper 移动、原测试提取、独立观测修复、完整生产切换与宿主/文档验证组织提交；生产切换保持原子，避免留下旁路或长期两套实现。约 1300 行原测试为迁移，不能按全量差异解释为新增逻辑。
10. **P2，旧形状 fixture 隐式选择假准入**（change-size）：[test_support.rs](../../../crates/uc-infra-p2p/src/network/clipboard/test_support.rs)，原 116 行；node 原 1198 行。已删除 DispatchFixture/with_connections 与 install_test_clipboard 兼容形状，测试直接显式提供 EpochOnlyAdmission、registry、policy；成员/授权用例仍走真实 IrohConnectionAdmission。
11. **P3，共同/独占拨号重复准入实现**（change-size）：manager 原 274、369 行。已抽到唯一内部 [acquisition.rs](../../../crates/uc-application/src/connectivity/acquisition.rs)；未交给 record 的连接用 RAII guard 处理准入失败、取消和 panic，第二 backend 保留引用时也会关闭。
12. **P2，两个相同 ACK 无法检测 receipt 串线**（testing）：[receiver_tests.rs](../../../crates/uc-infra-p2p/src/network/clipboard/receiver_tests.rs)，原 311 行。先仅结算第二份 Accepted，并确认第一份仍无回执，再结算第一份 Duplicate；两个 ACK 与各自流匹配。
13. **P2，新 /2 生产撤销/世代围栏覆盖不足**（testing）：node 原 2933、2978 行。已补实际生产 Node 的 /2 双流成员移除与 quiesce/endpoint 不重建；Engine/SQLite/MLS 在途与已有恢复套件分别验证，不能用裸 Router 或四份健康发送代替这些证据。
14. **P2，共享连接独立 trace 未覆盖**（testing）：receiver_tests 原 289 行。两条真实流使用不同 traceparent，各接收 span 校验各自 parent，且 receipt 不同结果；Application 后续关联由既有真实 Engine trace 套件验证。没有为读取不透明 observation 扩大接口。
15. **P2，生产 ALPN 与第二 backend 合同证明不足**（testing）：测试裸 Router 的 set_alpns 与 sink/empty 不足。已补生产 ProtocolRouter 双 offer 选择 /2、旧 offer 选择 /1；双工 backend 运行同一 dispatch/receive/应用 Duplicate 回执代码。真实旧基线二进制直连与仅中继矩阵另外验证互通。
16. **P1，shutdown 清除 flight 后仍可能发布成功**（context 复审）：manager 原 312 行。发布成功前无条件检查 stop/cancel，不依赖 flight 仍在表；领取结果后也检查 stop，重复 shutdown 串行排空。并发完成/关闭回归验证关闭后不能开流。该回归在修复前也通过，发现依据为时序审查，不冒充红灯证据。
17. **P2，请求级成员读取失败跳过连接级关闭**（context 复审）：[admission.rs](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/admission.rs)，原 94 行。已把 request 检查限制为自己冻结的 epoch；当前成员读取及失败整体关闭只由 record grant 执行，减少一次冗余权威读取。
18. **P1，新增 Engine 验收引用兄弟模块私有函数**（testing 复审）：[clipboard_connectivity.rs](../../../crates/uc-engine/tests/space_membership_auto_pairing_e2e/clipboard_connectivity.rs)，原 86、104 行。已使用已知 b_id，并直接执行既有会话诊断 DevOperation。
19. **P2，新增 Engine 同步点早于持久工作**（testing 复审）：同文件原 20、78 行。已观察接收 Application handle_one 下的现有 apply_inbound.execute（已完成解密与接收授权；SQLite 首次写入先于 ClipboardPersist），SQLite 写锁使持久接受未结算后才触发切换/撤销，不增加生产阶段查询。
20. **P2，切换动作无界使 3 秒断言可能假通过**（testing 复审）：同文件原 80 行。出站启动时设 4 秒绝对预算，同时覆盖切换/撤销与发送结束，早于原 5 秒 fan-out 和 SQLite busy_timeout；释放锁再验证远端提交。
21. **P2，多次全局 subscriber 安装冲突**（testing 复审）：同文件原 48 行。采用仓库真实 trace 验收的独占进程模式，明确 ignored 原因并用 nextest 精确执行；不把默认全套中的跳过项计作通过。

22. **P2，系统写入的观测关联跨任务/阻塞线程丢失**（context 复审及真实 trace 验收）：[coordinator.rs](../../../crates/uc-application/src/clipboard/write/coordinator.rs)（255 行），与 [diagnostics/mod.rs](../../../crates/uc-observability-contract/src/diagnostics/mod.rs)（109 行）。write 所有的异步任务和 spawn_blocking 分别恢复现有不透明 ObservationContext；计时/分类仍在既有 Engine decorator。真实 Engine 用例曾发现 clipboard.write_system 成为独立 root，修复后复验。

23. **P2，接收方 SQLite 锁只证明准入提交等待，未证明旧工作排空**（testing 复审）：[clipboard_connectivity.rs](../../../crates/uc-engine/tests/space_membership_auto_pairing_e2e/clipboard_connectivity.rs)（318 行），原 152 行。删除 100ms 推断；改用宿主系统写入 gate 阻塞已持久接受的 OwnedWork continuation，等待原 session_stop_application 开始事件，确认完成入群仍等待，释放后验证新会话和 endpoint 复用。

24. **P2，排空观测可能消费初始化阶段的旧 Notify permit**（testing 最终复审）：同文件原 268、334 行。observer 默认未启用；旧 Space 建立、持久接受、写入阻塞与邀请准备完成后才 arm，本次开始事件用原子 swap 只通知一次。不会把初始化的关闭记录当本次围栏证据。

25. **P2，手动重发根未延续到技术 span 下的 fan-out**（context 复审及真实 trace 验收）：[clipboard.rs](../../../crates/uc-engine/src/assembly/observability/clipboard.rs)（163 行），[resend_entry.rs](../../../crates/uc-application/src/clipboard/sync/resend_entry.rs)（231 行） 与 [dispatch_entry/mod.rs](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/mod.rs)（531 行）。在既有完整 resend 观测入口捕获根 ObservationContext 并 scope 整个动作；技术 span 不进入 OT 层时也能回退到本次父关联，不扩展 facade 或业务 port。
26. **P2，离线目标 trace fixture 要求离线成员全部在线**（真实验收定位）：[clipboard_trace.rs](../../../crates/uc-engine/tests/space_membership_auto_pairing_e2e/clipboard_trace.rs)（176 行）。全部在线刷新移到 offline shutdown 前；正常双节点场景保持原刷新条件，部分投递断言不放宽。原 timeout 不能当产品多流失败。

完整测试命令、实际结果和平台跳过项在[执行计划](2026-10-11-clipboard-connectivity-plan.md)记录。稳定边界见[设计](../../design-docs/clipboard-connectivity.md)。

27. **P2，路径快照测试在 selected path 发布前要求 Direct**（全套回归）：[receiver_tests.rs](../../../crates/uc-infra-p2p/src/network/clipboard/receiver_tests.rs)（978 行）。路径契约允许暂时 Unknown；诊断重复执行发现本机 Clash 198.18.0.1 被选中，按既有规则属于 Unknown。该路径用例固定 loopback 并等待真实 selected path 后再交给原 receiver，保持 Direct 断言，不改变生产策略或用地址候选推断路径。

28. **P1，确认离线后恢复复用旧活动连接**（全套及 context 复审）：[manager.rs](../../../crates/uc-application/src/connectivity/manager.rs)（169 行）、[connection.rs](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/connection.rs)。真实进程崩溃回归两次失败；诊断显示两次 metadata 投递都 pending，60 秒后同时 NotConnected。bridge 在握手后订阅确认离线，用同步 drain 防排队 Offline/Online 先被恢复处理，manager 原 JoinSet 监督 closed 并统一 join；只持原生能力，最后租约仍关闭。明确不可达才映射 Offline，本地 Admission source 保留。新增真实连接测试验证连续 Offline/Online 后新发送获得不同连接，连接层不重发旧内容。

29. **P1，同条内容旧尝试覆盖新恢复成功**（context 复审）：[delivery.rs](../../../crates/uc-application/src/clipboard/sync/dispatch_entry/delivery.rs)（197 行）。原 repo 最后写入覆盖；旧结果覆盖新 Delivered 的行为红灯已取得。原发送 owner 内加每内容/目标的私有弱 token；意图持久成功后才转交权限，检查和最终写入同锁，deferred 强引用延续；失败的新意图不能取消原写入权，未被替换的目标仍结算。跨内容 Superseded 的既有直接写入路径不属于这个同条尝试围栏，单独登记债务，未冒充修复。

30. **P2，六轮退出资源验收受默认 20 秒期限截断**（全套复核）：[nextest.toml](../../../.config/nextest.toml)。直接执行相同二进制 35.06 秒通过，保留任务与句柄无累积断言；仅该用例设置 60 秒有限期限。

31. **P2，后台观察推进后同步游标仍积压**（context 最终复审）：[connection.rs](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/connection.rs)。后台 resubscribe 独立消费未推进原游标，其他 peer 的健康事件也可能使原游标 Lagged，下一次复用误关健康连接。已取得真实行为红灯（`liveness-boundaries-red.log` 两项失败）；修复后两项回归及全套中的对应场景通过。

32. **P1，准入等待期间旧 flight 失活使 Online 恢复错失新连接**（context 最终复审）：[acquisition.rs](../../../crates/uc-application/src/connectivity/acquisition.rs) 与 manager。握手后等待准入期间发生 Offline/Online，恢复请求可加入旧 flight；仅返回 Offline 不能完成只有一次 Online 边沿的恢复。已在任何消息发送之前、原请求准入基线与 deadline 内有界更新已确认失活的连接；不重发已写出的消息。已取得真实行为红灯（`liveness-boundaries-red.log` 两项失败）；修复后两项回归及全套中的对应场景通过。

33. **P2，离线回归把旧 ACK 通道存活当业务提交条件**（全套/测试复核）：[receiver_tests.rs](../../../crates/uc-infra-p2p/src/network/clipboard/receiver_tests.rs)。whole connection 关闭可结束 receipt 等待，finish 返回 false 不代表业务内容被撤销。删除旧 ACK 回送仍存活的错误断言；新连接负载各一次、Accepted、实际拨号计数不放宽，真实远端持久提交仍由 Engine/SQLite 验收证明。

34. **P2，正常重启文件恢复场景被默认 20 秒截断**（全套复核）：[nextest.toml](../../../.config/nextest.toml)。相同已编译二进制直接执行 21.077 秒通过，原内容/文件读回与离线本机工作断言保持；仅这个场景设置有限 60 秒，该项最终全套复验通过。

最终 context 静态复审确认上述更新有界且保留原准入快照、deadline、额度和关闭所有权；Pending 提交和旧结果围栏正确，来源仍完整，未再发现本次新增的必须修复项。运行结果仍以执行计划的实际验收为准。

35. **验证记录，待确认入群场景刷新作用域失败**（12 并发全套）：[admission.rs](../../../crates/uc-engine/tests/space_membership_auto_pairing_e2e/admission.rs)（490 行）。`pending_join_is_not_published_before_final_confirmation` 的 RefreshPeerConnections 报 `scope_timeout` / 1382；未改该测试断言或本次无关的成员流程。隔离执行 22.799 秒通过，但 4 槽与 12 槽全套均复现失败。源码确认 scope 与公开诊断消费同一 MembershipOwner；重启后的 fixture 缺少首次账本加载同步。追加既有 wait_for_active_member_count(2)，确保旧正式成员账本可读后执行原刷新 expect；不释放 Joiner pause，不放宽 15 秒/1 秒禁止提前发布与最终三成员断言。修复后原场景隔离及全套中该项通过。

36. **验证记录，G2 密码学拒绝场景全套失败、隔离通过**（最终 context 复审）：最新完整运行 4226 项，4225 通过、1 失败、35 跳过，560.072 秒；G2 在 70.310 秒报 A 对 B 的 RejectedByReceiver 投影为 None。同一源码隔离执行 27.183 秒通过，前两次全套该项分别 27.291、34.957 秒通过。保留 `workspace-scoped-nextest.log` 与 `group-rejection-isolated.log`，不把隔离通过改写为全套通过。

   源码复审找到两条既有 fixture 风险：[space_work.rs](../../../crates/uc-engine/src/dev/space_work.rs)（449 行）在真实地址解析/握手前消费唯一 tamper，临时失败后重试保存原件；[membership_update.rs](../../../crates/uc-infra-security/src/membership_update.rs)（48 行）的正常历史同步可先推进 B 的 epoch，[access.rs](../../../crates/uc-infra-security/src/access.rs)（1391 行）随后将旧更新按幂等成功处理。两者都可能结清欠账，当前日志不足以确认本次实际分支。这些路径未改，组更新不经过 Clipboard ConnectivityManager；未发现本次新增必须修复点。成员验收负责人应补 tamper、投递结果及接收 epoch 的顺序证据，并用确定性 gate 保证篡改先到真实校验边界；保持原拒绝、重启和后序 epoch 断言，不以增加 timeout 或篡改次数代替。待查项登记到技术债。
