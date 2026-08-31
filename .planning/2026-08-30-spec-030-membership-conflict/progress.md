# Spec 030 Progress

## 2026-08-30

- 已完整读取规格、相关领域词表与 ADR，确认分阶段范围和测试接缝。
- 已检查工作树并标记现有未提交修改为用户工作。
- 已开始 Phase 1 Core 规则的 TDD 探索。
- 完成三个 Core 行为切片：sibling 顺序无关编号、Active/Removed/Absent 资格、Same/ancestor 拒绝。
- 新增敏感 ID 的脱敏 `Debug`，避免 branch/head 摘要进入诊断输出。
- Phase 2 新增加密 ledger conflict record/status/选择 intent 字段，并修正新 generation ledger 初始化。
- 新增关系与 conflict record 单次 CAS 提交测试；Infra ledger migration 定向测试通过。
- Phase 3 已建立并接入 Application facade 的单一 resolve action；本机分支完成、Removed 重新配对、重复幂等和相反选择均由同一 ledger CAS 隐藏。
- 远端 Active 选择当前只保存 `Selected/Pending`；恢复包、transition id 和维护续跑仍属于下一工作切片，Phase 3 尚未完成。
- 增加 Core branch transition 七阶段单调状态机；稳定 transition id 与 transition map 已进入加密 ledger。
- 远端 Active 重复选择测试确认 transition intent 和 ledger revision 均保持不变。
- 新增 `MembershipBranchRecoveryPackageV1`：绑定 conflict/branch/recipient/author/expiry/nonce、目标历史、MLS 恢复密文和内容密钥目录密文。
- Core 验证覆盖目标历史重验、branch 重算、双方 Active 资格、过期、错误 recipient 和损坏授权签名。

## Verification

- `cargo test -p uc-core --test membership_history_v2 conflict_ --locked`：2 passed。
- `cargo test -p uc-core --test membership_history_v2 same_or_ancestor_history_is_not_a_selectable_conflict --locked`：1 passed。
- `cargo test -p uc-core --test membership_history_v2 --locked`：33 passed。
- `cargo test -p uc-application diverged_relationship_and_conflict_record_share_one_ledger_commit --locked`：1 passed。
- `cargo test -p uc-infra space::membership_ledger --locked`：1 passed，其他目标按过滤条件运行 0 项。
- `cargo check -p uc-application -p uc-infra --all-targets --locked`：通过（仅既有 warning）。
- `cargo test -p uc-application resolve_conflict --locked`：2 passed。
- `cargo check -p uc-core -p uc-application -p uc-infra --all-targets --locked`：通过（仅既有 warning 及尚未接入 Engine 的公开 re-export warning）。
- `git diff --check`：通过。
- `cargo test -p uc-core --test membership_history_v2 membership_branch_transition_advances_one_phase_and_never_retargets --locked`：1 passed。
- `cargo test -p uc-application resolve_conflict --locked`：3 passed。
- `cargo check -p uc-infra --all-targets --locked`：通过（仅既有 warning）。
- `cargo test -p uc-core --test membership_history_v2 branch_recovery_package_binds_recipient_branch_expiry_and_authorization --locked`：1 passed。
## 2026-08-30 · Application recovery coordinator

- 新增恢复包获取与无副作用 transition preparation ports。
- coordinator 验证 conflict、branch、recipient、expiry、完整历史与授权签名。
- 单次 membership ledger CAS 原子消费 nonce、保存 `Prepared` transition 并推进为 `Transitioning`。
- 新增成功、提交后重试幂等、跨 conflict nonce 重放零账本副作用测试。

## 2026-08-30 · Maintenance recovery wiring

- `RecoverMembershipConflictUseCase` 实现统一 maintenance step port。
- startup/resume/state-changed/periodic 在 effects 后执行 conflict recovery；peer-online 同样执行。
- `SpaceApplication` 装配 coordinator，Engine 当前显式注入 deferred adapters，使选择保持 Pending 且不发生协议降级。
- maintenance 固定顺序 8 项测试通过，`uc-engine` all-target check 通过。
- `cargo test -p uc-application space::application_tests --locked`：1 passed。
- `cargo test -p uc-application space::membership --locked`：59 passed。
- `cargo metadata --locked --format-version 1`、workspace all-target check、fmt、架构检查和 `git diff --check`：通过（仅既有 warning）。
- `cargo test --workspace --all-targets --locked`：成员相关套件通过；`uc-engine` 115 passed / 3 failed，失败集中于既有 clipboard/search host-adapter 路径。
- 单独重跑 `host_clipboard_change_is_processed_by_the_engine_and_stops_on_shutdown` 仍以既有 QueryHistory unavailable code 1243 失败；失败发生在本切片未修改的 history search 路径。

