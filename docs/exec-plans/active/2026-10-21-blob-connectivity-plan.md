# Issue #34 第七切片：文件／blob 受管传输执行计划

状态：**阶段 0 前置证明完成，进入阶段 1**（2026-10-21）。高层 SDK 停止缺口由同一 SDK 公开底层 API 持有完成权解决；材料、旧资料、授权、协议及资源证据见[复现与方案](2026-10-21-blob-connectivity-feasibility.md)。
唯一设计来源：[实现规格](2026-10-21-blob-connectivity-spec.md)。
本计划不复制参数、接口和异常规则；均以 spec 相应章节为准。

## 完整负责人及交付边界

- Application `transfer` 对接收、重试、调度、取消和结算负责；内部接收 owner、blob transport 与文件会话按职责拆分。
- 调用方只提交完整接收／重试／取消动作，不编排拨号、逐文件失败处理或回滚。
- 成功、部分失败、取消及尚未完成严格按 spec §5 结算；本地成功不等待进度通知送达。
- 自动网络重试由该 owner 负责，用户手动重试创建新尝试；启动恢复由现有接收 reconciliation 负责，不自动恢复网络下载。
- Infra 提供 SDK／store／通用流能力；Engine 仅装配及稳定映射，绑定只调用同版本 uc-engine。
- 第八片进度协议与发送侧投影不在本片实施，已有非阻塞通知交接是本片必要依赖。

## 阶段 0：冻结基线与证明可行性

允许增加验证原型和测试，不先替换生产路径。所有 Rust 规划与实现须使用仓库 Rust 开发规范及对应 skill。

* [x] 读取最近 AGENTS、架构、Rust／错误／安全／观测规范，冻结开工 main 和受支持历史旧端的精确 SHA。
* [x] 盘点 BlobTransferPort 全部消费者、SDK 任务来源、provider handler、publish／tag／GC、实际取消与 shutdown 路径。
* [x] 冻结 blob ALPN、ticket 编解码、原始字节及合法空负载向量；保存旧版真实资料副本与旧旧内容基线。
* [x] 用固定 SDK 公开 get::fsm／ImportBaoHandle、provider::StreamPair／export 字节流承载真实 Uni 流，证明真实内容读回；高层 API 缺口与替代责任见 spec §5.4。
* [x] 两个 blob 流共用一条真实连接，暂停 A 后只取消 A；B 不断线、不重试且读回正确。本机 Uni 原型通过；完整 Engine 硬验收仍在阶段 5。
* [x] 观察 SDK 内部工作与导出：独立暂停 import／export worker，证明替代原型取消时保持等待，门释放后结算；Uni 工作租约另验证网络释放后不提前归还额度。
* [x] 证明失败独立文件可从既有接收材料重建准确来源与原始身份，正常单项重试不新增 schema／持久字段。
* [x] 核对旧端与当前成员身份映射、撤销及旧 ticket，证明不修改 blob 协议仍可安全互通。
* [x] 记录同内容 SDK 共享行为及等待者所有权，确认是否需要显式共享状态；不为优化主动新增去重。
* [x] 记录原大文件峰值内存、FD、连接、任务、tag 和 staging 基线，测试记录不含敏感内容或原始路径。

**退出条件**：spec §11 三项技术证明均有可重复命令与结果，正常与失败路径都可观察。
任一失败时明确标为设计阻塞，保留 source、最小复现和替代方案；不得先上线旁路、关闭共享连接或修改格式绕过。
验证原型若被采用，应演进成唯一实现；未采用的原型在本片内删除。

## 阶段 1：规则、调度与专属能力

修改位置：`uc-core` 的纯规则、`uc-application/src/transfer/receive/`、`transfer/blob/transport/` 和 `connectivity/`。

* [ ] 定义单项、尝试、批次结果及稳定错误，复用现有 attempt、提交 CAS 与产物日志，不建立第二状态权威。
* [ ] 实现 spec §5.3 的容量、期限、退避和累计恢复计时；常量集中，只有一处事实来源。
* [x] 公平调度按对端轮转，同端保序；入队与名额原子，取消等待者不启动后续工作。规则模块已验证，正式网络接线仍待后续阶段。
* [ ] 明确额外共享等待者的有界计数，实际工作结束前保留活跃许可；恢复等待不另建隐形队列。
* [x] 按实际 SDK 接缝补充最小通用流停止能力与 Application 专属交换 port；不暴露 Iroh 类型或 downcast。唯一 SDK 实现已由真实 Uni／SDK 探针使用，业务组装仍待后续阶段。
* [ ] 构造独立 blob Uni policy，提供端与本地下载分别限额；复用现有身份目录和统一撤销登记。
* [ ] 使用可控时钟及第二 backend 验证所有恰好到期、并发、关闭与额度边界。

