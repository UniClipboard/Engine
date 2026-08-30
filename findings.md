# Spec 028 Cleanup Findings

## Current State

- `SpaceAdmissionAggregate` 已拥有类型化 admission 状态和取消转换。
- 旧取消、pending transition 与 current-join 投影已全部迁移，不再读取 membership ledger。
- `SqliteSpaceAdmissionState` 已加密保存新 Aggregate，并维护
  `current_local_join_id`。
- 现有 recovery load 会过滤无 recovery payload 的 Candidate，因此不能直接作为
  “读取当前加入”接口。
- `SponsorAdmissionSecurityDelivery` 原本只作为新安全 preparation 的传递载体；将等价但归属
  正确的 `PreparedMemberSecurityDelivery` 放入 Application 端口后，Core legacy 模块即可删除。
- 取消状态不能复用 recovery load：Candidate 没有 pending recovery，但仍是合法取消阶段。
- 新 `CurrentJoinAdmissionStatePort` 按 JoinId 读取并携带版本 token，Infra 在同一事务内
  校验 current pointer、record version 和密文替换；Facade 已不再读取旧 ledger。
- membership ledger 的 legacy 准入字段没有剩余生产消费者，字段、操作模块与旧 outbox
  已删除；空间重建继续只清理成员事实和效果。
- Core legacy `AdmissionRejectionReason` 的最后一个消费者是公开 current-join 投影；新协议
  枚举覆盖同一稳定原因集合，切换后不再阻止删除整个文件。

## Invariants

- `SpaceAdmissionProtocol` 是 start/cancel/handle/recover/complete 的唯一完整负责人。
- Application 只接收角色能力对象，不接收完整 Aggregate。
- 取消的读、版本校验、写和 current pointer 更新必须由同一个状态端口隐藏。
- 不改变持久化明文边界；业务负载继续经 MasterKey AEAD 加密。
- 错误分类必须保留 source chain。

## Delivery Audit

- Spec 028 文件头仍写“Draft，尚未实施”，且 Acceptance Criteria 未记录结果，文档状态落后于实现。
- Engine 已在 Router 启动前安装新 admission endpoint，Facade 的 Join/Cancel/状态/完成动作均调用
  `SpaceAdmissionProtocol`。
- 仓库门禁和聚焦测试不能替代 Engine 双实例、真实 SQLite/Iroh、明文探针与实体设备矩阵。
- 仓库已有可直接执行的 Engine 双实例 E2E、真实 admission state/auth、OPAQUE RFC、绑定 contract
  和 plaintext probe tests；实体设备宿主存在，但当前环境是否连接设备仍需单独探测。
- 全 workspace 首个阻断与 admission 状态机无直接关联，位于配置迁移 E2E 的导出夹具；需要证明
  是测试夹具漂移、环境依赖还是生产导出回归后再决定修复位置。
- 配置迁移失败的正确假设是夹具漂移：安全材料初始化不等于完整 Space 激活，生产
  `CurrentSpaceResolver` 才拥有 `.current-space-id-v1`；测试漏写该必需文件，并错误保留已退休
  `.setup_status`。生产导出前置条件无需放宽。
- `space_membership_auto_pairing_e2e` 整体受 `dev-tools` feature 控制；默认 workspace 测试显示
  0 项不能作为证据。启用 feature 后，legacy DevOperation 编译断点成为必须先清理的验收问题。
## 2026-08-30 Engine E2E 验收发现

