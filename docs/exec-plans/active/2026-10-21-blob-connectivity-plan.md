# Issue #34 第七切片：文件／blob 受管传输执行计划

状态：**功能已接线，完整回归与交付验收未闭合**（2026-10-23）。
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
未勾选项仍须按规格核验，不能根据实现已接线直接登记完成。13 项最终验收以规格 §9 为准。

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

* [ ] 定义单项、尝试、批次结果及稳定错误，复用现有 attempt、提交 CAS 与产物日志，不建立第二状态权威。
* [ ] 实现 spec §5.3 的容量、期限、退避和累计恢复计时；常量集中，只有一处事实来源。
* [x] 公平调度按对端轮转，同端保序；入队与名额原子，取消等待者不启动后续工作。
* [ ] 明确额外共享等待者的有界计数，实际工作结束前保留活跃许可；恢复等待不另建隐形队列。
* [x] 按实际 SDK 接缝补充最小通用流停止能力与 Application 专属交换 port；不暴露 Iroh 类型或 downcast。
* [x] 构造独立 blob Uni policy，提供端与本地下载分别限额；复用现有身份目录和统一撤销登记。
* [ ] 使用可控时钟及第二 backend 验证所有恰好到期、并发、关闭与额度边界。

## 阶段 2：真实下载、提供服务与取消结算

* [x] SDK 获取只接收 Uni 已管理流，删除 downloader 自行拨号和连接 warmup 双路径。
* [x] Router 交给完整 Application 提供端，准入后调用有界 SDK 流处理，不保留 SDK 自管连接 handler。
* [ ] 保留内存 blob 的密码路径、文件流式路径、BAO 校验、tag／GC 保护和输出所有权。
* [ ] 取消先封后续动作并停止本地；有界非阻塞交接通知，保持原 5Hz 上限，不引入第八片新协议。
* [ ] 取消等待有上限，重复调用等待同一清理，调用方放弃等待不放弃监督；真实结算后才归还额度。
* [ ] 自动重试采用具体来源白名单，保留完整 source，未知／存储／校验／授权失败不重试。
* [ ] 正常完成包含所需引用、输出和提交成功；去掉只记日志却继续报告完成的相关路径。
* [ ] 排队、运行、提交竞争、共享等待者、离线与慢本地工作走统一结算；不关闭对端连接取消单项。
* [ ] 封口／暂停／锁定／切 Space／离组／撤销／删除／shutdown 及启动恢复接入同一 owner 并监督所有工作。

## 阶段 3：部分失败、手动重试与稳定接口

* [x] 独立文件逐项继续并保留成功项，最后一个成功不覆盖之前失败；批次汇总使用所有有效项。
* [ ] 目录完整发布、回滚与产物日志合同保持，目录失败只允许重发原始完整接收单元。
* [x] 实现 spec §5.7 的 RetryEntryReceive、逐项业务查询及取消未完成结果，稳定项标识不使用名称／路径。
* [x] expected_attempt CAS 创建新尝试，重复点击合并／明确已有任务，不让旧清理和新写入重叠。
* [ ] 保留未选中的已完成项，原始清单与记录身份准确；来源不可用、权限变化和旧 attempt 分别明确返回。
* [ ] 普通同进程失败单项重试真实通过；重启后材料可用才重新发起，材料不可用不伪造来源。
* [x] UniFFI／N-API 与 Rust 同版本映射，宿主唯一动作明确；稳定契约与本计划记录实际行为。
* [ ] 以真实 SQLite、文件输出和 Engine 入口覆盖重试、强杀、部分结果、目录回滚及错误分类。

## 阶段 4：唯一装配与删除旧实现

* [ ] Engine 组装唯一 blob owner、独立 Uni manager、SDK adapter 及既有完整观测 decorator。
* [x] BlobTransferPort 取消合同和全部内部消费者同步迁移；不保留无意义旧包装或生产 feature 旁路。
* [x] 删除对端级 shutdown_endpoint 取消、SDK 自拨池和旧 provider 业务路径，保留底层 store 能力。
* [ ] mod.rs 保持模块入口；所有权和恢复规则不散回 materializer、Engine 或绑定。
* [x] 架构检查限制业务代码直接用原生 Endpoint／Connection，实际 bridge／SDK 实现保留明确边界。
* [ ] 按 spec §5.8 验证业务触发、结果、部分失败与隐私，正常清理不冒充业务入口。

## 阶段 5：真实矩阵、回归与交付

* [x] 新新及两个冻结旧端，新版获取／提供双角色 × Direct／强制 Relay，真实资料和精确内容读回通过。
* [ ] 两流共用连接的独立取消、离线通知阻塞、共享内容等待者、永久错误和网络恢复在真实 Engine 路径通过。
* [ ] 至少 1GiB 流式内存、FD／任务／连接／额度与 tag／staging 恢复有前后证据，连续运行无持续增长。
* [ ] 默认 CI 中包含本片必需网络与资源验收；扩展既有 tests/hosts/connectivity 和 testing harness，不另建替代框架。
* [ ] 前六片、成员全集、升级 smoke、相关 blob／目录／存储测试和完整 workspace 回归通过。
* [ ] Linux／macOS／Windows、iOS／Android／HarmonyOS 必需编译门禁通过；物理设备未跑明确跳过。
* [ ] 本地根门禁通过，再核对交付 head 的线上必需检查；保存首轮失败与修复后结果，不删除失败现场。
* [ ] 稳定设计回写所属文档、更新入口；实际完成后才归档本 spec 与 plan，不关闭整个 Issue #34。

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

## 当前验收边界

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