## 2026-08-31 · Real transition preparation adapter

- Infra 新增 `DefaultMembershipBranchTransitionPreparation`，从加密 active manifest 读取真实 source database generation。
- target generation 使用非零随机 128-bit 标识，并保证不同于 source；准备阶段无目录、数据库或 manifest 写入。
- Engine 已替换 transition deferred adapter；recovery transport 仍显式 Deferred。
- Infra 定向测试 2 项通过，Engine all-target check 通过。

## 2026-08-31 · Recovery package issuer

- Application 新增唯一 recovery package issuer 与材料 preparation port。
- issuer 从 ledger 验证本机目标 branch、认证 source device 与 Active recipient instance、Active 本机签发者。
- 生成五分钟有效期与随机 nonce，绑定完整持久历史，并用当前成员签名能力授权。
- 测试确认错误认证设备在材料 preparation 前被拒绝，合法请求产出的包可通过 Core 完整验证。

## 2026-08-31 · MLS external recovery primitive

- Infra 可从目标 MLS 状态导出带 ratchet tree 的签名 GroupInfo，且不导出成员私钥。
- recipient 使用自身现有签名凭据创建 external commit；OpenMLS 原子替换目标树中相同凭据 leaf。
- 定向测试确认目标端应用 commit 后双方 epoch 和 wrapping key 相同，而各自私有 MLS snapshot 不同。
- 首次编译发现 `VerifiableGroupInfo` 未由 prelude 导出，改为从 `openmls::messages::group_info` 显式导入。
- `uc-infra` MLS 定向测试、workspace all-target check、fmt、架构检查与 `git diff --check` 通过。
- 复审修正 external recovery 新增路径的错误吞噬：稳定分类改为携带 `anyhow::Error` source，脱敏 `Debug` 不输出底层细节；测试验证 InvalidMessage 分类、`source()` 非空和稳定显示文本。

## 2026-08-31 · Authenticated recovery server

- Application issuer 新增 begin 动作；在导出 GroupInfo 前执行与最终签发相同的 branch、source device、recipient 和本机签发者校验。
- 最终签发要求非空 external commit，并把它传给材料 adapter；两阶段不共享未经重新验证的内存授权状态。
- Iroh handler 从连接公钥解析已知成员设备，未知连接、畸形帧、错误消息方向或 Application 拒绝均只返回脱敏 Rejected。
- 共享 Iroh node 已注册 recovery ALPN；生产组装测试增加协议可达性断言。
- Application issuer 定向测试与 Engine 生产协议装配测试通过；workspace all-target check、fmt、架构检查和 `git diff --check` 通过。

## 2026-08-31 · Two-phase recovery wire contract

- Infra 定义专用 Iroh ALPN 和五类有界帧：GroupInfo 请求/响应、external commit 提交、最终恢复包和拒绝。
- 两个请求阶段均显式绑定 conflict、target branch 与 recipient；所有 `Debug` 对绑定与密码学负载脱敏。
- postcard 解码错误保留 source，空密码学载荷、错误版本和超过 4 MiB 的帧稳定拒绝。
- 定向 wire round-trip 与损坏帧 source 测试通过。

## 2026-08-31 · Encrypted recovery session state

- recipient 与 target 的两阶段 staged state、external commit 摘要和幂等恢复包进入 membership ledger 的现有 MasterKey AEAD 载荷。
- session 以 transition id 为稳定键，并绑定 conflict、target branch 与 recipient；ledger 提交前拒绝键错配、空载荷、超限载荷和恢复包绑定错配。
- session 自身负责 recipient completion 与 target commit 的单调、幂等推进，调用方不能直接构造或改写内部状态。
- Space rebuild/reset 原子清除未完成恢复事务；旧 ledger 反序列化时以空 session map 兼容。

