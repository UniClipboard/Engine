# Issue #34 第七切片：文件／blob 受管传输执行计划

状态：**实现与验收完成，已归档**（2026-10-29补结算）。统一验收候选为后续 `1d1fdb59`（#340），不是将 #230 原审计失败改写为成功；最终结算见文末。
唯一需求与验收来源：[实现规格](2026-10-21-blob-connectivity-spec.md)；
实现原因和 SDK 取舍见[实现方案与关键取舍](2026-10-21-blob-connectivity-feasibility.md)。

## 完整负责人及交付边界

- Application `transfer` 统一承担接收、调度、重试、取消及实际结算；模块内部按职责拆分。
- 调用方只提交完整动作；Infra 执行 SDK、文件和存储能力，Engine 只装配与映射稳定操作。
- 网络完成不等于业务完成；本地输出、提交和清理责任结算后才能释放原许可与保护。
- 自动网络重试保持原尝试，手动重试创建新尝试；启动 reconciliation 不自动恢复网络下载。
- 第八片进度协议不在本片范围；本片只保留有界、非阻塞的现有通知交接。

## 计划核对方式

下列 48 项保留原计划范围。已勾选项表示此前已取得相应阶段证据，不代替最终候选的完整回归；
本次按原逐项证明及后续统一候选核验后更新勾选；不能根据实现已接线直接登记完成。13 项最终验收以规格 §9 为准。

## 阶段 0：冻结基线与证明可行性

* [x] 读取最近 AGENTS、架构、Rust／错误／安全／观测规范，冻结开工 main 和受支持历史旧端的精确 SHA。
* [x] 盘点 BlobTransferPort 全部消费者、SDK 任务来源、provider handler、publish／tag／GC、实际取消与 shutdown 路径。
* [x] 冻结 blob ALPN、ticket 编解码、原始字节及合法空负载向量；保存旧版真实资料副本与旧旧内容基线。
* [x] 用固定 SDK 公开 get::fsm／ImportBaoHandle、provider::StreamPair／export 字节流承载真实 Uni 流，证明真实内容读回；高层 API 缺口与替代责任见 spec §5.4。
* [x] 两个 blob 流共用一条真实连接，暂停 A 后只取消 A；B 不断线、不重试且读回正确。
* [x] 观察 SDK 内部工作与导出：独立暂停 import／export worker，证明替代原型取消时保持等待，门释放后结算；Uni 工作租约另验证网络释放后不提前归还额度。
* [x] 证明失败独立文件可从既有接收材料重建准确来源与原始身份，正常单项重试不新增 schema／持久字段。
* [x] 核对旧端与当前成员身份映射、撤销及旧 ticket，证明不修改 blob 协议仍可安全互通。
* [x] 记录同内容 SDK 共享行为及等待者所有权，确认是否需要显式共享状态；不为优化主动新增去重。
* [x] 记录原大文件峰值内存、FD、连接、任务、tag 和 staging 基线，测试记录不含敏感内容或原始路径。

## 阶段 1：规则、调度与专属能力

* [x] 定义单项、尝试、批次结果及稳定错误，复用现有 attempt、提交 CAS 与产物日志，不建立第二状态权威。
* [x] 实现 spec §5.3 的容量、期限、退避和累计恢复计时；常量集中，只有一处事实来源。
* [x] 公平调度按对端轮转，同端保序；入队与名额原子，取消等待者不启动后续工作。
* [x] 明确额外共享等待者的有界计数，实际工作结束前保留活跃许可；恢复等待不另建隐形队列。
* [x] 按实际 SDK 接缝补充最小通用流停止能力与 Application 专属交换 port；不暴露 Iroh 类型或 downcast。
* [x] 构造独立 blob Uni policy，提供端与本地下载分别限额；复用现有身份目录和统一撤销登记。
* [x] 使用可控时钟及第二 backend 验证所有恰好到期、并发、关闭与额度边界。

## 阶段 2：真实下载、提供服务与取消结算

