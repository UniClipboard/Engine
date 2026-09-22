# 规格 034：确定性虚拟 Peer Network 测试套件

## 状态

- **状态**：实施中；最小两节点成员历史切片已完成，F0-F7 完整矩阵尚未实施
- **日期**：2026-09-03
- **前置规格**：[029 持久化成员历史反熵](../completed/029-durable-membership-history-anti-entropy.md)、[030 成员分叉选择与复杂拓扑验证](../completed/030-membership-conflict-resolution-and-chaos-validation.md)、[031 Application 依赖表面深化](../completed/031-application-dependency-surface-deepening.md)
- **完整负责人**：`uc-application` 的 test-only `VirtualMembershipTopology`
- **调用方唯一动作**：测试场景只提交拓扑动作并调用一次有界收敛驱动；不得编排单条协议消息、ACK、水位、恢复阶段或后台任务
- **成功结果**：在给定 round/frame 预算内得到满足断言的 `VirtualTopologySnapshot` 和可复现脱敏 trace
- **失败结果**：返回稳定的测试失败分类，并附最后一段脱敏 trace；不得依赖 wall-clock 超时推断原因
- **重试与重启责任**：Application 生产负责人继续拥有持久欠账和恢复；virtual topology 只驱动逻辑时间、maintenance round 与节点重建，不复制重试规则
- **长期路线关系**：本计划是 [Engine 测试架构](../../design-docs/testing-architecture.md) 中快速确定性线的多节点实现专项；首个切片从配对、传输、断线重连、重启恢复、旧资料升级五类中选择一个最慢或最不稳定代表场景，不另建并行路线图

## 当前实施切片（2026-09-22）

本轮只实现配对类别中的“已完成准入后，两个成员节点交换成员历史”的最小多节点基础，以及真实 Engine nightly
的运行入口。它不实现完整邀请/准入，也不把这一切片记作五类业务覆盖完成。

### 完整负责人和唯一动作

- `VirtualMembershipNetwork` 只负责测试节点注册、有向链路状态、frame 预算和脱敏 trace；它把 typed
  `MembershipHistoryMessage` 交给目标节点真实 `MembershipHistoryExchangeEndpointPort`。
- 每个节点使用真实 `MembershipLedger` 和 `HandleMembershipHistoryMessageUseCase`；网络不读取 ledger，不生成 ACK，
  不判断成员关系。
- 场景只准备两个合法节点，执行 `send`、`partition`、`heal`，并断言公开的历史交换结果和网络 trace。
- 成功结果：开放链路调用真实 endpoint 并得到业务 ACK；分区时得到 `Offline`；恢复后再次成功，trace 顺序和
  frame 数稳定。
- 失败结果：未知节点、重复节点、frame 预算耗尽或 endpoint 拒绝返回稳定 test-only 错误，并由 testkit 工件记录。
- 重试责任：virtual network 不重试；场景显式恢复链路并再次调用。生产业务欠账与重试仍由 Application 负责人拥有。

### 失败方式（先于实现固定）

| 失败方式 | 预期 |
| --- | --- |
| 重复节点键或重复业务身份 | 构造/注册立即失败，不覆盖原节点 |
| source/target 未注册 | 返回稳定 fixture/unavailable 失败，不调用 endpoint |
| 单向分区 | 只阻断指定方向，反向链路不受影响 |
| endpoint 业务拒绝 | 保留 endpoint 错误分类，trace 记录 rejected，不包含 payload/身份 |
| frame 预算耗尽 | 下一次发送立即失败，不回绕、不提高预算 |
| endpoint 嵌套或异步执行 | 网络锁在 `await` 前释放，不能死锁 |
| trace 泄露 | 记录只含节点测试标签、协议、序号和结果，不含 DeviceId、消息、路径或地址 |

### 本轮目录与验收

- `crates/uc-application/src/space/membership/testing/virtual_membership_network.rs`：最小有向网络和两节点真实 endpoint 场景。
- `crates/uc-application/src/space/membership/mod.rs`：只在 `cfg(test)` 注册 testing 模块，不扩大 crate 或产品公开接口。
- `.github/workflows/engine-real-environment.yml`：scheduled 四种真实环境模式和 `workflow_dispatch` 单 mode/单 case；
  复用 `run-connection-recovery-e2e.sh`，不复制 host 或网络脚本。
- 快速场景预算 1 秒，不使用固定 sleep；连续运行至少 20 次无随机失败。
- nightly 每个 mode 独立 job 和工件，编译、环境准备、场景、清理与总耗时可从 job/summary 区分；单 mode 目标
  30 分钟内。首次实际 scheduled 运行仍待合并后自然触发，PR 中只验证 workflow 语法和现有真实脚本门禁。
- 回退点：virtual 模块和 nightly workflow 可独立回退；旧测试、PR 网络门禁和脚本均不删除。

### 当前切片完成记录

- `VirtualMembershipNetwork` 已在 `cfg(test)` 下实现节点注册、有向分区、恢复、frame 预算和脱敏 trace；消息交给
  目标节点真实 `HandleMembershipHistoryMessageUseCase`，网络本身不读取或解释成员账本。
- 场景 `two_member_nodes_partition_and_heal` 覆盖开放链路确认、单向阻断、恢复后再次确认和预算耗尽；固定 seed 为
  `0x0040_3401`，预算 1 秒，不使用固定 sleep。
- nextest 单场景 20 轮全部通过，总墙钟 11 秒；统一 evidence 入口 15/15 通过，测试累计 2.077 秒，场景工件记录
  `cleanup=completed` 和精确复现命令。
- 相关旧成员历史测试 21/21 通过；`uc-application` 完整库测试 960 通过、1 项既有忽略。
- 新增真实 Engine nightly/手工入口，复用既有 connectivity host、relay、network namespace 与证据脚本。四种 mode
  独立运行，手工入口可选择单个 case；编译、场景与清理、总耗时分别记录，场景设置 30 分钟硬超时。
- nightly workflow 只有进入默认分支后才能自然 scheduled 运行；当前只完成本地语法、现有脚本参数和 PR 门禁兼容
  验证，不把该基础登记为真实外网、真实 relay 或设备通过。
- 本切片只证明“已完成准入后的两节点成员历史传递基础”，不等于完整配对，更不等于首批五类业务覆盖完成。

## 当前真实进程切片：完整配对与文字传输（2026-09-22）

本切片复用现有 `uc-connectivity-host`、Linux network namespace 和
`connection-recovery-network.mjs`。现有 runner 已经执行真实 Engine 完整配对和双向 exact text，但两步此前只是后续
恢复场景的隐式 setup，不能按业务场景单独选择、计时和出具证据。本轮只把既有动作登记为稳定场景，不新增业务
状态机，不改生产接口，也不把文字覆盖冒充文件传输覆盖。

### 场景、负责人和复用边界

| 场景 | 开发者描述 | kit/runner 责任 | 最终公开断言 |
| --- | --- | --- | --- |
| `E01-complete-pairing` | 准备两个或三个 Engine 节点并完成配对 | 独立 profile/身份/端口、rendezvous、进程生命周期、事件等待、预算、清理和证据 | 加入方 setup 完成且 Space 一致；各节点普通通信资格为 usable；真实 peer connection online |
| `E02-text-transfer` | 已配对节点之间双向发送指定文字 | 复用 E01 setup、发送、历史轮询、耗时和失败证据 | 每个方向只接受一个目标，接收端按公开 history/entry 读取到 exact text |

完整配对和传输继续由真实 Engine `Operation` 与 Application 负责人执行。runner 只调用公开测试宿主命令并等待公开
结果；不得解释准入阶段、成员 ACK 或内容传输状态。为支持后续 `E03` 等场景，E01/E02 在未被选择时仍作为必需
setup 执行，但不登记为本次所选证据；全量运行和显式选择时才写入对应 scenario record。

### 失败方式和诊断要求

| 失败方式 | 失败证据 |
| --- | --- |
| 加入未完成或 Space 不一致 | `E01` 失败，保留最后事件、节点状态、阶段耗时和复现命令 |
| 对端身份不可查询 | `E01` 失败为 `paired identity unavailable`，不得继续伪造 peer id |
| usable 或 online 未收敛 | `E01` 失败并保留节点连接事实 |
| 发送被拒绝、离线、待定或重复 | `E02` 记录公开发送汇总和脱敏原因分类 |
| exact text 未到达 | `E02` 失败为未满足条件，不用固定 sleep 或放宽内容断言 |
| 进程或 namespace 清理失败 | 场景结果与 cleanup 结果分别记录；成功场景不能覆盖清理失败 |
| 证据包含正文、设备身份、地址或路径 | 隐私检查失败，工件不得作为通过证据 |