风险：新增调度可能覆盖旧扫描计时、共享等待者可能绕过容量；以同一 owner 和原子计数收口。
**退出条件**：调度不依赖原生库，精确边界和公平性测试通过，已有 Uni 消费者无行为回归。

## 阶段 2：真实下载、提供服务与取消结算

修改位置：Application blob／file／receive、Infra iroh blobs／connectivity bridge／node。

* [ ] SDK 获取只接收 Uni 已管理流，删除 downloader 自行拨号和连接 warmup 双路径。
* [ ] Router 交给完整 Application 提供端，准入后调用有界 SDK 流处理，不保留 SDK 自管连接 handler。
* [ ] 保留内存 blob 的密码路径、文件流式路径、BAO 校验、tag／GC 保护和输出所有权。
* [ ] 取消先封后续动作并停止本地；有界非阻塞交接通知，保持原 5Hz 上限，不引入第八片新协议。
* [ ] 取消等待有上限，重复调用等待同一清理，调用方放弃等待不放弃监督；真实结算后才归还额度。
* [ ] 自动重试采用具体来源白名单，保留完整 source，未知／存储／校验／授权失败不重试。
* [ ] 正常完成包含所需引用、输出和提交成功；去掉只记日志却继续报告完成的相关路径。
* [ ] 排队、运行、提交竞争、共享等待者、离线与慢本地工作走统一结算；不关闭对端连接取消单项。
* [ ] 封口／暂停／锁定／切 Space／离组／撤销／删除／shutdown 及启动恢复接入同一 owner 并监督所有工作。

风险：SDK 脱离 future 的任务、不可中断文件工作和提交竞争可能留下迟到输出。
**退出条件**：spec 硬验收及真实资源回收通过；没有仅 facade 返回取消、底层仍继续运行的假成功。

## 阶段 3：部分失败、手动重试与稳定接口

修改位置：receive owner、apply_inbound materializer／usecase、Engine contract／history operations、薄绑定。

* [ ] 独立文件逐项继续并保留成功项，最后一个成功不覆盖之前失败；批次汇总使用所有有效项。
* [ ] 目录完整发布、回滚与产物日志合同保持，目录失败只允许重发原始完整接收单元。
* [ ] 实现 spec §5.7 的 RetryEntryReceive、逐项业务查询及取消未完成结果，稳定项标识不使用名称／路径。
* [ ] expected_attempt CAS 创建新尝试，重复点击合并／明确已有任务，不让旧清理和新写入重叠。
* [ ] 保留未选中的已完成项，原始清单与记录身份准确；来源不可用、权限变化和旧 attempt 分别明确返回。
* [ ] 普通同进程失败单项重试真实通过；重启后材料可用才重新发起，材料不可用不伪造来源。
* [ ] UniFFI／N-API 与 Rust 同版本映射，宿主唯一动作明确；更新接口及相关 Product Spec 的实际行为。
* [ ] 以真实 SQLite、文件输出和 Engine 入口覆盖重试、强杀、部分结果、目录回滚及错误分类。

风险：把失败占位当原始材料、按整个条目重传成功文件、稳定结果新增后绑定映射遗漏。
**退出条件**：失败／取消项可准确手动重试，成功项不重新下载，批次和绑定行为一致。

## 阶段 4：唯一装配与删除旧实现

* [ ] Engine 组装唯一 blob owner、独立 Uni manager、SDK adapter 及既有完整观测 decorator。
* [ ] BlobTransferPort 取消合同和全部内部消费者同步迁移；不保留无意义旧包装或生产 feature 旁路。
* [ ] 删除对端级 shutdown_endpoint 取消、SDK 自拨池和旧 provider 业务路径，保留底层 store 能力。
* [ ] mod.rs 保持模块入口；所有权和恢复规则不散回 materializer、Engine 或绑定。
* [ ] 架构检查限制业务代码直接用原生 Endpoint／Connection，实际 bridge／SDK 实现保留明确边界。
* [ ] 按 spec §5.8 验证业务触发、结果、部分失败与隐私，正常清理不冒充业务入口。

**退出条件**：生产调用图全部经过受管路径，无双实现或观测专用步骤接口；真实正常流程仍互通。

## 阶段 5：真实矩阵、回归与交付