* [x] SDK 获取只接收 Uni 已管理流，删除 downloader 自行拨号和连接 warmup 双路径。
* [x] Router 交给完整 Application 提供端，准入后调用有界 SDK 流处理，不保留 SDK 自管连接 handler。
* [x] 保留内存 blob 的密码路径、文件流式路径、BAO 校验、tag／GC 保护和输出所有权。
* [x] 取消先封后续动作并停止本地；有界非阻塞交接通知，保持原 5Hz 上限，不引入第八片新协议。
* [x] 取消等待有上限，重复调用等待同一清理，调用方放弃等待不放弃监督；真实结算后才归还额度。
* [x] 自动重试采用具体来源白名单，保留完整 source，未知／存储／校验／授权失败不重试。
* [x] 正常完成包含所需引用、输出和提交成功；去掉只记日志却继续报告完成的相关路径。
* [x] 排队、运行、提交竞争、共享等待者、离线与慢本地工作走统一结算；不关闭对端连接取消单项。
* [x] 封口／暂停／锁定／切 Space／离组／撤销／删除／shutdown 及启动恢复接入同一 owner 并监督所有工作。

## 阶段 3：部分失败、手动重试与稳定接口

* [x] 独立文件逐项继续并保留成功项，最后一个成功不覆盖之前失败；批次汇总使用所有有效项。
* [x] 目录完整发布、回滚与产物日志合同保持，目录失败只允许重发原始完整接收单元。
* [x] 实现 spec §5.7 的 RetryEntryReceive、逐项业务查询及取消未完成结果，稳定项标识不使用名称／路径。
* [x] expected_attempt CAS 创建新尝试，重复点击合并／明确已有任务，不让旧清理和新写入重叠。
* [x] 保留未选中的已完成项，原始清单与记录身份准确；来源不可用、权限变化和旧 attempt 分别明确返回。
* [x] 普通同进程失败单项重试真实通过；重启后材料可用才重新发起，材料不可用不伪造来源。
* [x] UniFFI／N-API 与 Rust 同版本映射，宿主唯一动作明确；稳定契约与本计划记录实际行为。
* [x] 以真实 SQLite、文件输出和 Engine 入口覆盖重试、强杀、部分结果、目录回滚及错误分类。

## 阶段 4：唯一装配与删除旧实现

* [x] Engine 组装唯一 blob owner、独立 Uni manager、SDK adapter 及既有完整观测 decorator。
* [x] BlobTransferPort 取消合同和全部内部消费者同步迁移；不保留无意义旧包装或生产 feature 旁路。
* [x] 删除对端级 shutdown_endpoint 取消、SDK 自拨池和旧 provider 业务路径，保留底层 store 能力。
* [x] mod.rs 保持模块入口；所有权和恢复规则不散回 materializer、Engine 或绑定。
* [x] 架构检查限制业务代码直接用原生 Endpoint／Connection，实际 bridge／SDK 实现保留明确边界。
* [x] 按 spec §5.8 验证业务触发、结果、部分失败与隐私，正常清理不冒充业务入口。

## 阶段 5：真实矩阵、回归与交付

* [x] 新新及两个冻结旧端，新版获取／提供双角色 × Direct／强制 Relay，真实资料和精确内容读回通过。
* [x] 两流共用连接的独立取消、离线通知阻塞、共享内容等待者、永久错误和网络恢复在真实 Engine 路径通过。
* [x] 至少 1GiB 流式内存、FD／任务／连接／额度与 tag／staging 恢复有前后证据，连续运行无持续增长。
* [x] 默认 CI 中包含本片必需网络与资源验收；扩展既有 tests/hosts/connectivity 和 testing harness，不另建替代框架。
* [x] 前六片、成员全集、升级 smoke、相关 blob／目录／存储测试和完整 workspace 回归通过。
* [x] Linux／macOS／Windows、iOS／Android／HarmonyOS 必需编译门禁通过；物理设备未跑明确跳过。
* [x] 本地根门禁通过，再核对交付 head 的线上必需检查；保存首轮失败与修复后结果，不删除失败现场。
* [x] 稳定设计回写所属文档、更新入口；实际完成后才归档本 spec 与 plan，不关闭整个 Issue #34。

## 验证入口

从仓库根目录串行运行，复用原共享 `target` 与编译缓存。根门禁：

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

行为与回归分组：

```bash
just mbx-exec bash scripts/testing/run-test-group.sh workspace --no-fail-fast
just mbx-exec bash scripts/testing/run-test-group.sh membership-e2e --no-fail-fast
just mbx-exec bash scripts/testing/run-test-group.sh upgrade-matrix --smoke --no-fail-fast
bash scripts/testing/run-blob-connectivity.sh
bash scripts/testing/run-blob-lifecycle-resources.sh
bash scripts/testing/run-peer-reachability-connectivity.sh
```