### 预算、验证范围和回退

- 聚焦 `E01` 或 `E02` 的真实进程运行目标为场景及清理合计 60 秒内，编译单独计时；nightly 单 mode 的环境准备、
  场景和清理实测目标仍为 30 分钟内。超时是失败，不通过自动重试、删断言或提高预算掩盖。
- 本轮本地验证运行脚本语法、受影响的静态检查和旧入口兼容。Linux 真实网络必须由受影响的远程 network job 或
  等价隔离 Linux 环境实际执行，并读取该 job 上传的 JSON/文本证据。
- 远程验收只等待本轮相关的 repository 检查和真实网络 runner job/step。相关步骤通过且工件可读取后即可继续；
  其他慢作业仍运行时明确记录“未等待”，不把整条 workflow 写成全绿。相关 job 失败必须定位修复。
- 文件传输当前不是本切片：现有 connectivity host 的 `HostFileAccess` 明确返回 unavailable，必须在后续切片增加
  受管测试文件能力和真实 `SendFiles`/接收证据后才能登记覆盖。
- 回退只删除 E01/E02 场景登记和对应文档；既有 setup、E03-E13、旧 PR 门禁和 nightly 入口保持不变。

### 验收标准

- `--mode direct --case E01` 和 `--mode direct --case E02` 可独立运行，并分别产生准确场景、耗时、清理和复现证据。
- 不带 `--case` 的旧 direct/relay 流程继续先完成配对和基线文字传输，随后执行原恢复场景。
- 证据明确区分场景失败与 cleanup 失败，且不包含 exact text、真实身份、地址或本机路径。
- 报告更新五类覆盖映射：完整配对和文字传输记为已实现；文件传输、旧资料升级的真实 nightly 仍保持未完成；
  重连和重启只引用现有 E03/E04/E06/E10/E11/E12/E13，不重复重写。

## 当前真实文件与旧资料升级切片（2026-09-22）

本切片补齐两个已确认缺口，不扩建通用模拟层：connectivity host 提供一个进程内受管文件表，使真实 Engine 可通过
公开 `SendFiles` 读取固定测试字节；接收端继续通过公开 history 和 `ReadEntryFile` 验证文件名、媒体类型与完整字节。
旧资料升级不另写场景，nightly 直接复用现有 `profile_storage_upgrade` 和独立进程 crash recovery 测试。

### 完整负责人和唯一动作

- 文件内容的导入、加密历史、网络发送、接收 blob 和读取仍由 Engine/Application/Infra 原负责人完成。测试宿主只按
  opaque `HostFileHandle` 保存输入 bytes 和 metadata，不读取产品状态，也不实现传输状态机。
- 开发者场景只描述“在节点 A/B 准备固定文件并双向发送，接收端读取同一 entry”；runner 继续负责节点、profile、
  namespace、等待、预算、清理和脱敏证据。
- 旧资料升级 nightly 只调用既有测试 binary；升级、崩溃恢复、重启和清理由原测试负责人及 testkit 完成。

### 实现前失败清单

| 失败方式 | 预期诊断 |
| --- | --- |
| 未登记或空文件句柄 | host 返回稳定 invalid handle/unavailable，场景失败 |
| offset 溢出或越界读取 | host 返回稳定 IO/空尾块，不 panic |
| Engine 拒绝、离线或未接受文件发送 | E02 file 子步骤记录发送汇总 |
| 接收历史没有 file entry | 事件驱动等待耗尽并报告最后节点状态 |
| 文件名、media type 或 bytes 不一致 | product assertion 失败，不只检查“有记录” |
| cleanup 或 plaintext scan 失败 | 与业务结果分开记录，整体不通过 |
| 升级测试或 crash recovery 失败 | nightly upgrade job 失败并上传 testkit 工件 |
| alpha.5 外部 fixture 不存在 | 明确保持未执行，不用环境变量或本机路径伪造通过 |

### 预算、验收与回退

- `E02-file-transfer` 聚焦运行目标为环境准备、双向发送、接收读取和清理合计 60 秒内；nightly upgrade job 的测试与
  清理目标 30 分钟内，编译和总耗时分别显示。
- 文件场景必须使用真实多进程 Engine 和真实隔离网络；本地 macOS 只做 host 编译与静态检查，远程 network job 或
  等价 Linux 隔离环境才构成运行证据。
- nightly upgrade 必须实际运行现有 synthetic profile migration 与五个 crash boundary；alpha.5 完整 fixture 因依赖
  外部合成资料继续列为未验证，不能用普通 migration 测试冒充。
- 回退可独立删除 host 受管文件命令、E02 file 子场景和 upgrade job；旧测试、旧门禁、存储格式与生产接口不变。

### 当前完成记录

- `E02-file-transfer` 已实现双向真实 Engine 文件发送：测试宿主只保存 opaque 受管输入，发送端调用公开
  `SendFiles`，接收端从公开 history 定位 file entry，再以 `ReadEntryFile` 核对文件名和完整 bytes。没有新增产品状态机、
  生产公开接口、协议或持久格式。
- 旧资料升级 nightly 已接入既有 `profile_storage_upgrade` 与 `profile_storage_upgrade_crash` 测试 binary；本地 nextest
  18/18 通过、2 项外部 fixture 保持 ignored，测试累计 5.546 秒。crash recovery 工件为 passed、固定 seed
  `0x00400304`、cleanup completed。
- connectivity host check、workspace all-target check、metadata、fmt、脚本语法、Rust style、repository、privacy 与 diff
  check 均通过；旧本地入口 89 项通过。alpha.5 外部完整 fixture、真实外网和设备没有执行。
- 当前提交的受影响远程验收限定为 repository checks、真实网络 runner 的场景步骤和同 job 清理工件，均已通过。
  direct 与 relay 的 E01、exact text 和双向 exact bytes 均通过；四种模式工件均为 `failed=false`、`cleaned=true`、
  `plaintext_clean=true`，并包含准确复现命令。其他无关慢 job 不作为本切片等待条件。
- `profile-upgrade` workflow 尚未进入默认分支，GitHub 不允许从当前 PR 分支触发新增的 workflow definition；本轮只登记
  等价本地隔离证据，不把 schedule 或 workflow_dispatch 写成已生效。

### 首批五类双线验收状态

| 类别 | 快速确定性线 | 真实 nightly / 手工线 |
| --- | --- | --- |
| 配对 | 部分完成：成员恢复五场景和已准入成员历史网络；无完整快速 invitation -> settled | E01 当前提交 direct/relay 已验证 |
| 文字与文件传输 | 部分完成：文件接收生命周期进入 fast；无快速网络或 exact bytes | E02 text/file 当前提交 direct/relay 已验证 exact value/bytes |
| 断线重连 | 部分完成：成员消息在 partition/heal 后恢复；不等于 Engine transport 重连 | E03/E04/E06/E10/E13 当前 runner 回归通过 |
| 重启恢复 | 部分完成：Application 持久准入重建，不是 Engine 进程重启 | E11/E12 当前 runner 回归通过 |
| 旧资料升级 | focused migration/process 18 项本地通过 | workflow 已接入但默认分支未生效；alpha.5 fixture 未验证 |

当前真正可复用的“准备/操作/预期”入口只有三类：Application `Scenario` + 领域 fixture、只覆盖已准入成员历史的
`VirtualMembershipNetwork`，以及真实 runner 的 `--mode`/`--case`/`--repeat`。不能把真实 runner 的 E01/E02 命名算作
快速多节点五类已经实现。

## 当前快速配对作者入口与真实环境计时切片（2026-09-22）

### 本 PR 测试目录收敛

在继续扩展快速线前，先解决测试与业务文件混排。迁移只覆盖本 PR 新增或扩展的场景，不做全仓重排：

| 调整前 | 调整后 | 原因 |
| --- | --- | --- |
| `space/admission/protocol/admission_recovery_scenarios.rs` | `space/admission/protocol/tests/admission_recovery_scenarios.rs` | 场景需访问 admission 私有装配，但文件名与业务实现并排，无法一眼识别为测试 |
| `space/admission/protocol/pairing_scenario_fixture.rs` | `space/admission/protocol/tests/support/pairing_scenario_fixture.rs` | 这是 admission 专用作者 fixture，不属于生产 protocol，也不应进入通用 testkit |
| `rendezvous/invitation_adapter.rs` 内的 `provider_dependency_evidence_reports_all_outcomes` | `rendezvous/invitation_adapter/tests/provider_dependency_evidence.rs` | 场景必须访问 adapter 私有 helper，保留私有访问但从业务实现文件移出 |

