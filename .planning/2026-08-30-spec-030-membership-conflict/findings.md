# Spec 030 Findings

## Initial state

- 当前分支领先远端 18 个提交，并有 7 个代码文件及架构圣经未提交修改；主要属于 Spec 029/Admission 重启诊断，Phase 4/5 可能重叠，必须保留。
- `VersionedMembershipHistory` 已是签名单父历史、关系与成员事实的事实来源；新 policy 应建立在验证后的 history 上，不复制验证逻辑。
- `MembershipLedger` 已是加密原子边界；conflict record 应成为 ledger 模型的一部分而非独立明文 repository。
- 规格的 Open Questions 不阻止核心实现：本地选择语义、短期恢复包时长和 CI 分层可以采用保守内部默认，不改变公开 contract。

## Invariants

- conflict id 与传输 peer、到达顺序无关。
- branch id 只绑定 lineage 与完整目标 head。
- Removed 不能恢复旧成员实例；Absent 不是可选目标。
- 用户选择 intent 一经持久化，后台不得改写。

## Phase 1

- `current_position().history_digest` 包含完整持久历史与决定，branch id 必须同时绑定它和 head；只使用 event head 会把同一移除的 Accept/Reject 分支误判为相同。
- 共同祖先使用事件链证明，不使用 branch-specific history digest；conflict id 对排序后的两个 branch id 做摘要，因此观察顺序不影响结果。
- 本机实例在目标 `active_members` 中才可恢复；保留历史凭据但已不在 `effective_members` 中表示 Removed，需要重新配对；尚未激活或完全缺席均不可选择。

## Phase 2

- SQLite `membership_ledger_state.encrypted_payload` 已对整个 ledger 使用 profile MasterKey AEAD；conflict record 追加到 `LoadedMembershipLedger` 即进入既有密文与 transaction/CAS 边界。
- conflict record 的 `Debug` 只输出选择分类、阶段、revision 和计数，conflict/branch/peer/transition 标识全部脱敏。
- 多个 peer 的同一分支证据使用集合保存，不把 transport 来源纳入 conflict id。

## Phase 3/4

- 远端 Active 选择的 transition id 可由 conflict id 与 target branch id 领域分隔摘要稳定产生；不依赖随机数即可跨 CAS 重试和重启保持唯一。
- branch transition 使用独立七阶段 Core 状态机，source/target generation 必须不同，且只能推进到直接 successor；阶段对象随 ledger 整体加密。
- 恢复包不能信任 transport 提供的 branch 声明：验证入口必须重新解码完整目标历史、验证全部历史签名并重算 branch id。
- recipient 与授权签发成员都必须位于目标 `active_members`；只保留历史 credential 的 Removed 实例不能签发或接收恢复。
- nonce 是否已使用依赖持久状态，归 Application ledger CAS；Core 只拒绝零 nonce 并把 nonce 纳入授权签名载荷。
- 恢复包 nonce 必须绑定首次消费它的 conflict，而不能只保存一个无归属集合；这样才能区分同一流程重试与跨 conflict 重放。
- transition preparation port 只允许返回无外部副作用的 `Prepared` 计划，generation 文件写入必须留给 CAS 成功后的后续阶段。
- conflict recovery 必须位于 membership effects 之后、group update 与受限交付之前；这样先恢复已有本地安全欠账，再决定是否具备切换准备条件，且损坏状态能阻断后续权限扩展。
- peer-online 是恢复包重新获取的直接触发条件，不能像 admissions/effects 一样跳过。
- active manifest 的 `database_generation` 是当前完整数据库 generation 的真实来源；transition preparation 不应从 ledger revision 或随机 source 推断。
- recovery package 的安全签发能力尚不存在；在它能绑定当前有效成员签名、MLS 恢复密文和内容密钥目录前，不应只安装一个会稳定拒绝的 Iroh 协议外壳并宣称 transport 已完成。
- Iroh endpoint 只负责把认证连接映射成 source device；recipient instance 与 source device 的对应关系必须由 Application 使用目标完整历史重新验证。
- 恢复材料密封方式归 Infra capability，Application 只接受两个非空 opaque ciphertext，并负责 package 绑定、时效、nonce 与成员授权签名。
- OpenMLS 0.8 external commit 会按相同签名公钥自动移除旧 leaf；因此 Active recipient 可使用 sibling 状态中自己的签名私钥重新加入目标 group，不需要也不得复制目标设备的 `MlsClientState`。
- 内容密钥目录必须在 external commit 后使用新 epoch exporter wrapping key 密封：目标端先给签名 GroupInfo，recipient 返回 external commit，目标端应用到 detached state 后才得到与 recipient 相同的 wrapping key。因此真实恢复协议是两阶段握手，不是单次请求响应。
- Iroh handler 只能把认证连接公钥映射为 source device；begin 与 submit 两阶段都必须重新调用 Application issuer 复核 conflict、branch、recipient 和 Active 状态，不能让第一阶段认证结果跨网络往返隐式延续。
- 两阶段 Iroh wire 必须在两个请求中重复绑定 conflict、target branch 和 recipient；连接身份只证明 source device，不能替代 Application 对目标历史的成员资格复核。
# 2026-08-31 · 恢复事务持久化边界

- membership ledger 已是按 generation 绑定的整体 MasterKey AEAD 载荷，恢复 staged state 放入该 ledger 可复用现有加密、CAS 与重启恢复边界，不应另建明文表或文件。
- transition id 是恢复事务的稳定索引；session 内再次保存并校验它，能在反序列化和提交前识别 map key 与载荷错配。
- target 必须缓存签发后的 recovery package，才能在 external commit 已应用但响应丢失时返回同一结果；recipient 必须保留 staged MLS state，直到最终 generation 提升完成。
- 状态推进由 session 对象隐藏，recipient 与 target 角色不能互相转换，重复完成操作只接受同一绑定结果。

# 2026-08-31 · Recovery client 边界

- 现有一步 `FetchMembershipBranchRecoveryPort` 无法表达 GroupInfo 与 external commit 之间必须持久化的 recipient staged state，不能直接由 Infra 实现而不破坏重启安全。
- Iroh client 应是窄 channel：只对一个由 Application 指定的 peer 做一次认证请求。peer 选择、MLS preparation、ledger CAS 和重试属于 Application coordinator。
- 下一切片完成 coordinator 切换后应删除旧 fetch port；否则两套抽象会让 Infra/Application 边界继续含混。

# 2026-08-31 · Recovery coordinator checkpoint 顺序

- recipient external commit 属于不可安全重建的密码学结果，必须先把它和 staged MLS state 加密提交，再允许网络发送。
- 最终恢复包可以在单独 checkpoint 验证并保存；之后 generation transition preparation 即使失败或重启，也不必再次触发目标端 external commit。
- nonce 冲突只能在取得最终包后确定，因此此时 staged checkpoint 已合法存在；安全保证应表述为不覆盖 nonce、不创建 transition，而不是零 ledger commit。
