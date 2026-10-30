# 技术债跟踪

只记录已经有明确证据、负责人边界与退出条件的债务；模糊想法应先进入设计讨论。

| 项目 | 状态 | 依赖/退出条件 | 计划 |
| --- | --- | --- | --- |
| 冻结 a07 Relay 互通完成后关闭返回 1104 | #345 首轮 CI 已证实，本机两次原场景复验通过，根因未明 | 网络／关闭负责人保留冻结旧 SHA、15 秒关闭预算与完整内容断言，定位实际未结算参与者；驱动 EPIPE 已有独立修复，不能将证据留存修复当成旧端关闭修复 | [原失败及后续结算](completed/2026-10-28-membership-branch-recovery-connectivity-plan.md#合并后最终-ci-结算2026-10-29) |
| 网络日志目录故障验收的子进程间歇退出 | 已修复：#350首轮workspace失败后，本地两核第11轮复现DirectoryNotEmpty并修正故障注入竞态；12项、同两核100轮及33dc6f03完整workspace4990项均通过 | 网络日志与夹具负责人已核对本轮完整Engine原XML；相同两核100轮及原12项报告继续保留；保持目录删除、阻塞恢复与业务日志隔离断言，保留首轮及本地红转绿证据，不把原CI缺失的stderr写成已取回 | [首轮证据与诊断修复](completed/2026-10-28-membership-branch-recovery-connectivity-plan.md#首轮交付候选的观测失败与主动取消) |
| 管理下载1GiB大流资源原场景超时 | 本轮首次工作区91.723秒超时，隔离70.170秒及最终完整工作区78.520秒通过，根因未定 | Blob传输完整负责人定位原超时的实际阶段，保留内容、SDK tags与连接结算断言，在原90秒预算和完整工作区环境复验；不以隔离通过认定修复 | [首次全量反馈](completed/2026-10-30-issue-34-followups.md#全量首次反馈) |
| 成员证据验证跨控制数据库切换继续使用旧结果 | 已复现并完成本地修复验收：四项首红转绿、原成员56通过、最终工作区5004通过 | 成员证据完整负责人保留最初数据库代号；验证中／提交排队时切换返回暂不可用，分歧写入、一致更新、无写回复四项首红转绿；原F2／G2等间歇异常根因仍独立保留 | [后续计划](completed/2026-10-30-issue-34-followups.md) |
| 成员收敛、Space 切换与分支冲突恢复间歇失败 | 第九片本地全集分别出现失败，后续全集复验通过，根因未明 | 成员／会话及恢复负责人分别定位原收敛超时、激活后恢复超时和冲突未清零；保留各次原日志与同代码复验结果，不把三个现象认定为同一原因，不增加期限掩盖 | [第九片执行记录](completed/2026-10-28-membership-branch-recovery-connectivity-plan.md) |
| 冻结a07旧旧Relay文件接收间歇拒绝 | #332／#335原CI失败，后续完整原矩阵通过；#350首轮及SQLite修复候选33dc6f03的CI均在升级前旧旧阶段再次出现相同拒绝，根因未定 | 网络／接收及兼容验收负责人定位原B到A文件`peer_rejected`／A的`apply_failed`；旧版已丢失部分source chain，须保留固定旧SHA、原文件断言和失败证据，建立实际反馈环并明确兼容处置；不以通过重跑称修复 | [历史失败](completed/2026-10-24-transfer-progress-connectivity-feasibility.md#映射期间发现的旧旧relay失败)、[本次候选失败](completed/2026-10-28-membership-branch-recovery-connectivity-plan.md#交付候选-350-的首次网络失败) |
| 成员F2恢复场景偶发多一个issue | 本片首次完整回归失败，隔离及后续完整56项通过；根因未定 | 成员恢复及测试负责人定位实际产生多一个issue的时序，在原全并发场景验证原计数和恢复断言；保留首次结果，不增加超时或将smoke代替全集 | [首次失败与完整回归](completed/2026-10-24-transfer-progress-connectivity-feasibility.md#当前候选完整回归与首次失败保留) |
| 本地产物统一准备 | 待实现 | 脚本、清单和三目标验证完成 | [计划](active/local-artifacts-preparation.md) |
| 双设备配对本机耗时仍约 5 秒 | 实施中 | 可信纯网络扣除门禁通过，预热后不少于 20 次的本机耗时 p95 小于 1 秒 | [计划](active/038-pairing-local-latency-budget.md) |
| 历史本地调试调用点尚未逐项分类 | 待实现，当前默认拒绝输出 | 每个生成清单项完成删除、安全本地化或明确保留，真实 sink 隐私检查通过 | [计划](active/039-local-debug-inventory-cleanup.md) |
| 本地日志并发完成记录偶发缺失 | 已在固定 rc.15 基线复现 | 运行期观测负责人核实并发记录到文件刷新的完整边界；`local_correlation` 独立进程连续运行不再出现 3/4 条记录 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| 十设备分叉场景中新增成员一直等待加入 | 已在固定 rc.15 基线复现 | 成员准入负责人关联 F7 中新成员的启动及邀请处理；完整测试在独立进程通过，不能扩大等待时间或改成刷新连接促成加入 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| iroh-blobs 定时回收延后运行时退出 | 已确认锁定依赖与旧版具有同一行为 | 依赖负责人修复或升级 `b33af91e` 的关闭流程；连续创建/关闭 Engine 后系统线程数返回基线，不能只检查任务登记数 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| 网络身份被替换后没有恢复方式 | 已能检测并阻止静默替换，恢复方式未定 | 先按只读现场证据判定应“重新配对引导”还是“身份更新事件”，另写计划实施；在此之前不得公开 `re_pair` 恢复动作 | [发现记录](completed/2026-09-23-inbound-peer-admission.md#s5b-恢复方式待定不在本计划实施范围) |
| 已有空间设备的网络身份文件在两次运行之间消失 | 原因未查明（`dev` 现场一次） | 资料与身份存储负责人查明删除来源（疑似开发构建未设 `UC_PROFILE` 落到 `dev` 资料目录）；复现后修复并保留诊断 | [发现记录](completed/2026-09-23-inbound-peer-admission.md#实施记录) |
| 网络身份私钥以未加密文件保存，不符合“凭据与密钥材料只存系统安全存储”规则 | 现有实现与规则的差距，未获任何豁免（2026-10-21 规则同步时核对） | `uc-infra-p2p` 的 `IrohIdentityStore` 经 `FileSecureStorage` 把 32 字节 Ed25519 原始私钥写入私有目录 `iroh-identity/`（Unix 权限 0600，无加密）。安全负责人决定迁入系统安全存储（须保持已配对身份，另立迁移计划）或经明确批准登记为例外；在此之前不得在规则中把它列为已有例外 | [规则](../SECURITY.md) |
| `vault/keyslot.json` 中口令包装的 MasterKey 未被规则措辞覆盖 | 规则措辞差距，不是明文泄漏 | KeySlot V1 只保存 Argon2id 参数、盐与经口令派生 KEK 以 XChaCha20-Poly1305 包装的 MasterKey，不含明文密钥（`uc-infra-crypto` `crypto_model.rs`）；规则只批准 `vault/profile-secrets-v1` 为文件形式密钥材料。安全负责人决定明确批准该格式或调整规则措辞 | [规则](../SECURITY.md) |
| Core 规则外泄、效果义务无约束力、存储格式与运行时依赖在 Core | 已盘点，修复未开始 | 清单逐项修复或登记为规范例外；Core 自动检查在 CI 阻断 | [计划](active/2026-09-23-core-boundary-remediation.md) |
| 新会话发布前的出站连接被本机拒绝，每次加入的最终确认多等约 1 秒 | 已定位，候选方案因暴露重启补传问题暂缓 | 先修复同身份重启后握手超时及投递恢复只尝试一次的问题；`interrupted_file_transfer_recovers_after_receiver_process_restart` 连续 10 次通过，且最终确认首连不再 `locally_rejected` | [发现记录](../../.planning/2026-09-26-session-outbound-before-publish/findings.md) |
| 接收方以同一身份、同一端口重启后，文件续传偶发等不到投递 | 已复现，原因即上一行记录的两个既有问题；该测试已进入 PR 必需门禁 | 本机 nextest 全工作区构建下 `interrupted_file_transfer_recovers_after_receiver_process_restart` 失败 2/10（1381 修复前）与 3/10（修复后），单包构建 1/10，均为等待投递超时；网络与投递恢复负责人修复同身份重启握手超时及投递恢复只尝试一次，之后连续 10 次通过 | [发现记录](../../.planning/2026-09-26-session-outbound-before-publish/findings.md) |
| 跨内容自动替换的迟到投递结果可覆盖 Superseded | 静态审查确认既有写入断点，未做独立故障复现 | Application 发送与恢复负责人把 supersede/停止恢复直接写入纳入同一尝试围栏；阻塞旧 deferred、标记 Superseded 后释放旧结果，状态仍终止自动恢复；新意图写失败不得丢失旧有效回执 | [审查记录](completed/2026-10-11-clipboard-connectivity-review.md) |
| 成员 G2 密码学拒绝场景偶发未投影拒绝 | 最新全套失败一次、同源码隔离与此前全套通过；实际原因未证实 | 成员验收负责人记录 tamper 消费、投递结果及接收方 epoch 顺序；以确定性 gate 保证真实拒绝边界，原拒绝/重启/后序 epoch 断言在完整并发全套稳定通过；不增加篡改次数或超时掩盖 | [审查记录](completed/2026-10-11-clipboard-connectivity-review.md) |
| 已完整接收目录的再次激活触发重复 pull 并存储失败 | 已修复并本地验收：本轮独立进程首红、真实SQLite首红均红转绿，四种内容Direct／Relay共8项通过；存储全集283通过、3忽略；最终工作区5004项及根交付门禁通过 | 完整流程负责人不变；Infra复用已认证清单实时核对实际成员、内容及结构，不新增存储格式或调用方步骤；固定旧端和真机不据此扩张验收 | [后续处理计划](completed/2026-10-30-issue-34-followups.md) |
| 目录旧v1身份不能区分根类型及同名根分组 | 已由真实SQLite回归确认；本轮复用检查保守拒绝歧义清单，身份格式本身尚未修复 | 内容身份负责人先确认发布冻结边界并设计兼容与存量演进方案；不得静默改变v1字节或从根路径类型推断原始意图；普通无歧义完整目录仍可直接复用 | [ADR-010复用边界](../design-docs/decisions/010-directory-sync-as-file-set-manifest.md#旧v1身份的复用边界) |


关闭项目时记录验证证据，更新稳定文档，并将对应计划移入 `completed/`。


## Configuration import: lifecycle and upgrade-journal adoption

Status: implementation and public Engine acceptance are tracked by the host-configuration plan.
The immutable checkpoint `74122413` retains the original unresolved gate and negative evidence.

The original defect used the pre-import target lifecycle and later reused target upgrade state under
source keys. Synthetic diagnosis proved that the imported encrypted vault already carried the admission
key. No new independent-key export is needed. A profile-only V3 snapshot also omitted control security
material and could not restore a session. Merging V3 credentials into a legacy upgrade source did not
preserve their authenticated generation bindings.

The repair gives the existing offline import owner a durable adoption transaction. It retains target
state and upgrade backups, verifies an archive, then installs the source V3 manifest, both consistent
database snapshots, content-key catalog, encrypted blobs and existing encrypted vault. Startup rereads
the imported lifecycle. Failure retains the journal and staging for replay. See the stable
[configuration import contract](../design-docs/uc-engine-interface.md#configuration-import-adoption).

The user approved explicit rejection of incomplete schema-1 bundles before target mutation. These
bundles require a new export from the retained source installation. Their historical 1101 assertions
are preserved as repro evidence and do not count as successful imports.

Acceptance remains evidence-based: the final task8 receipt must identify the tested source SHA,
public fresh/existing-install runs, first/subsequent starts, partial adoption recovery, retained target
backup verification and public passphrase recovery. Task7 old-anchor mbx downgrade execution and
native/downstream acceptance remain separate gates.