以下路径保持不动：`membership/**/tests/` 已符合私有场景规则；`membership/testing/` 是明确命名的领域虚拟网络；
crate `tests/` 下的升级/进程场景只使用公开接口；既有 `protocol/test_support.rs` 虽然仍与业务并排，但属于历史大型
支撑，本轮移动会造成大量无关引用变化，登记为后续自然收敛项。

验收标准：普通 `cargo check` 不依赖 `uc-testkit` 或上述 test-only 模块；nextest 现有名称选择器继续选中相同场景；
旧 `cargo test` 入口继续通过；文档与采用清单不再引用调整前路径。回退只还原模块声明和文件位置，不改变生产行为、
公开接口、协议或持久格式。

### 最小交付与复用点

本切片只补两个已证实缺口，不建设统一多节点 DSL：

1. admission 测试作者目前需要知道恢复轮次、激活入口和最终确认顺序。新增 test-only
   `PairingScenarioFixture`，作者只准备加入输入、执行一次 `complete_joiner_pairing`，并断言返回的稳定快照。
   fixture 调用真实 `SpaceAdmissionProtocol`、成员维护入口和激活入口，不生成协议回复、不解释内部阶段；已有
   `SpaceAdmissionProtocolTestPair` 继续提供可控 transport、clock 和持久状态。
2. 真实 runner 工件只有逐场景耗时和 job 总墙钟，不能区分环境准备与清理。runner 在同一 JSON envelope 增加
   `timings.prepare_ms`、`timings.scenario_ms`、`timings.cleanup_ms` 和 `timings.total_ms`；计时只观察 runner 自己的
   生命周期，不改变场景、预算或重试。

快速场景选择加入方完整收敛，因为它复用现有真实负责人、补齐 invitation -> active settled 的作者入口，并能在
1 秒预算内完成。Sponsor 最终确认唯一性继续由既有三设备场景证明，双方真实 Engine 的 same-space、usable 和 online
继续由 E01 证明；本切片不机械复制真实链路。

### 完整负责人、唯一动作与结果

- 完整负责人仍是 `SpaceAdmissionProtocol`。fixture 只把已有完整动作组合成一次测试调用，不保存自己的业务阶段。
- 作者唯一动作：构造 `JoinSpaceInput` 后调用 `complete_joiner_pairing`；成功返回 `CurrentJoinStatus::Active` 与
  `final_confirmation_complete=true` 的脱敏快照。
- 失败结果：开始加入、成员维护、激活或最终确认任一步失败，返回稳定 fixture/product condition；testkit 记录阶段、
  最后事件、固定 seed、复现命令和工件位置。
- 重试和重启仍由生产 admission 负责人决定；fixture 不自动重试。既有 retry/restart 场景继续单独验证相应规则。
- 真实 runner 只记录 prepare/scenario/cleanup/total；cleanup 失败仍使场景失败，不能被 timing 覆盖。

### 实现前失败清单

| 失败方式 | 预期 |
| --- | --- |
| 加入输入无法保存 | fixture 返回 `join-start`，不进入恢复 |
| maintenance 在激活前 deferred/stable failure/corrupt | fixture 返回准确 condition，不猜测阶段、不增加循环次数掩盖 |
| 激活失败 | 保留原失败，场景工件标出 activation stage |
| final confirmation 未完成 | 快照不得写成 settled，场景以 product invariant 失败 |
| fixture 复制消息或持久状态机 | 架构审查失败；只允许调用现有完整负责人和读取测试仓储结果 |
| runner 在首场景前失败 | `prepare_ms` 保留，`scenario_ms` 为 0，cleanup 仍执行并计时 |
| 场景失败后 cleanup 失败 | 业务失败保持 primary，envelope 同时记录 `cleaned=false` 与 cleanup 时间 |
| 时间字段不满足总量关系 | 工件检查失败；允许毫秒取整误差，不允许负值或缺字段 |

### 预算、验收与回退

- 新快速场景预算 1 秒；单次 nextest 目标小于 1 秒，连续 20 轮无随机失败，加入现有 fast/evidence 选择器。
- 作者示例必须只出现准备输入、执行一次场景动作和断言最终快照，不暴露消息、恢复轮次或内部阶段。
- 与既有 `settled_is_saved_and_finishes_joiner_recovery` 双轨 20 轮，比较最终 joiner settled 结果；旧测试不删除。
- 真实 runner 修改后，本轮相关 Linux network step 必须通过并读回四种 mode 工件；每份 timing 字段和 cleanup 均核对。
- 30 分钟目标按 mode 的 `prepare + scenario + cleanup` 实测。PR 全矩阵仍可作为当前样本，但默认分支 nightly
  尚未生效时，不把 scheduled 入口记为通过。
- 回退可独立删除 test-only fixture/场景和 timing 字段；旧测试、真实场景、门禁、生产接口、协议与持久格式保持。

### 当前完成记录

- 本 PR 新增/扩展的测试已按职责收敛：admission 场景位于 `protocol/tests/`，专用配对 fixture 位于
  `protocol/tests/support/`，provider 证据位于 `invitation_adapter/tests/`。业务目录不再出现无测试标识的新增场景文件，
  `invitation_adapter.rs` 不再内嵌 testkit 长场景；没有扩大生产可见性或新增生产测试开关。
- 新增 test-only `PairingScenarioFixture`。作者示范只准备 `JoinSpaceInput`、调用一次 `complete_joiner_pairing` 并断言
  Active + final confirmation；实现调用真实 admission maintenance、激活和最终确认负责人，没有生成消息或保存平行阶段。
- 测试先于实现落下，初次按预期因 fixture 模块不存在而编译失败；最小实现后场景通过，单次 nextest `0.040s`。
- 新场景与既有 `settled_is_saved_and_finishes_joiner_recovery` 双轨 20 轮全部通过，总墙钟 11 秒。工件包含固定 seed
  `0x00403402`、`complete-joiner-pairing` 阶段、最后事件、复现命令和 cleanup completed。
- evidence 16/16 通过、测试累计 1.944 秒；fast 基础组 7/7 通过；`uc-application` 全库 961 通过、1 项既有忽略。
- 真实 runner JSON 已增加 prepare/scenario/cleanup/total 四项计时，脚本语法通过；Linux 实际值与 cleanup 工件仍须由
  本轮相关远程 network step 读回后才能登记。
- 快速配对仍标为“部分”：它证明 joiner 确定性收敛；Sponsor 唯一性沿用三设备场景，双方真实链路沿用 E01。

## 当前快速文件传输生命周期切片（2026-09-22）

### 最小交付与真实边界

快速线不模拟文件字节网络，也不复制 E02。它复用 `uc-application` 现有公开 `FileTransferFacade` integration fixture，
新增一个作者场景：准备一个接收传输，执行 progress 与 complete，断言最终公开事件只有一个 `Completed` 且进度保持
单调。testkit 只提供 1 秒预算、阶段、固定 seed、失败分类和工件。真实文件内容、双向发送、history entry 和
`ReadEntryFile` exact bytes 继续只由真实 E02 证明。

完整负责人仍为 `FileTransferFacade`；作者唯一动作是开始一个已登记的 receiver session 并完成它。失败结果分别为
fixture 调用失败或最终公开事件不满足 product invariant；不增加自动重试。场景位于 crate `tests/file_transfer.rs`，
因为它只使用公开 Application/Core 接口和该 integration test 自有 ports。

### 实现前失败清单

| 失败方式 | 预期 |
| --- | --- |
| receiver registration 被拒绝 | `fixture_invalid`，最后阶段为 begin |
| progress 被拒绝或倒退 | `product_invariant`，不放宽为只检查终态 |
| complete 失败 | `product_invariant`，保留最后事件和 complete 阶段 |
| 公开 history 缺少或出现多个 terminal event | `product_invariant`，报告准确 condition |
| 测试自行传输 bytes 或解释网络状态 | 架构验收失败；真实内容只由 E02 验证 |
| 超过 1 秒预算 | 场景失败，不增加 sleep、重试或扩大预算 |

### 验收与回退