矩阵细则只在 spec §8–9 定义。以下为执行追踪，所有项当前均未执行：

* [ ] 新新及两个冻结旧端，新版获取／提供双角色 × Direct／强制 Relay，真实资料和精确内容读回通过。
* [ ] 两流共用连接的独立取消、离线通知阻塞、共享内容等待者、永久错误和网络恢复在真实 Engine 路径通过。
* [ ] 至少 1GiB 流式内存、FD／任务／连接／额度与 tag／staging 恢复有前后证据，连续运行无持续增长。
* [ ] 默认 CI 中包含本片必需网络与资源验收；扩展既有 tests/hosts/connectivity 和 testing harness，不另建替代框架。
* [ ] 前六片、成员全集、升级 smoke、相关 blob／目录／存储测试和完整 workspace 回归通过。
* [ ] Linux／macOS／Windows、iOS／Android／HarmonyOS 必需编译门禁通过；物理设备未跑明确跳过。
* [ ] 本地根门禁通过，再核对交付 head 的线上必需检查；保存首轮失败与修复后结果，不删除失败现场。
* [ ] 稳定设计回写所属文档、更新入口；实际完成后才归档本 spec 与 plan，不关闭整个 Issue #34。

## 命令与证据

所有 Cargo 从仓库根目录通过 just，共用 target 和 MBX，串行执行。先发现并运行直接受影响测试，
再执行根检查；真实网络 harness 的最终新增命令在实现阶段补齐，未落地脚本不冒充可运行入口。

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

涉及 Rust 行为时另执行相关 nextest、workspace、成员与平台门禁；只通过上述检查不能宣称行为通过。
证据记录精确 SHA、SDK 修订、驱动参数／摘要、命令、退出码、实际 ALPN／路径、资源样本与内容比较结果。
敏感真实资料不进入 Git 或公开工件；临时旧源码、资料和进程由本片执行者在结束前检查并回收。

## 当前记录

- 2026-10-21：完成设计及待实现计划。用户已确认行为、预算与 blob 兼容范围，SDK 双向接口仅完成源码盘点。
- 设计交付时阶段 0–5 未执行；本次开工进展见以下记录。未勾选项不表示通过。
- 第八片的最终结果 ACK 建议尚未确认，继续保留在讨论中，不视为本计划前置功能。

### 2026-10-21 阶段 0 停止证明

基于最新 `origin/main` 的 `a461b89444efd60081c7ef63f927946f9dfc072d` 创建 `feat/blob-connectivity`，保留原有未提交规划材料。源码盘点和最小探针见[可行性证据](2026-10-21-blob-connectivity-feasibility.md)。

固定 SDK 的 `Remote::execute_get` 在网络失败时已返回，但该次 import 仍可在独立 store 运行时中执行。公开接口未交还该次完成等待权；全 store 的 `wait_idle` 不能替代独立任务 join。此为 spec §11 第一项技术证明失败，阶段 0 退出条件未满足，阶段 1–5 未启动。

先后修正探针自身的锁 guard 编译错误和 provider store 重复 shutdown，保留首轮日志。停止门释放后实际等待 store 空闲并关闭资源；生产实现、协议与 SDK 来源未修改。原始重试材料、旧端矩阵、Uni 双向与资源基线尚未证明，不能据源码推断勾选。

本次变更根 metadata、全 workspace 全目标 check、fmt、Rust style、repository preflight、文档链接与差异检查通过。完整 workspace 分组为 **4305 passed、1 failed、27 skipped**（555.896 秒，退出码 100）；唯一失败为未改动的 `peer_address_read_diagnostics` 符号化断言（`unresolved`／`captured`），已有二进制单独运行同样失败。新增停止探针在整轮中通过。完整回归不能登记通过；具体证据与命令见可行性报告。线上 CI、设备、旧端矩阵及独立成员全集／升级矩阵未运行。

### 2026-10-21 公开底层 SDK 替代方案重验

高层 API 的失败保留，但它不证明同一 SDK 的底层组合不可行。新增接收／提供原型分别持有公开 import 完成 receiver 与 export 字节流，取消后完成本次本地结算，不调用全局 wait_idle。真实原生双流和真实 Uni 租约原型已通过；A、B 同时暂停，取消 A 后恢复 B，B 精确读回 4MiB，连接 ID 保持且 backend 获取计数为 1。原生 stop/reset 保留来源，工作租约持有连接与额度；这些能力尚未用于生产 blob 路径。

