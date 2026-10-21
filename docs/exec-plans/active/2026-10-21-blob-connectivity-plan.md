# Issue #34 第七切片：文件／blob 受管传输执行计划

状态：**阶段 0 设计阻塞，生产迁移未开始**（2026-10-21）。固定 SDK 高层获取的失败路径缺少单项 store 完成等待权；[复现与方案](2026-10-21-blob-connectivity-feasibility.md)。
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
* [ ] 盘点 BlobTransferPort 全部消费者、SDK 任务来源、provider handler、publish／tag／GC、实际取消与 shutdown 路径。
* [ ] 冻结 blob ALPN、ticket 编解码、原始字节及合法空负载向量；保存旧版真实资料副本与旧旧内容基线。
* [ ] 用固定 SDK 的 GetStreamPair 和 provider::handle_stream 承载 Uni 通用流，证明双向真实内容读回。
* [ ] 两个 blob 流共用一条真实连接，暂停 A 后只取消 A；B 不断线、不重试且读回正确。
* [ ] 观察 SDK 内部工作与导出：证明取消可以 stop／reset 并等待实际任务退出，不能只看进度接收器结束。
* [ ] 证明失败独立文件可从既有接收材料重建准确来源与原始身份，正常单项重试不新增 schema／持久字段。
* [ ] 核对旧端与当前成员身份映射、撤销及旧 ticket，证明不修改 blob 协议仍可安全互通。
* [ ] 记录同内容 SDK 共享行为及等待者所有权，确认是否需要显式共享状态；不为优化主动新增去重。
* [ ] 记录原大文件峰值内存、FD、连接、任务、tag 和 staging 基线，测试记录不含敏感内容或原始路径。

**退出条件**：spec §11 三项技术证明均有可重复命令与结果，正常与失败路径都可观察。
任一失败时明确标为设计阻塞，保留 source、最小复现和替代方案；不得先上线旁路、关闭共享连接或修改格式绕过。
验证原型若被采用，应演进成唯一实现；未采用的原型在本片内删除。

## 阶段 1：规则、调度与专属能力

修改位置：`uc-core` 的纯规则、`uc-application/src/transfer/receive/`、`transfer/blob/transport/` 和 `connectivity/`。

* [ ] 定义单项、尝试、批次结果及稳定错误，复用现有 attempt、提交 CAS 与产物日志，不建立第二状态权威。
* [ ] 实现 spec §5.3 的容量、期限、退避和累计恢复计时；常量集中，只有一处事实来源。
* [ ] 公平调度按对端轮转，同端保序；入队与名额原子，取消等待者不启动后续工作。
* [ ] 明确额外共享等待者的有界计数，实际工作结束前保留活跃许可；恢复等待不另建隐形队列。
* [ ] 按实际 SDK 接缝补充最小通用流停止能力与 Application 专属交换 port；不暴露 Iroh 类型或 downcast。
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