- 新场景单次 nextest 小于 1 秒，连续 20 轮稳定；加入 fast/evidence 选择器并生成 JSON/文本/JUnit。
- 统一脚本传给各 crate 的工件根必须是仓库绝对路径；不得因 integration test 工作目录不同把报告写进 crate 内的
  `target/`，脚本打印位置必须与实际文件一致。
- 与既有 `repeating_same_terminal_call_is_idempotent` 双轨 20 轮，二者都断言完成终态且旧测试保持权威。
- `cargo test -p uc-application --test file_transfer` 旧入口继续通过；普通 Application 构建不依赖 testkit。
- 回退只删除新场景和选择器；不改变 facade、ports、生产行为、协议或持久格式。

### 当前完成记录

- 新场景 `file_transfer_completion_scenario_reports_final_state` 只使用公开 `FileTransferFacade`，按 begin、progress、
  complete 三个阶段断言唯一 `Completed` 终态；固定 seed 为 `0x00403403`，单次 nextest `0.033s`。
- 与既有 `repeating_same_terminal_call_is_idempotent` 双轨 20 轮全部通过，总墙钟 11 秒；完整旧
  `file_transfer` test binary 15/15 通过、`0.07s`。
- fast 统一入口现在包含 testkit 与 Application 快速场景，15/15 通过、测试累计 `0.472s`；evidence 17/17 通过、
  测试累计 `2.019s`。JSON/摘要记录三个阶段、最后事件、cleanup completed 和准确复现命令。
- 实际接入发现相对 `UC_TEST_ARTIFACTS_DIR` 会受 integration test 工作目录影响；统一脚本现传递仓库绝对工件根，
  实际文件位置与打印位置一致。未新增 testkit API 或配置层。
- 已有 `two_member_nodes_partition_and_heal` 明确登记为快速重连规则的部分证据：链路阻断时返回 unavailable，heal 后
  同一真实 Application endpoint 接受消息；它不证明 Iroh/Engine transport 重建，后者继续由 E03/E04/E06/E10/E13 负责。

## 当前快速文字传输切片（2026-09-22）

快速文字场景复用既有 `ClipboardSyncFacade` 完整负责人和测试 ports：作者准备一个 `text/plain` 快照，执行一次
`dispatch_snapshot`，断言 V3 envelope 被编码、canonical snapshot hash 产生、目标 peer 得到 accepted 结果。最终 transport
ACK 由既有 mock 固定，真实网络 exact text 仍只由 E02 证明。

原场景较长且内嵌在 `facade.rs`，本轮按新目录规范移到 `facade/tests/text_transfer_scenario.rs`，作为私有实现测试子模块；
不扩大 facade 或 port 可见性。testkit 增加 1 秒预算、encode/dispatch 阶段、固定 seed 和结构化报告，不改变业务调用。

失败方式：快照未编码为 V3、加密入口未收到 envelope、目标 fan-out 未发生、accepted 数量或 canonical hash 错误时均为
product invariant；fixture 装配失败为 fixture invalid；超时直接失败，不自动重试。验收为单次小于 1 秒，与原
`dispatch_entry_returns_public_outcome_for_online_peer` 双轨 20 轮，进入 fast/evidence，旧 facade 测试入口继续通过。

### 当前完成记录

- `text_transfer_scenario_encodes_and_dispatches_snapshot` 已移入明确的私有测试子目录，复用真实
  `ClipboardSyncFacade`、固定 seed `0x00403404` 和 1 秒预算；单次 nextest `0.043s`。
- 与既有 `dispatch_entry_returns_public_outcome_for_online_peer` 双轨 20 轮全部通过，总墙钟 12 秒；完整
  `uc-application` lib 入口 `961 passed, 1 ignored`，测试耗时 `21.96s`。
- fast 统一入口现在 16/16 通过、测试累计 `0.268s`；evidence 18/18 通过、测试累计 `2.122s`。
  结构化工件记录 `passed`、最后事件 `text-dispatch-accepted`、encode/dispatch 阶段、cleanup completed、固定 seed
  和准确复现命令。
- 快速场景只证明 V3 envelope、canonical hash 和 accepted fan-out；transport ACK 仍由测试 port 控制，真实网络
  exact text 继续由 E02 负责。普通构建不依赖 testkit，回退只需移除测试子模块与选择器。

# 1. Overview

规格 030 已用真实 Engine operation、SQLite、Iroh endpoint、网络分区和正文传输完成 F0-F7 验收。其中 F7 单项
耗时 430.46 秒，规格 029 的 Desktop C0-C5 串行验收也超过十分钟。真实验证证明了交付链路，但把成员协议、
持久化、安全状态、Engine 生命周期、rendezvous 和 Iroh 连接同时放入每个复杂拓扑，造成三个问题：

1. 多数协议回归只有运行到分钟级 E2E 才能发现，反馈过慢。
2. 失败同时跨越 Application、Infra 和 Engine，难以判断是拓扑规则、持久恢复还是 Iroh adapter 问题。
3. 环、深链和不平衡树只能通过最终状态间接判断；缺少逐协议、逐链路的确定性消息预算，难以直接证明无循环、
   无重复 effects 和无公平性饥饿。

本规格在现有领域 port seam 上增加 test-only virtual provider。它把真实 `SpaceApplication`、成员账本、反熵、
冲突恢复和维护顺序连接成内存多节点拓扑，以确定性节点顺序、逻辑时钟和有界 frame trace 执行 F0-F7 的协议矩阵。
Iroh 继续是生产 adapter；ALPN、认证 remote identity、codec、frame bounds、QUIC timeout、连接关闭与重连由独立
Iroh provider contract 和小型 Engine smoke 验证。原 F0-F7 真实 Iroh 测试不删除、不改写历史结果，转入明确的
nightly/release slow lane。

本规格不增加一个覆盖全部网络能力的生产 `TransportProvider`。现有窄 port 已经是实际 seam；再包装成总 provider
只会复制 Engine 组装清单、泄露 Iroh 生命周期，并形成浅模块。

# 2. Goals

- 在 `uc-application` 内建立不进入生产构建和公开白名单的 deterministic virtual membership network。
- 复用真实 `MembershipHistoryAntiEntropy`、冲突选择/恢复、成员维护和 ledger CAS 流程，不实现第二套成员状态机。
- 通过现有 `MembershipHistoryExchangePort`、`RestrictedMembershipDeliveryPort`、`GroupUpdateDispatchPort` 和
  `MembershipBranchRecoveryChannelPort` 注入 virtual adapter，不改变生产 port 接口或所有权。
- 以 typed domain message 传输，保证测试覆盖 port 以上的真实业务语义；Iroh wire 由真实 provider contract 覆盖。
- 用稳定节点顺序、逻辑时钟、round/frame 双预算和脱敏 trace 确定性执行 F0-F7。
- 将 F0-F7 中的 branch/head、成员资格、冲突、effects、group epoch、公平性和授权矩阵放入常规 Application 测试。
- 保留真实 SQLite、control-generation、MLS、Iroh partition、Engine 生命周期和 exact content 的独立验证证据。
- 同一场景重复运行时产生相同的最终 snapshot 和 trace signature；失败可用场景名和固定 seed 单独复现。
- 常规 virtual F0-F7 总耗时在当前 macOS-14 PR runner 上不超过 30 秒，且场景内不使用真实 `sleep` 等待收敛。

# 3. Non-Goals

- 不修改规格 029/030 已完成的历史证据，也不把 virtual 结果登记为真实 Iroh、SQLite、MLS 或设备通过。
- 不创建 Engine 级通用 `TransportProvider`、万能字节总线或可由产品选择的网络 provider。
- 不公开 `SpaceApplication`、内部 use case、maintenance runtime 或测试 fixture。
- 不把 virtual network 加入 `uc-application` 的 `test-support` feature；该 feature 继续只服务已有外部测试需求。
- V1 不虚拟化邀请 discovery、完整 Space admission、clipboard/blob/file transfer 或 LAN compatibility。
- V1 不实现 `PeerReachabilityPort` 或模拟 Iroh presence；topology 在动作边界直接选择真实 maintenance trigger，
  presence cache、probe 和连接在线状态继续由现有 Iroh tests 验证。
- V1 不模拟 QUIC 握手、stream、拥塞、MTU、ALPN 协商、已有连接被关闭或 relay 行为。
- V1 不实现随机丢包、任意乱序、带宽、延迟分布或概率故障；F12 的 codec/分页/提交故障继续在责任层测试。
- 不用 in-memory repository 替代真实 SQLite 原子性、密文持久化或 control-generation 崩溃恢复证据。
- 不用授权矩阵替代 exact text、密文和错误密钥的真实数据面验证。
- 不顺便迁移现有 port 所有权，不清理与本规格无关的单元测试 fake。
- 不承担真实网络、真实 relay 或设备通过；这些由 nightly/手工真实环境线保留。
- 不要求所有真实环境场景与 virtual fixture 共用同一套场景实现，只要求覆盖映射和最终业务结果可对照。

