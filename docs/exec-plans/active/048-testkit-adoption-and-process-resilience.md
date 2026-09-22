# 048 testkit 采用指南与进程韧性

## 状态

- 状态：实施中
- 日期：2026-09-22
- 完整负责人：`uc-testkit::Scenario` 负责单场景证据，cargo-nextest 负责测试进程调度与总超时
- 唯一调用：测试按层级选择普通 `cargo test` 或统一分组；需要场景证据时创建一个 Scenario，并通过其资源与进程能力完成一次测试动作
- 成功结果：并行场景互不覆盖，子进程正常、异常退出或超时后均被回收并留下准确工件；新增测试指南中的命令可直接运行
- 失败结果：产品、环境、driver、framework 与 cleanup 失败保持稳定分类，原始失败不被清理失败覆盖
- 重试责任：testkit 不重试；nextest 默认不重试，长期环境型任务只有明确策略时才记录一次 retry
- 恢复责任：测试进程由 nextest 终止；testkit 启动的子进程由 testkit 在预算内 kill 并 wait，产品恢复仍由原业务负责人处理
- 长期路线：[Engine 测试架构](../../design-docs/testing-architecture.md) 是唯一长期路线图；本计划只交付首次采用与进程韧性切片，不替代 034 或真实环境 nightly

## 1. 问题与目标

前三阶段已证明 testkit 能生成证据，但首次使用者缺少从测试层级选择到 CI 接入的完整指南，现有采用也暴露两个真实缺口：

1. 同一工件根下并行运行相同场景时，稳定 `artifact_id` 对应同一目录，第二个场景以 `framework_artifact` 冲突失败。现有 provider/process 场景自行拼 PID，说明隔离责任泄漏给调用方。
2. 独立进程测试自行 spawn/wait，异常退出可以断言，但超时回收、cleanup 分类和统一报告需要每个调用方重复实现。

本计划交付面向贡献者的使用指南、现有测试采用清单，以及由上述案例驱动的最小能力补齐。简单、快速、无资源生命周期的单元测试继续直接使用 `cargo test`，不强制套 Scenario。

## 2. 非目标

- 不批量迁移全仓测试，不删除旧测试、脚本或 CI 门禁。
- 不读取、修改、迁移或复制 t-0010 工作区及测试。
- 不改变产品行为、生产公开接口、协议、持久格式或业务状态机。
- 不增加虚拟多节点网络、全局 panic hook、万能 DSL 或跨平台设备调度器。
- 不把隔离 Linux 网络检查写成真实外网或设备通过。
- 不将同日重复运行计入 10 个工作日趋势。

## 3. 已确认失败方式

| 失败方式 | 当前证据 | 预期处理 |
| --- | --- | --- |
| 同身份并行写同一根目录 | 两个 success example 并行运行，一个以 `artifact-directory-create` 失败 | kit 分配唯一、安全的工件实例目录，稳定身份仍保留 |
| 子进程 spawn 失败 | 现有 process case 直接 `Command::output` | 分类为 `environment_unavailable`，资源记录可读 |
| 子进程异常退出 | profile upgrade crash child 以 73 退出 | 返回退出状态给场景解释，testkit 只证明已回收 |
| 子进程超过等待预算 | 当前依赖 nextest 杀死整个测试，Scenario JSON 可能缺失 | testkit 在场景内 kill 并 wait，分类为 `driver_protocol` 并记录 cleanup |
| kill 或 wait 失败 | 当前没有统一记录 | cleanup 标记 failed，并作为 primary 或 secondary 保留 |
| 已有产品失败同时 cleanup 失败 | 现有 `finish` 已支持 secondary | 保持原始失败为 primary，cleanup_failed 为 secondary |
| 测试本身 panic/abort | nextest/JUnit 可见，Scenario 可能来不及 finish | 本轮明确由 runner 诊断，不引入影响全仓的 panic hook |

## 4. 模块与文件职责

- `tests/uc-testkit/src/report.rs`：稳定场景身份与唯一工件实例目录，不覆盖并行运行。
- `tests/uc-testkit/src/process.rs`：只负责测试子进程 spawn、预算等待、超时终止、回收和资源状态；不解释业务退出码。
- `tests/uc-testkit/src/scenario.rs`：组合预算、进程资源和最终报告，继续作为单场景唯一负责人。
- `tests/uc-testkit/tests/`：先覆盖并行冲突、异常退出、超时回收和失败分类，再修改实现。
- `crates/uc-infra/tests/profile_storage_upgrade_crash.rs`：代表性真实进程采用，保留原业务断言并使用 kit 回收能力。
- `docs/design-docs/testing-guide.md`：首次使用者指南和可运行最小示例。
- `docs/references/test-adoption-inventory.md`：现有测试采用清单、保留/优先/真实网络设备边界。
- `scripts/testing/run-test-group.sh`：保持唯一分组入口，补充只由真实使用需要证明的参数或输出。

## 5. 设计

### 并行工件隔离

`ScenarioIdentity::artifact_id` 继续表示稳定的“场景名 + seed”。ArtifactWriter 在其下创建唯一实例目录，目录名只含进程 ID 与进程内原子序号。`ScenarioReport` 升级 schema，新增安全的 `artifact_directory` 叶子名；报告不包含绝对路径。旧字段保留，现有消费者可继续按 `scenario`、`artifact_id` 和 `outcome` 读取。