上一轮 sibling error 3 已定位为探针将 Raw blob 用 GetRequest::all 当成 HashSeq，改用 blob 后读回通过；保留首轮失败。SDK 版本、ALPN 和字节格式未修改。SDK handler 的非受管后台工作不作为可采用方案，采用 SDK 公开 parser／encoder 的完整交换原型。

原始项重试材料、当前成员与旧端身份授权、旧资料角色矩阵、资源基线尚未证明；阶段 0 整体未完成，生产迁移未开始。本次不是解除所有前置或宣布 spec 完成。

本轮相关验证：Uni 29、原生 connectivity 3、替代接收生命周期 6、Application transport 20 均通过；metadata、workspace all-targets check、fmt、Rust style、repository preflight、相对链接与 diff 检查通过。固定 SDK 接收普通／空 blob（0.40 秒）和真实 Uni 无效停止码来源／双流隔离（0.09 秒）补充探针通过。完整 workspace 本轮未重跑，首轮失败历史继续保留；下一步仍为其余阶段 0 前置证明。

### 2026-10-21 材料、准入与协议资源补充

原始 V3 材料两项探针通过：真实物化失败后从原材料重建新 attempt，恢复精确来源和文件内容，
重复名称与内容按原位置区分。当前成员／会话准入及同内容等待者的真实 Uni 分组四项通过。
这证明材料与独立完成权可用，稳定重试 API 和 SQLite/CAS 完整实现仍归阶段 3。

在未经修改的 rc.15 库及其固定 iroh-blobs 0.102.0 fork 上生成合成 Raw／合法空负载向量，
当前固定 SDK 0.103.0 对照测试通过：ALPN、ticket 编码、Get 请求和 BAO 响应字节完全相同。
生成脚本只给冻结源码添加独立 example，不改旧库或锁文件；正常退出后检查进程并回收临时源码。

1GiB 迁移前旧适配器基线通过（16.50 秒）；逐块校验、RSS 峰值约 46 MiB、tag 释放及 SDK
临时文件均有计数。旧连接总数与峰值为 2，正式迁移后须移除 warmup／Downloader 双路径。
原始计数与局限见可行性报告，不能冒充新 owner 的资源验收。

### 2026-10-21 阶段 0 退出

两个冻结锚点（开工 main a461b894、rc.15 f6f305d9）各自正常配对、旧资料重开，分别升级一侧；
Direct／强制 Relay × 两侧升级共 8 组全部通过。每组均验证旧旧文件、新旧双向文件精确内容，
原接收文件升级后可读，原设备与传输身份／成员／历史保留，独立资料副本存在，最后清理与明文探针通过。
结合真实生产 IrohConnectionAdmission 的撤权／代次失效证明和相同身份算法，阶段 0 三项技术前置均具备可重复证据。
这些是迁移前基线及组件证明，不替代阶段 5 的完整新 owner 硬验收；旧端正常交换在 60 秒界限内完成。

本轮 metadata、workspace all-targets check、fmt、Rust style 和 repository preflight 通过。
preflight 首次与构建相邻执行时触发其内部 Cargo 等待，已中止并在构建结束后串行重跑通过；保留首轮日志。
全部 workspace 未重跑；首轮未修改符号化诊断失败依旧保留。

进入阶段 1：由 transfer 模块实现公平有界下载调度、精确时间预算和专属交换能力，随后迁入唯一 Infra SDK 实现。

### 2026-10-21 阶段 1 调度与预算

新增 transfer 内部 DownloadScheduler 与 ReceiveTiming。调度原子处理 4／2 活跃和 64／16 排队，
按可启动对端轮转、同端按接受顺序；等待 future 被取消或丢弃时只回收自身预留，
许可已交接但 future 未 poll 的竞争不会泄漏额度。许可记录真实授予时刻，
恰好 60 秒的到期项不能因名额同时释放而启动，未 poll 的过期项不占住新队列。
关闭封住新意图和等待项，运行许可仍由实际工作持有。

ReceiveTiming 覆盖首次 60 秒、有效新增数据 300 秒停滞、累计恢复 30 秒与四次尝试／退避。
已验证 Raw 内容前缀的最大位置跨网络轮保留，重传与重复事件不续期；
恢复计时遇到新数据暂停但不归零，故障后的拨号不获得新首数据预算。
合法空结束不伪造数据回调，仍须在原首数据期限内完成。

定向 transfer 分组首轮 38 项通过，含新增并发 80 个入队意图只接受 64 个等待者、
取消后新任务可用的实际竞争测试。调度测试首轮 ID 构造编译错误及修复结果均保留。
风格检查发现受限可见性中的绝对路径与原因类型的纯转调分类，已用祖先相对可见性和
主错误的固定原因分类收口，不扩大模块接口。