# 4. Current Architecture Context

```text
Component: Engine F0-F7 membership topology E2E
Path: crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs
Responsibility: 通过公开 Engine operation、真实 profile 目录、SQLite、rendezvous 和 Iroh endpoint 执行复杂拓扑。
Relationship: 当前 `MembershipTopology` 同时拥有节点生命周期、邀请、endpoint id、分区、轮询和业务断言；保留为真实 slow lane，协议矩阵迁入 Application virtual suite。
```

```text
Component: SpaceApplication
Path: crates/uc-application/src/space/application.rs
Responsibility: 组装成员 ledger、历史 endpoint、冲突恢复、维护负责人及 Space 生命周期出口。
Relationship: 已有 crate-private `build_for_test` 和 endpoint accessor；virtual fixture 应在同 crate 的 `cfg(test)` 模块使用，不扩大公开 interface。
```

```text
Component: MembershipHistoryAntiEntropy
Path: crates/uc-application/src/space/membership/anti_entropy.rs
Responsibility: 统一承担历史入站、出站、ACK、水位、重试欠账与同步结果。
Relationship: virtual history adapter 必须把消息交给远端真实 endpoint；测试不得自行解释 summary、suffix 或 ACK。
```

```text
Component: Membership transport ports
Path: crates/uc-core/src/membership/ports.rs, crates/uc-application/src/space/membership/recover_conflict/ports.rs
Responsibility: 表达历史交换、受限投递、group update 和两阶段 branch recovery 的领域能力。
Relationship: Iroh 与 virtual 是这些既有 seam 上的两个 adapter；034 不增加上层总 provider。
```

```text
Component: Iroh membership adapters
Path: crates/uc-infra/src/network/iroh/membership_history_exchange_adapter.rs, crates/uc-infra/src/network/iroh/group_update_adapter.rs, crates/uc-infra/src/network/iroh/membership_branch_recovery_adapter.rs
Responsibility: 地址解析、ALPN、认证来源、codec、frame bounds、timeout、ACK 与 endpoint dispatch。
Relationship: 继续作为生产 adapter；独立 contract 验证 port 到 Iroh 的映射，virtual suite 不复制 wire 实现。
```

```text
Component: IrohNetworkPartitionGate
Path: crates/uc-infra/src/network/iroh/network_partition.rs
Responsibility: 在连接前和握手后拒绝 blocked endpoint，并关闭已建立连接。
Relationship: `VirtualPeerNetwork::partition` 只阻断动作边界后的新领域调用，不能替代真实 gate contract。
```

```text
Component: Membership persistence and branch transition integration tests
Path: crates/uc-infra/tests/membership_ledger.rs, crates/uc-infra/src/security/v3_membership_branch_transition/tests.rs
Responsibility: 验证真实 SQLite、MasterKey AEAD、CAS、nonce、control-generation 阶段与崩溃恢复。
Relationship: virtual node restart 只验证 Application 重新组装和恢复决策；介质与安全原子性继续由这些测试证明。
```

当前数据流为：Application 调用领域 port，Iroh adapter 编码并建立连接，远端 handler 从连接身份解析
`DeviceId` 后调用 Application endpoint。复杂拓扑通过 Engine dev operation 把 endpoint id 加入分区 gate。virtual
suite 只替换“领域 port 到远端 endpoint”这一段，前后的 Application 业务逻辑保持不变。

# 5. Proposed Design

## Components

### `VirtualPeerNetwork`

- **位置**：`crates/uc-application/src/space/testing/virtual_network.rs`
- **职责**：保存 test node 注册表、有向 link policy、单调 frame sequence、调用计数和脱敏 trace；按领域协议把一次
  调用路由到目标 endpoint。
- **输入**：已注册 source、目标 `DeviceId`、`VirtualProtocol` 和 typed request。
- **输出**：typed response 或 `VirtualDeliveryError`。
- **关系**：它不读取 ledger、不运行 membership policy、不生成 ACK、不保存重试欠账。

删除检查：若删除该模块，身份绑定、分区、路由、预算和 trace 会重新散落到每个 fake port 和 F0-F7 场景，因此
该模块应隐藏这些共同知识。

### `VirtualMembershipTransport`

- **位置**：`crates/uc-application/src/space/testing/virtual_transport.rs`
- **职责**：作为每个节点的 test-only adapter bundle，实现现有领域 ports，并把 `VirtualDeliveryError` 映射为各
  port 的稳定错误分类。
- **输入**：本机注册身份、共享 `VirtualPeerNetwork`。
- **输出**：可注入 `SpaceRuntimeAdapters` 的 trait objects。
- **关系**：source identity 只能从 adapter 注册信息取得，调用参数不能伪造；远端 endpoint 仍执行真实业务验证。

### `VirtualMembershipNode`

- **位置**：`crates/uc-application/src/space/testing/virtual_node.rs`
- **职责**：持有可跨重建保留的 test repositories、确定性 clock/signature/security adapters，以及当前
  `SpaceApplication`；提供生产负责人级动作，不暴露 ledger 逐字段修改。
- **输入**：`VirtualNodeSeed` 和 transport bundle。
- **输出**：成员动作结果、诊断 snapshot、授权 scope 和 endpoint registration。
- **关系**：节点启动后禁止测试直接写 repository。`restart` 销毁并重建 `SpaceApplication`，复用同一 durable
  test repository 和逻辑身份。

### `VirtualMembershipTopology`

- **位置**：`crates/uc-application/src/space/testing/topology.rs`
- **职责**：F0-F7 的唯一测试入口；按稳定顺序执行节点动作、网络控制、逻辑时间推进、maintenance round、预算和
  最终断言。
- **输入**：声明式 `VirtualTopologyAction`、`ConvergenceExpectation`、`VirtualExecutionBudget`。
- **输出**：`VirtualTopologySnapshot`、trace signature 或 `VirtualTopologyFailure`。
- **关系**：它驱动完整 Application owner，不解释 membership message 或 transition phase。

### Application manual maintenance test seam

- **位置**：`crates/uc-application/src/space/application.rs`
- **职责**：仅在 `cfg(test)` 下允许 topology 对当前 `MaintainSpaceMembershipUseCase` 执行一个明确 trigger。
- **输入**：`MembershipMaintenanceTrigger`。
- **输出**：真实 `MembershipMaintenanceReport`。
- **关系**：virtual scenario 不启动后台 interval runtime；生产构建、公开 interface 和 Engine 组装不变。

### Iroh membership provider contract

- **位置**：`crates/uc-infra/src/network/iroh/membership_provider_contract_tests.rs`
- **职责**：用真实 loopback endpoint 验证 Iroh adapters 对既有领域 ports 的实现，包括认证 source、codec、frame
  bounds、ACK、拒绝、timeout 分类、两阶段 recovery 和 partition gate。
- **输入**：真实 Iroh endpoints 与最小 endpoint fakes。
- **输出**：port 级成功或稳定错误分类。
- **关系**：只测试 adapter，不重跑完整 F0-F7 业务拓扑。

### Real-Iroh slow-lane runner

- **位置**：`scripts/testing/run-real-iroh-membership-topologies.sh`、`.github/workflows/membership-topology.yml`
- **职责**：串行执行被标记为 slow lane 的 F0-F7，并保存每项结果；支持 schedule 和手工触发。
- **输入**：当前提交与固定测试列表。
- **输出**：逐场景通过/失败；未执行时只能标为“跳过”。
- **关系**：不改变 release bundle 规则；正式发布前必须有同一提交的 slow-lane 结果或明确记录为跳过。

workflow 默认每日 UTC 02:00 执行，保留 artifact 14 天；同时提供 `workflow_dispatch`。

## Data Model

### `VirtualNodeKey`

test-only 稳定节点键。场景可显示 `A`～`J` 等固定标签，但不得包含真实设备名、`DeviceId`、endpoint id、地址或
路径。`DeviceId` 仅保存在注册表内部用于 port 路由和生产业务校验。

### `RegisteredMembershipEndpoints`

```rust
struct RegisteredMembershipEndpoints {
    device_id: DeviceId,
    history: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    branch_recovery: Arc<dyn IssueMembershipBranchRecoveryPort>,
    group_updates: Arc<dyn GroupRevocationPort>,
}
```