网络矩阵需要 Linux 隔离网络权限；支持用户命名空间的环境可显式设置 `UC_NETWORK_UNPRIVILEGED=1`。
脚本负责真实宿主、冻结旧端、Direct／Relay 拓扑及临时资料清理，不另建替代框架。
真实文件观测还须独立运行 `receive_retry::observation` 用例，接入正式过滤规则的 Collector 与 Jaeger，
检查部分失败、独立重试成功／取消、错误子能力、隐私及真实列表／详情。

## 原交付期间的验收边界（历史记录）

- 2026-10-24：手动重试准入前的读取错误原先保留 source，却在最后观测持有者退出时统一记成通用失败。原完整重试入口回归先红，修正为负责人描述既有固定类别、兜底完成复用该类别；四种 IO 类别、无新 attempt／capture 和原完成时点通过，相关组 13/13。未增加观测字段、阶段查询或持久格式；最终真实 Collector／Jaeger 和完整回归仍须当前候选验收。

- 2026-10-24：资源采样新增受控边界回归，原逻辑在最终索引尚未返回时触发原任务增长断言；修正后等待同轮完整入站结果与发送确认再取轮间样本。受控回归先红后绿，原两轮 1GiB 精确内容、内存、句柄及关闭回收通过；原阈值与负载不变，最终 workspace／CI 仍须复验。
- 新增真实 Engine 同资料关闭重开验收：复用原安全存储，原来源缺失时两次重试明确拒绝，原 attempt、逐项结果、成功文件及导出次数保持；同进程正常重试和普通重投同时通过。该用例不代替既有进程强杀验收。
- 新 RustSec 公告 `RUSTSEC-2026-0387` 导致完整依赖审计失败。锁文件中的 `crypto-bigint 0.7.5` 更新至修复版 `0.7.6`，连带 `ctutils 0.4.5`；固定 OPAQUE／elliptic-curve 0.13 仍引入 `crypto-bigint 0.5.5`，审计仍失败，不新增 ignore。旧 API 分支须单独解决，不能把本次局部升级记作完整安全门禁通过。

- 首次接收创建端口提交后、返回前，查询也必须计入完整未启动计划。确定性回归先得到 0／2 项失败；原动作改为持久 attempt 可见前准备不可变计划，接受成功后才替换完整来源，原有交付证明保持。默认特性 Application 相关组 150/150 通过，完整 workspace 装配仍待复验。查询沿用既有入口，不新增持久配方或格式；Engine 流夹具按文件种类选择意图，仍独立断言三项和原 30 秒暂停期限。

- 2026-10-24：补齐 Core／Application 的原生 Iroh 生产依赖与源码门禁，真实包名检查覆盖重命名依赖；完整检查先由新增反例触发失败，再通过七个新增反例与测试依赖／Infra bridge 正例。
- 真实 SQLite 回归证实接收 pending 登记的延迟事务会在无关并发提交后返回 busy；改为状态读取前取得写资格，新增与已有 pending 两种登记及密文结果验证通过，相关仓储组 10/10。此项不作为原 Engine 间歇失败已定位或已修复的结论，仍须完整流程复验。