### 有界子进程

使用仓库已有 Tokio，不增加新的 runner 或进程库。testkit 接收 `tokio::process::Command`：

1. 注册 pending 子进程资源并 spawn。
2. 等待显式进程预算与 Scenario 剩余预算中的较小值。
3. 正常或异常退出都 wait 完成并将 cleanup 标为 completed，退出码由调用方判断。
4. 超时则 start_kill 并 wait；成功回收后返回 `driver_protocol/child-process-timeout`。
5. kill/wait 失败将资源标为 failed，使 `finish` 保留 cleanup 证据。

### 统一入口

现有 `fast`、`evidence`、`persistence-provider`、`engine-smoke`、`process`、`real-network`、`device` 名称不变。指南说明简单单元测试直接运行；只有跨阶段、异步等待、资源、独立进程或结构化失败证据场景采用 testkit。

## 6. 实施顺序

1. 修正 047 已完成项与长期观察边界；新增本计划。
2. 先增加并行隔离、异常退出、超时回收的失败测试，证明当前缺口。
3. 实现唯一工件实例目录和最小进程运行能力。
4. 将 profile storage upgrade 代表场景改用该能力，不改变原退出码与持久恢复断言。
5. 编写测试指南和采用清单，逐条执行指南命令。
6. 运行 testkit、代表 process/provider/Application evidence、旧 cargo test 入口和仓库静态检查。
7. 更新稳定设计、架构圣经、计划进度和 Herdr 报告；branch guard 后推送同一 draft PR。
8. 将用户确认的 Engine-only 双运行线、首批五类、速度与报告标准回写长期路线；不在本计划提前实现完整多节点或 nightly 平台。

## 7. 验收标准

- [x] 两个相同身份场景在同一工件根并行完成，JSON 不覆盖且各自目录可定位。
- [x] 子进程 wait 的非零退出和超时路径已验证；超时子进程被终止并再次 wait，cleanup 结果准确。
- [x] 原始失败与 cleanup failure 的 primary/secondary 关系保持。
- [x] profile storage upgrade 独立进程代表场景采用新能力，原持久恢复断言不变。
- [x] 新指南说明层级选择、支撑复用、分组/CI、资源/预算、失败查看与复现，并明确普通单元测试无需 testkit。
- [x] 指南中的全部本地命令实际运行；最小示例生成 JSON、摘要和 JUnit。
- [x] 采用清单区分保留原样、优先改造、真实网络/设备保留，没有批量迁移。
- [x] 047 已完成事实与长期趋势待办准确分开。
- [x] 旧 cargo test、旧测试和原 CI 门禁保持；产品公开接口和行为不变。
- [ ] PR #113 保持 draft；推送后非发布检查终态与远程工件仍待核验。
- [x] 长期路线明确 kit 首版与全仓迁移的区别，并与 034、044-047 建立唯一关系。

## 8. 回退

- 工件实例目录可独立回退到调用方隔离根，但会重新暴露已复现的并行冲突。
- 进程能力和 profile crash 采用可独立回退，旧 `Command::output` 测试仍保留业务基线。
- 指南与采用清单不改变运行行为，可独立修订。
- CI 继续使用现有 evidence 入口；本计划不删除旧门禁，因此回退不影响主线测试。

## 9. 进度

| 日期 | 结果 |
| --- | --- |
| 2026-09-22 | 计划完成；并行复现确认相同场景同根运行一个成功、一个 `framework_artifact` 失败。 |
| 2026-09-22 | testkit 定向测试 7 项通过、2 个子进程入口忽略；相同身份并行目录、非零退出回收和 30ms 超时终止均有工件断言。 |
| 2026-09-22 | profile storage upgrade 五个崩溃边界采用有界进程入口，测试 1 项通过、测试耗时 2.24 秒，原持久恢复断言保持。 |
| 2026-09-22 | 新增首次使用指南与采用清单；核查发现 `persistence-provider` 统一入口遗漏已采用 provider evidence，已同步修正，待逐条运行命令和完整仓库检查。 |
| 2026-09-22 | 首次 `evidence` 14/14 通过、测试耗时 1.977 秒，但 workspace 发现造成 198.84 秒墙钟；已给 evidence/provider/process 增加明确 package 边界，待复跑确认。 |
| 2026-09-22 | schema v2 工件已核对成功、driver、产品、环境和 cleanup 分类；三个采用调用方移除手工 `process-<pid>` 根目录，隔离责任收回 testkit。 |
| 2026-09-22 | 指南命令实跑：fast 7/7（0.148 秒测试，1.95 秒墙钟）、evidence 14/14（warm 2.039 秒测试，5.34 秒墙钟）、persistence-provider 49/49（4.313 秒测试，5.58 秒墙钟）、engine-smoke 48/48、process 23/23（5 项 slow）。 |
| 2026-09-22 | metadata 0.55 秒、workspace all-target check 50.35 秒；fmt、Rust style、Engine repository、observability privacy、diff check 通过。远程 draft PR 检查待推送后验证。 |
| 2026-09-22 | 首个实施切片以 `ebb459ff` 推送：首次使用指南、采用清单、并行工件隔离、有界子进程及分组入口完成；PR #113 保持 draft。长期路线随后收敛到 `testing-architecture.md`，不新增重复计划。 |
