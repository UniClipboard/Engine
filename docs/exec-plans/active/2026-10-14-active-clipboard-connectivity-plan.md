# Issue #34 第五切片：当前剪贴板状态与按需拉取执行计划

状态：**本地实现与验收完成，待线上当前 head 检查**（2026-10-14）。
唯一设计来源：[实现规格](2026-10-14-active-clipboard-connectivity-spec.md)。
前置切片：[PR #180](https://github.com/UniClipboard/Engine/pull/180) 已合并；审计基线为
`main 2f9afc9956b0cda82e65c912f0c0f6dc6aa51256`。本地实现与功能验收已完成；线上最终 head 未通过前不归档。

## 完整负责人

- **业务负责人**：Application `clipboard/active` 及既有 `sync/active_state`；决定状态传播、完整拉取、应用与取消边界。
- **调用方唯一动作**：沿用 dispatch、subscribe、pull；入站 bridge 各调用一次完整 receiver。准备方只接受一次完整 prepare，不由 Engine 拼接步骤。
- **成功**：通知仅完成发送；pull 取得完整加密封套；实际 register 与系统应用由原写入 owner 判定，三者分别结算。
- **失败/取消**：当前网络交换结束，来源保留；已经开始且不能安全中断的本地动作由原 owner 结算并占用额度。
- **恢复**：不新增重试或持久补偿。后续激活/上线按原规则触发；重启不恢复内存待发槽，已有文件/存储恢复归原负责人。
- **资源负责人**：Uni 管理独立有限连接与请求；Infra 提供实际传输和 generation 工作执行；Engine 只装配完整能力与既有观测。

## 已确认决策映射

| grilling 决定 | 唯一细则位置 |
| --- | --- |
| 仅迁移连接层，保留恢复语义 | spec §2–3 |
| 出站通知、pull 与入站交换各 10 秒网络总预算 | spec §5 Workflow |
| 每轮最多 4 目标并行 | spec §5 最新待发状态与广播轮次 |
| 当前轮完成，待发只保留最新；入站业务串行 | spec §5 Data Model |
| 取消后只结算已开始的不可安全中断工作 | spec §5 取消、本地工作与关闭 |
| 状态、拉取独立容量；不扩大连接共享 | spec §5 初始容量、协议与连接 |
| 封套迁移，文件本体沿用旧链路 | spec §3、§8 |
| 固定旧 main 双向，直连/Relay，本地 CI 先于推送 | spec §8–9 |

## 阶段 0：冻结事实与建立失败信号

* [x] 从最新 main 建立实现分支，记录精确 SHA，核对与审计基线的相关差异。
* [x] 读最近的 AGENTS 与 Rust/错误/安全/观测规范；确认没有并行 Cargo 占用共享 target。
* [x] 清点四个原生方向、内部 serve port 全部消费者、恢复触发器、接收队列及 generation/任务 owner。
* [x] 用既有 codec 提取修改前 golden 向量与错误样本，保留原 ALPN 和状态码。
* [x] 先运行原正常文本恢复与按需拉取基线，再构造可重复失败：无帧无限等待、慢目标串行阻塞、当前轮发送期间积压恢复通知。
* [x] 验证共享准备的真实两文件取消边界；分别记录“已开始 A”“未开始 B”“网络结束”“A 实际结算”“额度归还”。
* [x] 解决固定旧 Engine 的测试驱动入口，先证明旧旧正常流程可执行再修改生产代码。

旧版本驱动方案：扩展/复用 connectivity host 的稳定 Engine 操作驱动，为 RestoreClipboard、状态观察与最终内容读回补测试命令。
同一驱动以隔离的测试 manifest 分别链接冻结旧 Engine 源码和当前 Engine；旧 Engine 库源码不修改。
驱动源码摘要、Engine rev、构建特性、二进制 SHA256 分别记录，不称其为“原封不动的旧发布宿主”。
临时旧源码放明确可再生缓存目录，共享仓库 target 串行构建、保留编译缓存，脚本退出回收临时源码并恢复当前宿主。
不得直接改写已有固定成员历史脚本的旧 rev，以免失去第四片的兼容门禁。

退出条件：有明确 red-capable 命令、原行为基线、固定旧库双端可执行宿主及未运行矩阵清单。

## 阶段 1：状态完整收发

* [x] 增加两个协议 profile/能力项，先接通状态的出站、入站、原队列及原串行用例。
* [x] wire 只适配已有 codec；新增 Application transport 按职责拆文件，不扩 `mod.rs` 成流程大文件。
* [x] 使用原身份目录/撤销登记；请求快照先于排队，交接前重检 grant。
* [x] 状态 10 秒 deadline 覆盖全路径，保留有界 FIN/关闭保护；不把关闭或发送成功写成远端应用确认。
* [x] 通过慢读、半帧、过载、无订阅者、无权限、移除再添加和旧新状态双向测试。

## 阶段 2：拉取与取消收尾

* [x] 第二个 manager 独立配额；迁移 requester 与 holder 全路径，保持原响应状态及 16 MiB 上限。
* [x] 将内部 serve 能力迁为 Application 的完整准备 port，同步迁移所有装配与测试，删除旧 Core serve 接口。
* [x] 给共享准备唯一实现接入停止检查，覆盖每次 blob 发布之间及编码/加密之前；不复制准备流程。
* [x] 已开始本地动作持有原 StreamUse 和 generation 许可，排入阻塞池但未执行时仍可停止。
* [x] 超时、对端关闭、撤销、调用取消和 shutdown 均能停止网络，不继续下一准备步骤；真实任务失败可见。
* [x] 验证拉取占满时状态可收、连续取消不能绕过限额、旧 generation 不污染新会话。

## 阶段 3：广播并发与最新值槽

* [x] 将 fanout 与上线补发接到同一每轮 4 目标的有界执行实现，保留各自目标与 gate 语义。
* [x] 无界恢复通知改为单待发槽，原子保存状态/类别/版本/时刻；正在发送一轮不被新状态抢占。
* [x] 覆盖新旧乱序回调、重复值、发送完成与新值同时到达、连续更新以及开关变更。
* [x] 300/1500 毫秒静默防抖沿用；不新增入站并发、重试或新状态抢占旧 pull。

## 阶段 4：生产接线与删除旧路径

* [x] Engine 只装配完整对象；Router 接入全部原 ALPN，两类 manager 与现有 generation 生命周期共同退出。
* [x] 清除原四个 adapter 的直接 Endpoint/Connection 使用及旧构造入口；不让测试接新路径而生产留旧路径。
* [x] source chain、固定分类诊断及业务结束边界符合规范；不为观测增大 facade/port 查询面。
* [x] 加入架构回归，限制当前状态/pull 生产流程回退到原生连接；原生类型只留在 bridge/backend 和明确测试中。

## 阶段 5：完整场景、兼容及 CI

新增测试及脚本名称在实现时采用以下计划入口，当前文件尚不存在，不应当作已经运行的命令：

- `tests/hosts/connectivity/tests/active_clipboard_connectivity.rs`：独立进程双端，复用现有 host、网络故障控制与合成内容。
- `scripts/testing/run-active-clipboard-connectivity.sh`：模式 `direct`/`relay`；只选择本片测试，不另建网络模拟框架。
- `scripts/testing/run-fixed-main-active-clipboard-interop.sh`：固定旧库+统一测试驱动，显式双向角色，输出构建 manifest 和摘要。

* [x] 文本、图片、单文件、目录四类分别走“恢复→通知→缺失时 pull→最终读回”；通过设置/场景控制避免普通推送提前填充内容掩盖 pull。
* [x] 已有内容用例断言没有冗余 pull；拉取失败/锁定/损坏/无权限断言 register 与转播不推进。
* [x] 直连与 Relay 独立进程测试记录 selected path、实际 UDP 丢弃证据、IPv4/IPv6 限制、长度/哈希/目录成员读回。
* [x] 固定旧端同时覆盖状态双方方向、pull 双方角色及无内容响应；不得以新新测试替代旧新。
* [x] 将新直连/Relay 与固定旧端检查分别加入 PR 必需 tests/upgrade job，保存独立 JUnit 和工件，不只放 nightly 或 ignored 测试。
* [x] 原工作区、evidence、成员 smoke、隔离直连、成员历史 Relay 与固定旧历史互通继续执行，保留既有升级 smoke。
* [x] 本地所有对应 CI 门禁通过再推送；失败先定类、复现和修复，不以提高重试数、放宽超时或反复重跑直到绿色代替诊断。

### 本地门禁顺序

Cargo 由一个负责人串行执行，复用根 target 与 MBX。`check-engine-repository.mjs`、日志宏门禁也可能调用 Cargo，
不能与其他 Cargo 并跑。Linux 与 CI 保持 `UC_EXPECT_DIAGNOSTIC_STACK=available`，分组复用构建时设置
`UC_TEST_BUILD_SCOPE=workspace`；cargo-nextest 与 cargo-audit 按工作流固定版本准备到 PATH。

1. `just cargo metadata --locked --format-version 1`
2. 各阶段 red/green 与相关 Application、Infra、codec、生命周期测试。
3. `bash scripts/testing/run-test-group.sh workspace`、`evidence`、CI 当前成员 smoke 过滤器。
4. 新增 active-clipboard 直连/Relay/旧新脚本；现有 `run-membership-history-relay.sh`、`run-fixed-main-history-interop.sh`。
5. `bash scripts/testing/run-test-group.sh upgrade-matrix --smoke`；现有隔离网络 direct E02，必要的完整 Relay 回归。
6. `just cargo check --workspace --all-targets --locked`、`just cargo fmt --all -- --check`。
7. Rust 风格、仓库架构、直接日志宏检查；`just cargo audit --deny unsound`；工作流列出的 JavaScript 测试。
8. Markdown 相对链接、索引、`git diff --check`；确认源码没有临时诊断或未归属的后台任务。

每组立刻另存自己的 JUnit，避免后续组覆盖。网络隔离执行使用仓库支持方式；缺失权限是阻塞或跳过证据，不能算通过。
线上 macOS/Windows 依工作流独立验收，Linux 本地通过不代替它们。覆盖率和物理设备未跑明确记跳过。

## 验收矩阵与工件

工件根目录拟为 `target/active-clipboard-connectivity-evidence/`；只包含合成 fixture 和脱敏结果。

| 项目 | 必需证据 | 当前状态 |
| --- | --- | --- |
| 原行为与 red 信号 | 版本、命令、原失败及原因 | 本地已执行，见下表 |
| wire 与单次协议 | golden、上限、旧字节及关闭顺序 | 本地已执行，见下表 |
| deadline/并发/最新值槽 | 逻辑时间与屏障，最大活动/待发数 | 本地已执行，见下表 |
| 撤销、关闭及本地结算 | 两文件真实依赖交错、许可/额度释放时刻 | 本地已执行，见下表 |
| 完整内容 | 四类内容实际读回与哈希、目录成员 | 本地已执行，见下表 |
| 直连与 Relay | 独立进程、网络约束、实际路径 | 本地已执行，见下表 |
| 固定旧新互通 | 两个库版本、驱动源码、二进制 SHA、双角色结果 | 本地已执行，见下表 |
| 本地 CI / 线上当前 head | 分组报告与最终检查链接 | 本地已执行，见下表 |
| 更早历史完整矩阵、物理设备 | 未执行原因及范围 | 本片不扩展；未运行时跳过 |

## 阶段 6：交付与归档

* [ ] 用 spec §9 逐项核对，不以 PR 合并或单条测试通过代替完整验收。
* [ ] 更新[剪贴板连接长期设计](../../design-docs/clipboard-connectivity.md)中本片已实现事实，明确状态无 ACK、封套/文件边界与有限资源。
* [ ] 按实际变更更新 Application/观测/安全主题，仅链接既有标准，不复制新的规范正文。
* [ ] 记录最终源码、构建摘要、本地结果与线上当前 head 结果；保留首次失败证据与范围外问题。
* [ ] 本 plan/spec 移入 completed，更新所有入口；Issue #34 保持剩余范围，不扩大本片完成声明。

## 设计文档检查记录

2026-10-14：已核对当前 main 与审计基线一致；实现及上述功能验收均未执行。
本文档自身检查已通过：Cargo metadata、工作区全部 target 的 cargo check、fmt、Rust 风格与仓库架构门禁、
四份变更文档的 49 个相对链接与双索引入口、spec 的 11 节结构，以及 `git diff --check`。
工件目录为 `target/active-clipboard-connectivity-spec-evidence/`，不得与实现验收工件混用。

## 实现与本地验收记录

- 实现分支 `feat/active-clipboard-connectivity` 从冻结 main `2f9afc9956b0cda82e65c912f0c0f6dc6aa51256` 开始；旧 Engine archive SHA256 为 `44d65418195c835f5ab8126a053000d10509332f918aefd50083ac014d283912`。
- 四向完整 Application transport、独立 manager、单次交换及 10 秒绝对预算已接生产。旧 Core serve 与最后内容 gate、Engine 转接器、原 native dispatcher 删除；client 原文件仅保留合同测试。
- 真实两文件 A 已导入、B 未开始时网络取消；A 完整结算，原许可释放前 holder 世代暂停不能完成。16 份本地工作保留额度时状态仍独立接收；阻塞池未开始的工作停止，已开始的真实失败在等待者取消后仍可见。
- 本地准备共用原 owner；广播目标 pool 为 4，恢复槽为单当前与单最新待发，LWW/类别/版本/到达时刻/剩余静默窗口回归通过；上线 1500ms 目标集合语义保留。
- 原相对顺序调整：真实两文件屏障在准备迁移后运行，晚于独立状态切片；未将其预记为修改生产前通过。冻结旧旧文本、golden 与各切片 red 信号仍先于对应生产修改。

| 门禁 | 实际结果 | 证据 |
| --- | --- | --- |
| workspace 最终全套 | 4234/4234 通过，25 配置跳过；561.136 秒 | `workspace-suite-final.log`、`workspace-final-junit.xml` |
| 合法半帧补验 | 1/1 通过，20.088 秒；不开流及有效 magic 后缺长度各 10 秒 | `valid-partial-frame.log`、同名 JUnit |
| evidence | 18/18 通过，success/failure 示例及结构化结果齐备 | `evidence-suite.log`、`evidence-junit.xml` |
| PR 成员 smoke | 16/16 通过，含长分区、暂停/重启与移除重加入 | `membership-smoke.log`、同名 JUnit |
| 原 Relay 历史 | 当前预建二进制 1/1 通过，UDP 阻断计数通过 | `history-relay.log`；`target/membership-history-relay/` |
| 原固定历史互通 | edf0a296 旧 rev 未改，V1/V2 两向 2/2 通过 | `fixed-history-interop.log`、`fixed-history-junit.xml` |
| 升级 smoke | 5/5 预期契约通过；pass 3 / rejected 1 / known-incompatible 1，其余 63 未执行 | `upgrade-smoke.log`、`upgrade-smoke-junit.xml` |
| 固定旧 main 本片互通 | 12 子场景矩阵通过 61.829 秒；四类两向及已有内容、无内容拒绝、稳定锁定 | `fixed-main-interop-final.log`、`fixed-main/` 来源/驱动/profile/binary 摘要与 JUnit |
| 本片最终隔离直连 | 四类 4/4，实际系统完整内容与 Direct 路径 | `direct-gate-final/` |
| 本片最终隔离 Relay | 四类 4/4，实际系统完整内容、Relay 路径及双栈 UDP 实际阻断 | `relay-gate-final/` |
| E02 原隔离直连 | 文本、文件双向传输通过 | `e02-direct.log`；`target/connection-recovery-evidence/direct.json` |
| 根检查 | locked metadata/check、fmt、Rust style、仓库架构/隐私/负例、diff 均通过 | `metadata-final.json`、`check-final.log`、`preflight-final.log` 等 |
| 直接日志宏 | default/LAN 两轮 0 违规 | `direct-log-macros-final.log` |
| 依赖审计 | CI 固定 cargo-audit 0.22.2，deny unsound 通过；8 条策略允许警告 | `audit-final-2.log` |
| iOS/Android/HarmonyOS 物理设备及更早完整历史矩阵 | 跳过：本片本机无设备验收；升级只按 PR smoke 范围执行 | 未记通过 |

未另注明的实现证据均在 `target/active-clipboard-connectivity-evidence/`，与原 spec 设计检查证据分开。
`code-review` 技能未安装；人工等价审查记录见[本任务发现与审查](../../../.planning/2026-10-14-active-clipboard-connectivity/findings.md)。

首次失败证据保留：原正常基线、无流无限等待/串行慢目标/旧回调 red、共享 target 旧快照 fresh 误判、观察登记 Busy 丢初始路径，以及有限日志输出丢开始记录。最终终态自带原 purpose，完整接收通过既有不透明物理上下文关联本机实际 channel；不放宽队列或用跨端时间/哈希猜关联。整条日志记录的丢弃仍由原统计诚实报告。

目录首次完整读回及旧新两角色通过。目录再次激活的既有可用性/重复存储失败单列[技术债](../tech-debt-tracker.md)；已有内容不重复拉取由文本、图片和单文件验证，不预记目录二次行为通过。稳定锁定会退休接收世代；此场景验证不写入，Locked 字节另由原 codec 与真实 handler 合同验证。

线上最终 head：待推送并完成 required checks，之后移入 completed、更新索引；不关闭整个 Issue #34。