注册生命周期与 `VirtualMembershipNode` 的运行实例一致。`stop` 移除 endpoints 但保留节点 repository；`restart`
以相同业务身份和新 Application 实例重新注册。

### `VirtualProtocol`

固定枚举：

- `MembershipHistory`
- `RestrictedMembership`
- `GroupUpdate`
- `BranchRecoveryGroupInfo`
- `BranchRecoveryExternalCommit`

不得使用自由字符串协议名。Admission、clipboard、blob 和 LAN 不进入 V1。

### `VirtualLinkState`

有向 link 只有 `Open` 和 `Blocked`。未注册目标等价 `Unavailable`。`partition(groups)` 阻断所有跨组双向 link，
`bridge(left, right)` 只打开指定双向 link，`heal(nodes)` 恢复相关 link。link mutation 前 topology 必须完成当前
动作；V1 不定义对 in-flight delivery 的取消语义。

### `VirtualFrameRecord`

```rust
struct VirtualFrameRecord {
    sequence: u64,
    protocol: VirtualProtocol,
    source: VirtualNodeKey,
    target: VirtualNodeKey,
    outcome: VirtualFrameOutcome,
}
```

`VirtualFrameOutcome` 只含 `Accepted`、`Rejected`、`Unavailable`、`Invalid`。记录中禁止保存 payload、错误文本、
业务 id、branch/head、成员身份、凭据或地址。失败只保留最后 128 条记录；完整计数按 `(protocol, source, target,
outcome)` 聚合。

### `VirtualExecutionBudget`

默认值：`max_rounds = 128`、`max_frames = 10_000`、`max_trace_records = 128`。每个场景可以向下收紧，不能在测试
内部静默提高。预算耗尽返回 `BudgetExceeded`，并报告已执行 round、frame 聚合和脱敏 trace。

### `VirtualTopologySnapshot`

每个节点只保存断言所需的稳定测试投影：branch 等价类标签、effective member count、membership 状态、group
epoch、pending conflict/effect 数量、ledger revision、可恢复/需重新配对状态和授权 peer 集合。snapshot 不包含
原始 branch/head digest、签名、恢复包或密钥。

## API / Interface

所有接口均为 `pub(super)` 或更窄，并受 `cfg(test)` 限制：

```rust
impl VirtualMembershipTopology {
    async fn from_seeds(
        seeds: impl IntoIterator<Item = VirtualNodeSeed>,
    ) -> Result<Self, VirtualTopologyFailure>;

    async fn execute(
        &mut self,
        action: VirtualTopologyAction,
    ) -> Result<(), VirtualTopologyFailure>;

    async fn converge_until(
        &mut self,
        expectation: &ConvergenceExpectation,
        budget: VirtualExecutionBudget,
    ) -> Result<VirtualTopologySnapshot, VirtualTopologyFailure>;

    async fn run_rounds(
        &mut self,
        rounds: usize,
        trigger: VirtualMaintenanceTrigger,
    ) -> Result<(), VirtualTopologyFailure>;

    fn partition(
        &mut self,
        groups: &[&[VirtualNodeKey]],
    ) -> Result<(), VirtualTopologyFailure>;
    fn bridge(
        &mut self,
        left: VirtualNodeKey,
        right: VirtualNodeKey,
    ) -> Result<(), VirtualTopologyFailure>;
    fn heal(&mut self, nodes: &[VirtualNodeKey]) -> Result<(), VirtualTopologyFailure>;
    async fn stop(&mut self, node: VirtualNodeKey) -> Result<(), VirtualTopologyFailure>;
    async fn restart(&mut self, node: VirtualNodeKey) -> Result<(), VirtualTopologyFailure>;
    fn trace_signature(&self) -> [u8; 32];
}
```

`VirtualTopologyAction` V1 包含 `Partition`、`PartitionGroups`、`Bridge`、`Ring`、`Chain`、`Heal`、`Stop`、
`Restart`、`Remove`、`Decide`、`ResolveConflict`、`AdvanceClock`、`RunRounds` 和 `AssertSnapshot`。Admission
相关 `Create/Join` 不进入 V1；场景通过 `VirtualNodeSeed` 建立已验证前置历史。

`VirtualNodeSeed` 只能在节点构造前使用。它必须调用生产 `VersionedMembershipHistory` constructor、编码/解码和
签名验证生成合法 ledger，不能手写跳过验证的内部 record。节点启动后，成员改变只能调用真实 Application owner。

`VirtualPeerNetwork` 内部 route 顺序为：

1. 在短锁内解析 source 注册、target 注册、link 状态并分配 sequence。
2. 释放网络锁。
3. 调用远端 typed endpoint；任何网络锁不得跨 `await`。
4. 在短锁内记录稳定 outcome 和计数。
5. 返回 typed response 或映射后的 port error。

身份绑定规则：source `DeviceId` 永远来自 `VirtualMembershipTransport` 的注册信息。普通 scenario API 不接受
source `DeviceId` 参数；需要验证恶意来源的单元测试使用独立 `inject_unauthenticated_for_test`，不得进入拓扑 DSL。

错误映射固定如下：

| Virtual outcome | History | Restricted delivery | Group update | Branch recovery |
| --- | --- | --- | --- | --- |
| target missing / link blocked | `Offline` | `Deferred` | `Offline` | `Unavailable { source }` |
| endpoint business reject | `Rejected` | `Rejected` | `Rejected` | `Rejected { source }` |
| endpoint response/type invalid | `Transport` | `Rejected` | `Transport` | `Invalid { source }` |
| frame budget exhausted | `Transport` | `Deferred` | `Transport` | `Unavailable { source }` |

带 source 的 Application 错误必须保留 `VirtualDeliveryError` source chain；错误 `Debug` 和 Display 不包含身份或
payload。现有不携带 source 的 Core transport error 不在本规格顺带改型。

## Workflow

### 场景准备

1. fixture builder 使用生产 Core constructor 和 deterministic signer 创建共同 baseline 与合法 sibling histories。
2. 每个 `VirtualNodeSeed` 只包含该节点起始时应持有的完整已验证状态、逻辑身份和持久 test repositories。
3. topology 创建所有节点的 test adapters 和 dormant `SpaceApplication`，获取真实 endpoints 后注册到 network。
4. 不启动 `SpaceMembershipMaintenanceRuntime`；所有推进由 topology 的 manual maintenance driver 完成。

### 确定性收敛

1. topology 按 `VirtualNodeKey` 排序选择本 round 的在线节点。
2. 每个节点调用一次真实 `MaintainSpaceMembershipUseCase`，使用场景指定的 Startup、StateChanged、PeerOnline 或
   Periodic trigger。
3. transport 直接把 typed 请求送入目标 Application endpoint，并记录 frame outcome。
4. round 结束后读取只读 snapshot；若满足 expectation 立即成功。
5. 若仍可推进，场景显式推进 logical clock 后进入下一 round。
6. 达到 round/frame 预算仍未满足时返回 `BudgetExceeded`，输出 snapshot 差异、计数与最后 128 条脱敏 trace。

### 分区、停止与恢复

1. link mutation 只发生在两个已完成 action 之间。
2. `partition` 后的新调用立即得到对应 port 的 unavailable/offline 结果；Application 自己保存欠账和退避。
3. `stop` 注销 endpoints 并销毁当前 Application，不清除 repository。
4. `restart` 用原 repository、身份和 clock 重建 Application，重新注册 endpoints。
5. `heal` 只恢复 link；后续收敛仍必须由真实 maintenance round 发现和偿还欠账。

### Slow lane

1. F0-F7 原 Engine tests 保持源码和真实断言，增加带原因的 `#[ignore]` slow-lane 标记。
2. runner 显式逐项执行 F0-F7，固定 `--test-threads=1`，不得用名称模糊过滤遗漏场景。
3. nightly workflow 保存逐场景结果；发布前引用同一提交结果。未执行时记录“跳过”，不得沿用旧提交结果。

# 6. Implementation Plan

```text
Step 1
Files: crates/uc-application/src/space/application.rs, crates/uc-application/src/space/mod.rs
Change: 增加 cfg(test) manual maintenance driver，保留同一个生产 MaintainSpaceMembershipUseCase；注册 testing 子模块。
Risk: 若复制 assembly 或启动第二个 runtime，会产生双 owner；测试模式必须只有手动 driver 推进。
```