- 原 `space_membership_auto_pairing_e2e.rs` 仍以旧 `WorkspaceConvergence` 快照和直接成员移除决策为主要断言，与 Spec 028 “只使用新消息和新 aggregate”不符。
- 恢复旧 facade 方法会重新建立被 clean cutover 删除的架构入口；正确处理是让 Engine E2E 只观察稳定 `JoinSpace` 结果、重启恢复和内容权限，协议乱序/replay/CAS 由 Core/Application/Infra 测试覆盖。
- Engine 启动配置的 build version 会参与协议版本解析，测试值也必须是合法语义版本。
- 新 admission 后台能到达非拒绝 terminal；真正缺口在 Engine 生命周期：首次加入的 `requires_session_transition` 为 false，后台激活完成后没有通知 `SessionSupervisor` 重建当前 session。
- `SessionSupervisor::install_new_session` 只在安装期检查 pending transition，dispatch 只处理调用期间已知的同步 transition；两者都没有覆盖运行期后台准入完成。
- current join 指针清理不是根因。保留指针只会让查询继续显示 Pending，不能让旧 session 看到新 Space。
- `SpaceAdmissionProtocol::recover_pending` 原先在每轮开头直接执行 `recover_activation()`；普通维护会抢在 Engine 观察前消费短暂 transition，轮询频率无法可靠修复。
- 正确稳定边界已有 Application 测试支持：maintenance 推进到 pending activation，只有 `complete_pending_space_transition()` 完成本机 Space 切换；Engine 随后重建 session。
## 2026-08-30 准入运行期红测补充定位

- Sponsor Complete 生成的 activated security 原先只随 aggregate 持久化，没有生产消费者；Joiner target generation 也只安装安全材料与关系投影，没有写 membership ledger。
- 增加两侧正式激活后，重启红测不再卡在 Sponsor roster 或 Joiner scope，剩余症状是 Joiner 重启后 relationship store 报 locked，P2P 握手超时；问题边界已转移到 active-generation keyslot/session 恢复。
- active-generation manifest、目标 keyslot 和数据库均正确；真正缺口是 `SessionSupervisor::resume()` 构建 session 后未调用静默恢复。改为每次安装均尝试恢复后，重启传输转绿。
- 跨 Space 切换的目标数据库来自 source snapshot，membership ledger 可能已有非零 revision；目标 ledger 安装必须基于实际 revision/history digest 做 CAS 替换，不能假设空库。
- `AdmissionSpaceTransitionPreparationV2` 已是 Application/Core 验证后的激活计划；Infra 再解码并验证成员历史会造成职责重复，也让合法的端口夹具依赖密码学编码细节。Infra 应仅安装计划中的目标关系与加密账本。
- `single_space_admission` 成为最新 migration 后，旧测试用固定次数回滚时偏移了一位；测试必须越过新增 migration 后再验证 revision down 与 legacy 清理 migration。

- `space_membership_auto_pairing_e2e` 中保存的异常终态不是 `Active::PendingSettlement`：它没有当前回复、不是 settled/rejected/superseded；结合终态枚举，实际只可能是 `RecoveryRequired`。
- 因此 watcher 没有观察到 pending transition 是结果，不是根因。根因位于加入协议恢复流程：状态在到达 `Activating` 前因协议/状态错误被转为 `RecoveryRequired`。
- 下一步应记录稳定的 recovery category 或沿 `save_recovery_required` 调用点定位具体分支，不能继续调整 watcher 猜测。
- 完整邀请的 admission route 被 Joiner start material 二次包装，导致初次 transport decode 失败；应原样保存邀请内已经编码好的路由。
- Sponsor continuation 加载错误复用了只描述 Joiner recovery 的 `pending_recovery()`；应通过 Sponsor 专属只读视图获取凭证。
- Sponsor state load 在查找已有 admission 前解析 `JoinRequest` invitation id，使后续阶段消息被判损坏；该解析只能发生在 fresh admission 分支。

## 2026-08-30 clean-cutover 清理审计

- Engine 仍调用 `IrohNodeBuilder::install_pairing`，生产 Router 注册 `/uniclipboard/pairing/2`。
- `IrohPairingSessionAdapter` 同时承担邀请发布/短码解析与旧 session transport；新准入仍需要前者，不能整模块直接删除。
- 新 `/uniclipboard/space-admission/1` transport 已独立安装；清理应把 invitation issuer/address/resolver 提取为 discovery adapter，不为旧业务 handler 保留 ALPN。
- `/uniclipboard/pairing/1` compatibility probe 仍会主动探测旧协议，违反 Spec 028 的 no-old-ALPN 决定，应随 session transport 删除。