这些规则尚未接入生产获取；原 SDK 下载器和对端级取消仍待阶段 2／4 删除。
阶段 1 的专属交换能力、完整资源策略与第二 backend 验证仍未完成，不把本小步算作整个阶段退出。

本轮最终规则检查：transfer 38 项（0.02 秒）、metadata、workspace 全目标 check、fmt、Rust style、
repository preflight 和差异检查通过。提交后包选择器门禁发现旧向量脚本仍使用已拆分包的 -p 名称，
改为直接指定冻结 crate manifest，重跑旧向量生成（5.76 秒）及门禁通过，fixture 字节未变；
首轮失败日志保留。code-review 技能缺失，按手工差异审查记录；完整 workspace 与最终平台 CI 本轮未重跑。

### 2026-10-21 专属 SDK 交换能力

新增 Application `transfer/blob/transport` 的完整交换 port 与不透明结果保护；Infra
`network/iroh/blob_exchange` 是唯一手动驱动 SDK 的实现，只接收已准入 Uni 租约，
没有 Endpoint、自拨或业务重试。原 tests/support 交换算法已删除，真实 Uni 双流、
同内容隔离及暂停 import／export 探针均调用同一实现。成功结果继续持有 TempTag 与
StreamWorkLease，后续引用／输出结算完成后才允许释放。请求头、tag 准备、数据交换和
import／export 结算都处理取消，析构兜底停止的失败也回收到同一结果，不按取消成功吞掉。

提供端不把 SDK 字节流 EOF 单独视为完成：使用 SDK 同源 BAO 遍历器核对完整响应长度，
并核对本地状态；编码继续由 SDK export 完成。真实 SDK export worker 故障注入已证明
不会报告完成。稀疏与越过末尾范围的首轮长度差异已修复为与 SDK 一致的尾部归一化，
失败日志保留。直接引用的 bao-tree 已存在于原锁文件，只新增当前 crate 的依赖边，
全部依赖版本／来源与 SDK 修订不变。

最终 nextest 定向 11／11 通过（0.527 秒，312 项按过滤跳过），包含旧协议向量、真实 Uni
不同／相同内容取消、SDK 工作结算、空文件及本地已有内容、请求头取消／到期、来源不存在、
隐式停止失败来源、网络重试分类与 SDK worker 故障。metadata、workspace 全目标 check、fmt、
Rust style、repository preflight 与差异检查通过。没有据此勾选完整 Engine 收发或阶段 5。

生产仍使用旧 BlobTransferFacade／Downloader／provider；下一步为完整 Application 监督 owner、
独立 blob Uni 的连接与服务资源策略、实际获取／提供接线及删除旧路径。稳定重试、部分失败、
绑定、生命周期和最终矩阵继续按原 spec 执行。

### 2026-10-21 外部队列前的请求围栏与实际结算

Uni 内部获取接口统一改为同步 `request` 创建不透明 `ConnectionRequest`，冻结成员撤销与会话代次，
随后才等待连接；重试克隆同一请求。旧请求即使尚未开始获取，或当前已有重新加入后的连接，也不能复活。
断开会话后重新启用 manager 不复活旧请求。内部调用方已迁移，稳定 Engine／绑定入口没有增加连接步骤。

新增 `supervise_local_work` 供非 presence 协议保留实际工作完成权。已开始的准入读取在取消／到期后
继续结算，未开始读取不补启动。共同拨号仅最后等待者退出才停止；非最后等待者立即得到自身结果，
不阻断兄弟请求。最后等待者继续持拨号额度到真实结束，期间 backend／准入异常与逻辑停止主因一起保留。

Application connectivity 定向 99 项通过（0.22 秒），包括排队前围栏、会话换代、共享等待者取消／
到期、最后等待者保留容量与原始 PermissionDenied 来源。workspace 全目标 check 通过；首次检查发现
迁移 import 置于模块文档之前的编译错误，已修正并重跑。本步尚未完成 blob 专属资源策略或生产接线。

迁移后的真实 Uni／SDK nextest 定向 5／5 通过（0.513 秒，317 项按过滤跳过），覆盖真实准入、
同／不同内容双流独立停止及 SDK 工作结算。metadata、workspace 全目标 check、fmt、Rust style、
repository preflight、文档相对链接及 diff 检查通过。完整 workspace 与正式生产路径矩阵未在本步重跑。