## 2026-08-31 · Narrow Iroh recovery client channel

- Application 新增单 peer 两阶段 channel port，明确输入指定 peer 与不可变 conflict/branch/recipient 绑定。
- Infra `IrohMembershipBranchRecoveryChannel` 只负责解析已保存 Iroh 地址、认证连接、有界请求响应和超时，不选择 peer、不运行 MLS、不接触 ledger。
- GroupInfo 与 recovery package 响应方向严格校验；拒绝、不可用和畸形响应保留 source 并稳定分类，Debug 不输出底层详情。
- 保留旧的一步 fetch adapter 作为尚未切换的 coordinator 接口；下一切片完成 Application 编排后删除该浅接口，避免长期双实现。

## 2026-08-31 · Restart-safe recovery client coordinator

- Application coordinator 现在确定性选择 evidence peer，依次驱动 GroupInfo、recipient MLS preparation、external commit 和 recovery package 验证。
- external commit 发出前，recipient staged MLS state 与 commit 已通过 membership ledger CAS 加密保存；重启直接复用该状态，不再请求 GroupInfo 或重新生成 commit。
- recovery package 验证后先进入加密 session，再由最终 CAS 消费 nonce、创建 Prepared generation transition 并推进 conflict。
- 删除旧的一步式 fetch port；Engine 组装真实 Iroh channel，recipient MLS preparation 在真实 adapter 接入前显式 Deferred，不做 LAN 回退。
- nonce 已被其他 conflict 消费时不覆盖 nonce、不创建 transition；此前已完成的必要 staged checkpoints 保留供诊断/安全恢复，因此契约不再错误宣称零次 ledger commit。

## 2026-08-31 · Real recipient MLS recovery adapter

- `DefaultSpaceAccessAdapter` 从当前 generation 的安全仓库加载 Ready MLS state，并调用既有 OpenMLS external recovery primitive。
- staged payload 同时保存 recipient MLS snapshot、共享 exporter wrapping key 和 epoch；该 payload 只返回 Application，并在 external commit 发送前进入 MasterKey AEAD ledger。
- Engine 的 recipient deferred adapter 已删除，组装根复用同一个 space access adapter 的窄 recovery port。
- 复核 target 侧发现直接 apply commit 后再构造/保存响应存在崩溃窗口，因此没有接入不安全实现；下一切片必须使用现有 TargetPrepared/TargetCommitted session 完成 prepare/commit 分离。

## 2026-08-31 · Shared recovery transaction key

- transition id 推导从 Application resolve use case 移入 Core `MembershipBranchTransitionV1::derive_id`。
- recipient 与 target 现在可仅凭 conflict/target branch 得到同一稳定事务键，无需扩展 wire 携带另一份可错配标识。
- Core 测试覆盖重复推导稳定、非零和不同目标分支隔离。

## 2026-08-31 · TDD target recovery transaction

- 第一轮红测确认旧 issuer 会返回 package 但不保存 target session、也不提交安全材料；绿色实现增加 prepare payload、TargetPrepared checkpoint、material commit 和 TargetCommitted checkpoint。
- 第二轮红测确认重复请求会重新 prepare 并因 session 冲突被拒绝；绿色实现改为在材料计算前读取 target session，并按 external commit digest 返回缓存 package。
- 故障注入测试确认 material commit 首次中断后，重试从 TargetPrepared 续跑，prepare/签发只发生一次，commit 可安全重试。
- Application port 已明确 prepare 返回 staged target material，commit 接受该 opaque payload；真实 Infra 实现属于下一切片。

## 2026-08-31 · Real target recovery material adapter

- 红测先固定 target prepare 无持久副作用、commit 后 epoch 前进、相同 staged material 重试幂等，以及无效 payload 保留稳定分类与 source。
- `DefaultSpaceAccessAdapter` 在 MLS 快照上应用 recipient external commit，生成下一 epoch 内容密钥并使用双方共享 wrapping key 密封内容密钥目录和恢复确认。
- staged `SpaceKeyMaterial` 只进入 Application 已有 MasterKey AEAD recovery session；commit 重新验证 space、MLS state、key catalog 与单步 epoch，再持久化和安装。
- Engine 已直接注入真实 target material port，并删除 `DeferredMembershipBranchRecoveryMaterial`。