```text
Step 2
Files: crates/uc-application/src/space/testing/virtual_network.rs, virtual_transport.rs
Change: 先写 link、identity、routing、error mapping、frame budget 和脱敏 trace 红测，再实现 virtual network 与四类 port adapter。
Risk: 网络锁跨 endpoint await 会死锁；source 从调用参数取得会允许伪造认证身份。
```

```text
Step 3
Files: crates/uc-application/src/space/testing/fixtures.rs, virtual_node.rs, topology.rs
Change: 提取最小合法 history/signature/repository fixture，组装真实 SpaceApplication，增加 restart、logical clock、manual rounds、snapshot 与 convergence budget。
Risk: fixture 若直接构造不可达内部状态，会形成第二套协议；所有 seed 必须经生产 constructor 和验证器。
```

```text
Step 4
Files: crates/uc-application/src/space/testing/scenarios.rs
Change: 按 F0-F7 建立协议等价矩阵；从各场景第一个需要网络传播的合法状态开始，不虚拟完整 admission。
Risk: 逐字复制 Engine 轮询会保留慢测试；断言必须改为稳定业务结果、授权矩阵和 frame/round 上限。
```

```text
Step 5
Files: crates/uc-infra/src/network/iroh/mod.rs, crates/uc-infra/src/network/iroh/membership_provider_contract_tests.rs
Change: 汇总或补齐真实 loopback contract：history codec/source、group ACK、branch recovery 两阶段和 partition close/reject/heal。
Risk: 只测成功 round trip 会漏掉认证来源和已有连接关闭，这些正是 virtual 无法覆盖的差异。
```

```text
Step 6
Files: crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs, scripts/testing/run-real-iroh-membership-topologies.sh, .github/workflows/membership-topology.yml
Change: F0-F7 标为明确 slow lane；脚本逐项串行运行；新增 scheduled/workflow_dispatch job。保留现有快速 admission/restart/content smoke 非 ignored。
Risk: `cargo test` 的 ignored 计数不能记为通过；release/nightly 记录必须绑定当前 commit。
```

```text
Step 7
Files: scripts/architecture/check-engine-repository.mjs, docs/architecture/architecture-bible.md, docs/exec-plans/active/034-deterministic-virtual-peer-network-test-suite.md
Change: 增加负向检查，禁止生产 `TransportProvider`、公开 Space test assembly 和 virtual provider 进入非 cfg(test)；同步稳定设计与实际验收证据。
Risk: 文本检查不能替代编译依赖检查；负向 fixture 必须证明规则可执行。
```

# 7. Edge Cases

```text
Scenario: 目标节点未注册或已停止。
Expected behavior: 调用映射为 Offline/Deferred/Unavailable，Application 保留欠账；不得 panic 或删除水位。
Implementation: route 在短锁内解析注册表并记录 Unavailable，不调用 endpoint。
```

```text
Scenario: 有向 link 只阻断一个方向。
Expected behavior: A→B 失败不代表 B→A 失败；ACK 和水位只按实际认证方向推进。
Implementation: link policy 以有序 `(source, target)` 为键，partition helper 显式写入双向规则。
```

```text
Scenario: 分区时存在已开始的 delivery。
Expected behavior: V1 只允许在 action 边界修改 link，已开始调用按开始时 snapshot 完成；不得宣称等价于 Iroh 关闭已有连接。
Implementation: topology 在 link mutation 前确认无 scenario action 正在执行；真实取消语义由 Iroh contract 验证。
```

```text
Scenario: endpoint 在处理请求时触发 maintenance wake。
Expected behavior: 当前 endpoint 完成后由后续 manual round 处理；不得递归启动第二个 maintenance owner。
Implementation: virtual tests 不启动 background runtime；topology 在 round 边界重新读取状态。
```

```text
Scenario: endpoint 又发起嵌套 transport 调用。
Expected behavior: 不死锁；嵌套调用也消耗 frame budget并记录因果顺序。
Implementation: network state lock 在 endpoint await 前释放，sequence 在每次 route 开始时分配。
```

```text
Scenario: 环拓扑形成无限协议扩散。
Expected behavior: 在 `max_frames` 内收敛；否则以 BudgetExceeded 失败并显示最后 trace，而不是等待 wall-clock timeout。
Implementation: 每次 route 原子消耗 frame budget，F5 另在稳定后执行额外 rounds 并断言无新增 conflict/effect 和有界 frame 增量。
```

```text
Scenario: 合法 peer 与 Diverged peer 同时存在。
Expected behavior: 公平游标最终服务合法 peer；冲突 peer 不消耗全部 round 预算。
Implementation: F7 按每个 peer 的首次成功 round 和 frame count 断言上界，不只检查最终成员数。
```

```text
Scenario: restart 时有持久 retry debt、conflict choice 或 branch recovery session。
Expected behavior: 新 Application 从同一 repository 继续，只向前推进；network 不替它记忆业务阶段。
Implementation: node stop/rebuild 保留 repositories，清空易失 endpoints 和 Application 实例。
```

```text
Scenario: seed 历史损坏、错 lineage 或签名无效。
Expected behavior: topology 构造失败，零节点注册、零 frame。
Implementation: `VirtualNodeSeed::validated` 强制生产 decode/verify；不提供 unchecked constructor。
```

```text
Scenario: 空 topology、重复节点键、重复 DeviceId 或未知节点 action。
Expected behavior: 构造或 action 立即返回稳定 fixture error，不进入 maintenance。
Implementation: 注册阶段验证唯一键和唯一业务身份；禁止 `unwrap`/`expect` 进入非测试生产代码。
```

```text
Scenario: frame 或 round 计数接近极限。
Expected behavior: checked arithmetic；溢出视为 BudgetExceeded，不回绕成成功。
Implementation: 使用 `checked_add`，默认预算远低于 `u64::MAX`。
```

```text
Scenario: trace 可能泄漏 payload 或身份。
Expected behavior: trace 只包含测试标签、协议枚举、序号和结果；Debug 快照无敏感字段。
Implementation: `VirtualFrameRecord` 不持有 payload，增加格式化和敏感 canary 负向测试。
```

```text
Scenario: 旧版本或 LAN compatibility。
Expected behavior: 034 不建立 fallback 或兼容 adapter；生产 Iroh 失败仍不自动切换 LAN。
Implementation: architecture check 保持既有 P2P/LAN 门禁，virtual provider 仅在 cfg(test) 可达。
```

# 8. Testing Strategy

## Unit Test

### Virtual network

- 输入：A/B 注册、双向 open link；操作：history request/response；预期：B endpoint 看到的 source 是 A 的注册
  `DeviceId`，trace 只有一条 Accepted 且无 payload。
- 输入：A→B blocked、B→A open；操作：双向调用；预期：仅 A→B 映射 Offline，方向不被合并。
- 输入：未知、停止和重复注册节点；操作：route/register；预期：稳定 fixture/delivery error，无 endpoint 调用。
- 输入：远端 endpoint 返回 reject/invalid；操作：分别经过四种 adapter；预期：严格符合错误映射表，带 source 的
  错误 `source()` 非空。
- 输入：`max_frames = 2`；操作：发送三次；预期：第三次 BudgetExceeded，计数不回绕，trace 长度受限。
- 输入：含内容、设备、地址和路径 canary 的 payload；操作：失败并格式化 trace；预期：任何 canary 均不存在。
- 输入：相同 seed/actions；操作：重复执行；预期：trace signature 和最终 snapshot 完全一致。

### Node and topology

- 输入：合法 seed；操作：构造、stop、restart；预期：业务身份和 repository 状态保留，Application/endpoint 实例更换。
- 输入：损坏 seed；操作：构造；预期：验证失败且 network 注册表为空。
- 输入：manual maintenance；操作：同一节点并发请求两轮；预期：沿生产 execution lock 串行，无双重 effects。
- 输入：逻辑 clock 未推进；操作：重复 periodic round；预期：未到期 retry 不被 wall-clock 唤醒。

## Integration Test

### Virtual F0-F7 matrix