- SDK 正式依赖固定为同一 fork 的 `152c42a`；停止、协议、旧新资料与资源须以该修订验证。
- 前一候选的完整 Engine job、资源验收、升级回归及平台编译已有通过结果，仍不能替代交付候选全部门禁。
- 网络矩阵、升级 smoke 与必需平台编译已有通过证据；当前 workspace 与资源回归仍有失败，不能登记整体验收完成。
- 正在复验 fixture 资源所有权、锁定测试的实际封口边界、配置导入测试的完整流程预算与故障控制连接等待；保留原资源上限、业务期限及发布断言。
- CI 来源探针只取快照，不在关闭前增加等待；资源步骤在 workspace 失败后仍执行，保留诊断证据且不改变整体失败结果。完整回归与真实观测验收仍须闭合。
- 原始日志、计数、截图和逐轮状态保留本地或 CI Artifacts；当前线上结果从 [PR #230 Checks](https://github.com/UniClipboard/Engine/pull/230/checks) 核对。
- 物理设备未执行，记为跳过；必需的平台编译与线上 CI 不能据此跳过。
- 所有验收完成并回写稳定结论后才归档本计划与规格，不关闭整个 Issue #34。


## 最终统一候选与归档结算（2026-10-29）

本片原交付 #230 已合并，但 `630accd2` 当时仅完成 46/48 项计划、12/13 项 AC；
安全依赖审计失败，P47／AC13 与 P48 归档未结算。以上失败保留，不能因已合并而登记完成。

后续第八片在 `1d1fdb5977ff6350200e444e68c9e667ef1eea89`（#340）形成统一验收候选。
本次重新核对 [主 CI 40493253916](https://github.com/UniClipboard/Engine/actions/runs/40493253916) 的 13 个 job
全部成功，Windows 同 head 结果及原工件证据见[统一候选结论](2026-10-24-transfer-progress-connectivity-feasibility.md#最终候选ci与交付结论)。
该候选实际执行 OPAQUE 固定旧端兼容和依赖审计，无新增漏洞 ignore；原安全阻塞已由
[OPAQUE 依赖兼容修复](../../design-docs/opaque-dependency-compatibility.md)解除。

原网络 ZIP `12731075500`、Engine ZIP `12733205834` 和 presence ZIP `12731195925` 本次重新计算摘要，
均与上述统一结论匹配。逐份 XML 核对原第七片测试映射的 630 条引用：629 条在统一候选 workspace／资源报告中通过；
旧 `attempt_cancel_settles_slow_local_work_with_a_permanently_blocked_reporter` 随第八片删除旧 reporter 后，
由 `attempt_cancel_settles_slow_local_work_even_when_the_caller_is_abandoned` 与真实 Engine
`real_engine_offline_cancellation_ignores_permanently_blocked_progress` 分别覆盖本地工作监督与永久阻塞通知。
两者在统一候选报告实际通过，不把名称变化当作静默丢失测试。其余源码／断言与真实边界沿原逐项证明保留。

| AC | 对应原计划与实际证明 |
| --- | --- |
| 01 | 阶段0原SDK／Uni双向流、import／export实际结束及原始项重试材料；原可行性证明和真实旧新矩阵 |
| 02 | 共用连接双流与共享内容两种真实Engine用例：取消A，B原attempt、不额外获取，最终内容精确读回 |
| 03 | 来源Engine关闭、反向通知永久pending时本地取消仍结算；真实文件清理与旧成功项保护 |
| 04 | 五秒调用预算、Incomplete、重复／放弃等待与实际本地结束前不释放许可；慢本地工作用例按上述新旧映射核对 |
| 05 | 共享等待者、提交CAS、生命周期世代、真实强杀和重开staging；不自动恢复丢失来源 |
| 06 | 第二backend精确容量、时钟、退避、次数和公平调度；本地工作持有原额度，真实资源验证另行覆盖 |
| 07 | source白名单仅重试暂态网络；真实Engine读到64KiB后暂态失败，经新流完成原attempt且不增加连接 |
| 08 | 首／中／末独立文件失败、部分成果、expected_attempt CAS、成功项不重传；目录完整发布／回滚 |
| 09 | BAO／适用AEAD／引用与输出及持久提交均在成功之前；网络完成不冒充业务保存 |
| 10 | 唯一Application owner与独立Uni获取／提供；架构负例拒绝旧Downloader自拨、对端级取消及旧handler |
| 11 | 固定协议／ticket／字节、旧新双角色Direct／Relay及两轮各1GiB；原资源阈值和tag／staging恢复不变 |
| 12 | Rust／UniFFI／N-API原生映射与Harmony声明；真实Engine／Collector／Jaeger、固定错误分类与隐私核对 |
| 13 | 后续同head统一CI补齐原安全阻塞；workspace4931、资源9、testkit18、成员smoke16及完整网络原矩阵成功 |

AC12 的第七片真实观测原结果为90 spans／100 logs，部分接收与独立重试分别验收；第八片后续观测的
远端日志丢弃边界另行保留，不能将第七片的结果外推为当前远程日志永不丢失。
原本机成员全集56项和首轮失败分别保留；CI smoke16项不替代全集。统一候选还包含后续受管进度，
本次不声称仅重跑了原 #230 的相同代码。

稳定行为已在[连接与blob责任](../../design-docs/clipboard-connectivity.md)及
[Engine接收重试契约](../../design-docs/uc-engine-interface.md)维护。48项计划与13项AC按原范围完成结算，
spec／plan／方案同时归档。原失败、大目录重复激活、完整升级非smoke64维度、物理设备与产品宿主采用继续明确保留。
第九片 #345 后续CI失败独立记录，不被本次归档覆盖；不关闭整个#34，不代表发布。