| 编号 | Virtual 前置与动作 | 常规 CI 断言 | 仍由真实层证明 |
| --- | --- | --- | --- |
| F0 | 从共同 baseline seed 两个 Add sibling，heal | 两个 branch 等价类、无自动赢家、跨分支授权关闭 | admission、邀请、exact text |
| F1 | seed Remove/Add sibling，交换历史 | Removed/Active 精确、冲突唯一、无联合历史 | Sponsor/Joiner admission 与真实 group update wire |
| F2 | seed 两个 Remove sibling，明确选择目标 | 选择不可变、目标成员/epoch、恢复 session 只前进 | MLS external commit、control-generation 介质切换 |
| F3 | Accept/Reject sibling，restart chooser | conflict/choice/revision 跨 Application 重建保持、授权隔离 | SQLite AEAD、真实进程重启、exact text |
| F4 | 两个三节点分支只开放单 bridge | bridge 只传播冲突证据，不拼成六成员历史 | Iroh link/connection 行为 |
| F5 | 四节点环双向传播同一 conflict | 每节点一个 issue、effects 不重复、frame 数有界 | QUIC 多连接时序 |
| F6 | 深链中间节点 stop，叶子选择后 heal | 不依赖原 Sponsor，逐跳最终同 branch，retry debt 偿还 | 真实 endpoint 重启和 secure session 恢复 |
| F7 | 十节点三 sibling 不平衡树 | 合法 peer 在固定 round 上界内被服务，冲突 peer 不饥饿合法 peer | 实际 Iroh 调度和资源压力 |

每个场景至少断言：branch 等价类、effective members、pending conflict/effect、ledger revision 单调、授权 peer
矩阵、round/frame 上限。group epoch 只有在场景使用的 deterministic security adapter 实际应用 production group-update
port 后才可断言；fixture 直接赋值的 epoch 不得计为通过。

### Iroh provider contract

- History：summary/suffix/ACK round trip、wire version、最大/超限 frame、未知认证身份、source 绑定、remote reject。
- Group update：accepted/rejected ACK、零长/超限 payload、offline 和 timeout 分类。
- Branch recovery：GroupInfo 与 external commit 两阶段绑定、错误方向/recipient、畸形/超限 frame、认证 source。
- Partition：新连接在 before-connect 拒绝、已连接在更新 blocked 集合后关闭、握手竞态拒绝、heal 后可新建连接。
- Shutdown：Router/handler 关闭后调用有界结束，不遗留长期任务。

### Real persistence/security integration

- 继续运行真实 membership ledger CAS/AEAD、nonce、防重放和 branch transition phase fault replay。
- F3 的 Application restart 不替代 SQLite restart；两者必须分别登记。
- 测试输出执行明文 canary 探针，禁止业务 payload 和身份进入数据库、日志或 trace。

## Regression Test

- 常规门禁运行 virtual F0-F7、现有 Core/Application F8-F13、真实 Infra contract 和非 ignored Engine smoke。
- F0-F7 原真实 Engine/Iroh 测试保留，slow-lane runner 逐项串行执行；失败不得用 virtual 通过覆盖。
- 至少保留这些非 ignored 真实 smoke：fresh join、完成准入后重启并 exact text、history adapter round trip、branch
  recovery round trip、partition reject/close/heal、group update ACK。
- P2P 失败不自动回退 LAN；三端绑定和 `uc-engine` 稳定 operation 不因 test provider 改变。
- 完整实现至少运行：

```bash
cargo metadata --locked --format-version 1
cargo test -p uc-application --locked
cargo test -p uc-infra --locked
cargo test -p uc-engine --all-targets --locked
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

- slow lane 使用仓库脚本运行。未连接实体设备或未生成 Release bundle 时明确记录为“跳过”。

# 9. Acceptance Criteria

* [ ] `VirtualMembershipTopology` 是 virtual F0-F7 节点、逻辑时间、网络控制、预算与 trace 的唯一完整负责人。
* [ ] 生产代码没有新增通用 `TransportProvider`，现有领域 ports 和 Engine 公开 contract 不变。
* [ ] virtual provider 仅在 `uc-application` 的 `cfg(test)` 构建可达，不进入 `test-support` 公开 feature。
* [ ] virtual history、restricted delivery、group update 和 branch recovery adapters 均调用远端真实 Application endpoint/capability。
* [ ] source identity 只能来自注册 adapter；普通 topology action 无法伪造 `DeviceId`。
* [ ] 网络锁不跨 endpoint `await`，嵌套调用和并发 maintenance 不死锁。
* [ ] 节点启动后，场景不能直接改写 ledger；所有 seed 经生产 constructor、codec 和 signature verification。
* [ ] virtual 场景不启动真实 periodic runtime，不使用 wall-clock `sleep` 等待收敛。
* [ ] F0-F7 virtual 矩阵全部通过，并具有明确 round/frame 上限、授权矩阵和脱敏失败 trace。
* [ ] 同一 F0-F7 场景重复执行产生相同 snapshot 与 trace signature。
* [ ] virtual F0-F7 在 macOS-14 PR runner 上总耗时不超过 30 秒，单场景无分钟级 timeout。
* [ ] F5 能以 frame/effect 上限证明无循环，F7 能以首次成功 round 上限证明合法 peer 不被冲突 peer 饿死。
* [ ] Application restart、真实 SQLite restart 和真实 control-generation fault replay 分别通过并分别登记。
* [ ] Iroh provider contract 覆盖认证 source、codec/frame、ACK、两阶段 recovery、partition close/reject/heal 和 shutdown。
* [ ] 非 ignored Engine smoke 覆盖真实 admission、重启、exact content、group update 和 branch recovery。
* [ ] F0-F7 原真实 Engine/Iroh 测试仍存在，并可由 slow-lane 脚本逐项串行执行。
* [ ] nightly/release slow-lane 结果绑定当前提交；未执行项明确记为“跳过”，不沿用旧证据。
* [ ] trace、错误、日志和 CI artifact 不含设备身份、地址、邀请、branch/head、凭据、密钥、路径或业务内容。
* [ ] 架构检查阻止 virtual provider 进入生产依赖、公开 Space 内部 assembly 或恢复 Engine 级万能 provider。
* [ ] workspace check、Application/Infra/Engine tests、fmt、architecture 和 diff gates 全部通过。
* [ ] 实施结论同步到 `docs/architecture/architecture-bible.md`；完成后本计划移入 `completed/`。

# 10. Risks and Trade-offs

- **virtual 不等于 Iroh**：typed 调用绕过 ALPN、codec、QUIC 和连接生命周期。通过 Iroh provider contract、Engine
  smoke 与保留的 slow lane 明确补齐，而不是提高 virtual 仿真复杂度。
- **seed 降低 setup fidelity**：F0/F1 不再每次经过完整 invitation/admission。收益是把拓扑协议测试从准入成本中
  分离；真实 admission 与 fork 形成仍由 Engine smoke/slow lane 证明。
- **test-only manual driver 接近内部 seam**：它能稳定执行生产 maintenance owner，但不得导出到外部 crate 或让
  scenario 调用内部步骤。interface 只允许“一轮指定 trigger”，不暴露子 use case。
- **in-memory restart 可能给出过强信心**：它只能证明 Application 从保存状态恢复，不能证明 SQLite 事务、AEAD、
  fsync 或 manifest promotion；验收矩阵必须把这些证据分开。
- **两套场景会增加维护成本**：virtual 与 real slow lane 关注不同证据，不能逐行复制。F0-F7 的业务期望以本规格
  矩阵为入口，real 测试只保留必须跨 Infra/Engine 的断言。
- **未模拟概率网络**：确定性 link failure 更适合回归和复现，但不能发现所有真实调度问题。固定 Iroh slow lane
  继续承担集成风险；随机压力只能作为附加证据，不能替代固定矩阵。
- **30 秒目标依赖 CI**：不得把机器时间作为唯一正确性断言；round/frame budget 才是稳定契约，wall-clock 只作
  工程反馈目标。

替代方案一是在 Engine 增加完整 network provider factory，使所有 E2E 可切换 Iroh/virtual。该方案需要虚拟
IrohNode builder、Router、admission、clipboard、blob 和生命周期，interface 几乎等于实现清单，并破坏 031 已收口的
Application 私有装配，因此不采用。

替代方案二是只保留各 use case 的独立 fake。它不能表达多节点、有向分区、逐跳传播、frame budget 和公平性，
F5/F7 的复杂度会继续散落在测试调用方，因此不采用。

替代方案三是编写纯模型模拟器。它运行最快，但会复制 membership 状态机并可能与生产规则共同错误，不能作为
实现回归证据，因此不采用。

# 11. Open Questions

唯一未决项不阻止 virtual suite、Iroh contract 和 nightly slow lane 实施：

1. Release 是否硬性依赖同一提交的 slow-lane success 尚未在现有 release workflow 中定义。未决定前，发布记录必须
   明确写“通过”或“跳过”，不得自动继承 nightly 状态。
