# Issue #34 第四切片：成员历史完整迁移与连接复用执行计划

Status: completed. [PR #180](https://github.com/UniClipboard/Engine/pull/180) merged as `2f9afc9956b0cda82e65c912f0c0f6dc6aa51256` on 2026-10-13. Its final head was `b30011bcf3319560b96bd96f09ca4bfb74e3eafe`. Engine repository checks, Engine tests, upgrade smoke, and Windows x64 and arm64 builds passed on that head. Coverage and connection-recovery jobs were skipped. Physical-device acceptance was not run. The earlier local failures remain recorded below.
基线：`origin/main edf0a296`；第三切片 `247490af` 已合并，Issue #34 保持开放。
唯一设计来源：[实现规格](2026-10-12-membership-history-connectivity-spec.md)。本文记录执行顺序、退出条件和证据，
不建立第二套协议或授权规则。

## 责任与结果

- 完整业务负责人：Application Space 成员模块；HistorySynchronizer、入站历史用例、受限投递与成员 owner 共同归该模块负责。
- 调用方唯一动作：提交一次完整历史 exchange，或向完整历史端点交入站连接使用权；不在 Engine 拼接分页和提交步骤。
- 成功：原历史/受限业务验证、持久提交和对应真实 ACK；历史确认不替代组更新安装成功。
- 失败：既有 Deferred、Rejected、PairingInProgress 与带 source chain 的本地/传输错误；未确认仍有持久债务。
- 重启/重试：原成员 owner 从加密记录恢复，worker 按 due_at 调度；Uni 只重获尚未发送请求的坏连接，不业务重送。
- 连接生命周期负责人：历史独立 Uni manager，负责容量、跨轮持有、取消、回收及 shutdown；generation/local-work 复用原会话交接。

## 阶段 0：固定旧事实与内部接口

状态：已完成实现，证据见实施记录。

1. 枚举全部八种消息的真实发送、响应、受限投递、地址刷新、Node 路由与 Engine 装配入口，记录源码位置。
2. 从基线原适配器保存不可变字节向量：请求封套/trace、状态/body/FIN、八种消息、长度限制与错误。
3. 固定 DirectoryVerified、IdentityUnresolved 介绍、其他目录失败、移除/受限、冲突及 outbound
   期望 TLS 身份的真实矩阵。不能只以普通 active member 测试作全部授权证据。
4. 冻结 pending/bind/retain 内部类型及文件职责、撤销快照取得点、资格使用权失效点和现有 local-work 复用方式。
   对首帧介绍不得伪造已授权 grant；测试未知 candidate 不进入权威身份登记。
5. 固定旧宿主为未修改 `edf0a296`，登记完整 SHA、构建来源及二进制摘要。旧向量也来源于该版本。

退出条件：旧行为可复现；规格第 5 节身份绑定/授权合同已有证据，内部接口能表达特殊恢复及 generation 关闭。
如果缺少安全绑定材料，保存具体失败案例并更新受影响设计；不得边实现边默认信任自报 DeviceId。

## 阶段 1：唯一 wire codec 与兼容能力

状态：已完成实现；验证见阶段 5。

1. 机械提取 codec 至 uc-sync-protocol，保留 V4 消息体和原判别值；原 adapter 暂时只委托唯一 codec。
2. 增加历史新旧 ALPN 常量、profile 和实际 ConnectionProtocol/Mode；通过真实协商选择多流或单次。
3. 用旧向量、旧端响应及非法长度/截断/版本测试验证。先证明 wire 兼容，再接生产 owner。

退出条件：固定字节向量全部一致；新新 `/5` 与旧新双方向 `/4` 能由实际 ALPN 证明，不靠版本猜测。

## 阶段 2：Uni 有界生命周期

状态：已完成实现；验证见阶段 5。

1. 实现有界首帧 pending lease 和明确临时 scope；全部网络资源由 Uni 持有，unknown 身份不创建无界 peer 表。
2. 历史独立 manager 启用规格的强引用保留策略，其他协议继续原策略。
3. 实现双方向独立、同方向唯一、活动/等待计数、空闲代次、30 秒定时器与合计 16 条上限。
4. 实现 per-stream 取消和 deadline、过载结算、保留资格撤销、generation/关闭与 tasks drain。
5. 用受控 clock/barrier 验证 timer 与请求、撤销与 retain、取消与 permit 释放竞态，不靠真实 sleep 判断逻辑。

退出条件：容量和引用有硬上限；一流取消不伤另一流；全部等待/任务可关闭；现有 Clipboard/GroupUpdate 回归通过。

## 阶段 3：完整历史 owner 接入

状态：已完成实现；验证见阶段 5。

1. 在成员模块私有 history_transport 下按流程/port/错误/模型/测试拆分，mod.rs 保持入口。
2. 完整迁移 exchange、receive/respond 和 restricted delivery；保留原完整 Core port 和地址刷新职责。
3. 每消息复用原授权和可信提交；介绍身份只可临时交换，资格使用权来自当前权威，不能缓存首条授权。
4. 保持 page→ACK→next page、peer 锁、单来源 transfer 和 CAS；deadline 不延长一轮 10 秒预算。
5. 绑定既有 Space/local-work permit，网络取消不丢弃已开始不可取消工作。
6. 首写后所有失败由 owner Deferred/Rejected 结算；真实 ledger 退避与重启债务保持。

退出条件：常规、冲突证据、移除/受限及介绍身份路径都有真实输入/输出测试，原持久化格式不变。

## 阶段 4：生产接线与删除被取代路径

状态：已完成实现；验证见阶段 5。

1. Node 注册 `/5`、`/4` 并纳入 generation permit/quiesce；handler 仅包装 Infra transport，完整端点在 Application。
2. Engine 组装独立历史 manager、完整 port decorator、成员 owner；不增加 stable API、阶段查询或测试 facade。
3. 原 adapter 删除已迁移的 native 拨号、codec 与业务处理；身份解析/地址能力迁到对应 Infra 私有文件。
4. 搜索全部入口确认 restricted delivery 和 verified address refresh 未遗漏，取消原无界 connection.closed 等待。
5. 验证 shutdown/Space 切换/移除关闭和失败 source chain；池诊断不制造新的业务 root。

退出条件：只有一套生产历史流程/codec；完整 Engine 实际路径与测试接线一致，其他协议仍隔离。

## 阶段 5：独立进程与故障证据

状态：本地完成；最终完整门禁及首次失败证据见实施记录。

按照规格第 8 节运行真实 SQLite/MLS、实际网络的最小必跑矩阵：

| 必跑组 | 退出证据 |
| --- | --- |
| 新新多页、跨轮复用 | 实际 `/5`、同方向连接序号不变、逐页真实 ACK；下一页不越过上一页 |
| 双向与独立请求 | 双方向各一条，独立请求能并行，一流故障另一流成功 |
| 30 秒/上限/撤销/关闭 | 定时器与资格竞态确定性测试；真实进程结束连接和任务归零 |
| 身份介绍/冲突/移除/受限决定 | 原业务授权和签名结果一致，非当前成员不跨轮保留 |
| ACK 丢失与强杀重启 | 保存页后/提交后 ACK 前故障，重开 SQLite 恢复债务并真实确认 |
| direct、relay-only | 两组分别验证实际协商/连接复用，网络模式证据独立 |
| 新→旧、旧→新 | 未修改旧宿主摘要、实际 `/4`，包含摘要/后缀/冲突/受限路径 |
| 第三/第二切片回归 | 真实组更新安装、明确拒绝恢复、Clipboard 多流/取消/重启通过 |

复用现有独立进程 harness/testkit；新增用例并入强制 PR CI，不依赖手动 workflow 才能发现回归。
每项保存具体命令、提交、退出码、工件路径及实际结果；旧端构建通过不算旧新交换通过。
测试探针只在测试宿主，证据脱敏，不扩大生产接口。

退出条件：上述必跑组通过；首次全套失败有保留证据及解释，不能用隔离通过替代。
物理桌面/iOS/Android/HarmonyOS 和网络切换/休眠/前后台矩阵按实际运行单列，未跑记“跳过”。

## 阶段 6：本地完整门禁、线上检查与归档

Status: completed for the checks listed in the status above. Skipped jobs and physical devices remain unverified.

### 本地先行

按最终提交与 PR base 对齐 `.github/workflows/pr-check.yml`，先在本地完成可运行的完整 CI 等价集，
再推送让线上运行；修改后重新验证受影响检查，不能拿早期提交的结果作最终通过。
PR-base Rust 风格/日志检查覆盖整个 PR，不能只检查最后一轮 working diff。

所有 Cargo 从仓库根目录经 `just cargo`，共用仓库 target 与现有 wrapper，单一负责人串行运行。
旧宿主优先使用已固定、可验证来源的工件；确需旧源码构建，使用明确的可再生目录并按仓库要求
串行管理编译和清理，不在 `/tmp` 创建另一套完整 target。

最低根门禁：

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
node scripts/architecture/check-direct-log-macros.mjs
just cargo audit --deny unsound
node --test scripts/architecture/check-mobile-build-profile.test.mjs scripts/release/build-profile.test.mjs scripts/architecture/check-direct-log-macros.test.mjs
just cargo test -p uc-infra-profile --test peer_address_read_diagnostics --locked
bash scripts/testing/run-test-group.sh workspace
bash scripts/testing/run-test-group.sh evidence
bash scripts/testing/run-test-group.sh membership-e2e
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode direct --repeat 1 --case E02 --prebuilt
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode relay --repeat 1 --prebuilt
bash scripts/testing/run-test-group.sh upgrade-matrix --smoke
git diff --check
```

命令参数、测试选择和工具版本以实施时锁定 workflow/test-group 为准：本计划的组名只是执行入口，
必须核对没有遗漏 workflow 必跑项。先检查分组是否覆盖新增历史用例；未覆盖就修改强制分组与 CI。
覆盖率或平台专属检查若成为 PR 必需门禁，也纳入最终矩阵；本机不支持的平台明确交给线上对应 runner，
不得声称本地跨平台全部通过。此要求不以自动推送/发布作为本计划授权。

### 线上与文档

同一最终提交线上必需检查全部通过后才能声明代码交付。将确定的实现事实回写[剪贴板连接边界](../../design-docs/clipboard-connectivity.md)、
[成员历史所有权](../../design-docs/membership-history-ownership.md)及实际涉及的会话交接设计文档/ADR，更新技术债与 Issue #34 范围；本切片不关闭整个议题。
完成后 spec/plan 移入 completed，修复 PLANS、active/completed 索引及相对链接。

退出条件：必跑本地与线上矩阵均有结果，跳过平台独立列出，文档不夸大设备验证。

## 业务记录验收

完整动作是上线/重启/重试触发的成员一致性恢复，或原移除/决定动作中的历史投递。
负责人保持成员 owner；当前在线执行 Deferred 即结束，后续重试按原调度开始新尝试。
正常空闲回收/开流不成为独立业务动作；同一连接各流的上下文不串联。
按[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)验证原完整能力的
触发、结果、未完成债务和后续责任，使用固定分类，不记录身份、地址、文件、内容或原始错误正文。
真实测试输出与产品数据隔离；设备/可视化未执行项单列跳过，不为观测扩大接口。

## 设计阶段检查记录（实施前）

设计阶段新增 spec/plan 并接入导航；当时生产代码、新协议互通、连接复用及设备矩阵均**未执行**。
文档根门禁结果在完成后写入下表；实施时另追加对应阶段证据，不能继承文档检查作行为验证。

| 检查 | 结果 |
| --- | --- |
| 规格结构、源码路径与相对链接 | 通过；11 节结构完整，新增导航及全部相对 Markdown 目标存在 |
| metadata/check/fmt | 通过；2026-10-13，workspace all-targets check 49.26 秒，保留原有 unused_imports warnings |
| Rust 风格、仓库架构、diff whitespace | 通过；仓库 preflight 含隐私、Collector、OpenMLS 与负例检查 |
| 新传输行为、本地门禁 | 本地完成；实际结果见下方记录；线上及设备单列跳过 |

文档检查工件保存在 `target/membership-history-connectivity-spec-evidence/`（忽略目录）；
设计文档检查时基于 `edf0a296`，当时尚未提交或推送；原 `.planning/` 工作记录保留。


## 实施记录（2026-10-13）

基线与最新远端 main 均为 `edf0a296118c071a97d4c14ae27b09a61931bbae`，工作分支为
`feat/membership-history-connectivity`。完整八种消息迁移到成员模块私有 transport，唯一 codec 保持 V4；
历史独立 Uni manager 负责容量和生命周期，当前成员记录仍为 V6，稳定 Engine 和绑定接口未变化。
长期实现合同回写[成员历史所有权](../../design-docs/membership-history-ownership.md#成员历史连接边界)。

实际 QUIC 协商要求服务端优先注册 `/5`。两端资格不同或池满时 `/5` 用 AcceptedClosing 告知发送端
不能保留；`/4` 字节和状态保持原样。审查补齐首帧撤销围栏、首写前预算核对、取消安全 accept 缓存、
入站首个 received_at 截止点、过期队列独立清理，以及 local-work 结算前保留交换额度；
不增加新的任务登记，也不透明重送已发送消息。

强杀验收曾暴露旧 QUIC 被反复复用，以及宿主只固定 IPv4、重启后 IPv6 广告端口变化：
前者在完整轮次超预算时淘汰复用资格，后者仅修正测试配置，两族保持同一固定端口并断言广告地址稳定。
真实进程组已通过 6/6；强杀、第三方缺口恢复和原组更新 ACK 清债均实际执行。

锁文件中的基线 quinn-proto 0.11.15 命中五项 RustSec 阻断公告，已更新到 0.11.18；
`audit --deny unsound` 通过，没有添加忽略公告。旧版构建与当前构建串行复用根 target 和 MBX，
脚本备份并恢复同名当前宿主，固定旧源码、二进制摘要和测试结果分别留存。

### 本地验证证据

工件根目录：`target/membership-history-connectivity-spec-evidence/`；该目录被忽略，
`local-ci-status.txt` 保留首次失败和后续运行退出码，不覆盖失败记录。

| 验证 | 实际结果与证据 |
| --- | --- |
| Uni 生命周期与真实历史 QUIC | 41/41；`ownership-final-green.log`，Uni25、历史16，包含取消、过载、预算、容量、认证父上下文与旧协议 |
| 完整成员 E2E | 56/56；`ci-membership.log` |
| 故障/真实依赖证据组 | 18/18；`ci-evidence.log` |
| 隔离直连文本和文件 | 通过；`ci-direct.log` |
| 完整 Relay 场景 | 最终源码通过；`ci-relay-source-final.log`，包含 E15 双向基线与重启（46.065 秒） |
| Relay-only 三进程历史复用 | 最终源码通过；`ci-relay-history-source-final.log`，16.32 秒；`target/membership-history-relay/` 留存连接与实际 UDP 丢弃计数 |
| 固定未修改 main 的 `/4` 双向历史 | 最终强制脚本 2/2；`ci-fixed-main.log`，V1/V2 分别 12.474/12.328 秒；旧端 revision/SHA256 见 `target/fixed-main-history/` |
| 升级 smoke | 5/5；`ci-upgrade.log`，3 个成功互通、1 个预期拒绝、1 个已知不兼容；完整 68 格矩阵的其余 63 格未执行 |
| 最终完整工作区 | 无诊断源码 4202/4202；`workspace-two-slots.log`，485.289 秒，24 skipped；CI profile 外部并发固定为已验证的两槽，用例内并发与原超时、零重试不变 |
| 根门禁 | metadata、workspace all-targets check、fmt、Rust 风格、仓库架构、直接日志宏、audit、diff 均退出 0；`ci-*-final.log` 与 `local-ci-status.txt` |
| 线上、macOS/Windows、物理设备、覆盖率 | 跳过；本轮未推送，计划保持 active，不能声明这些项通过 |

三进程验收还发现拨号完成早于异步物理观察器登记，导致完成记录缺少脱敏 connection_id。
已用确定性 red/green 固定这个顺序：真实握手完成按同一原生编号取得有界本地别名，
后续物理观察复用该别名，关闭清理规则不变；`physical-alias-green.log` 的 3/3 包括真实三进程复用。
未按 peer 或时间猜测关联，也未增加稳定接口。

### 首次失败与剩余不确定性

旧有 E15 配对场景曾在重启前基线剪贴板传输偶发失败：公开发送错误 1252，或接收端 apply_failed。
未修改的 `edf0a296` 宿主同样复现（`e15-old-main.log`），最终宿主单项与完整 Relay 复跑均通过；
首次完整失败保存在 `ci-relay.log`、`ci-relay-final.log`，最终复跑不替代这些记录。
该偶发失败的根因**未确认，也未声称已修复**；临时诊断已全部移除，不进入交付。
原有 Engine 剪贴板宿主合同也在完整工作区两次出现 SendFiles 的 1252（4200/4201）；
同一原始宿主合同单项 20 连跑、相同 nextest 配置 5 连跑和包目录 3 连跑均通过，
带诊断的完整工作区 4201/4201 通过，但不代替最终无诊断源码。根因未确认，
不能认定它与 E15 同因。受限单核压力另出现 ResendEntry 的 1452，症状单独保存。
默认随宿主核数扩张的外部并发还出现既有设置测试关闭 1108（15 秒），保留
`workspace-4201-failed-settings-shutdown.log`；两槽下同用例 4.025 秒完成。最终完整工作区
4202/4202 通过，但不能由此声称旧偶发失败已根治。CI profile 明确固定两槽外部并发，
让本地与线上采用相同资源预算，不扩大期限或启用重试。
新历史 Relay 测试另需在持久化自定义 relay 设置后重启，并等实际 Endpoint 的脱敏连接诊断就绪，
再发邀请；其生产能力来自已有入口，不新增 facade 查询。

`implement` 所需的 code-review skill 在本机未安装；已完成同范围手工审查，工作记录位于
`.planning/2026-10-13-membership-history-connectivity/code-review.md`，不声称调用了缺失技能。


### PR #180 首轮线上失败与后续修复（2026-10-13）

首轮 Actions `38383389293`：仓库检查、Windows x64/arm64 通过；升级 smoke 在脚本第 162 行因
`rg: command not found` 退出，工作区 4201/4202，宿主合同的 SendFiles 返回 1252。
旧源码时间戳刷新改用系统 `find -exec touch`；两个脚本均在无 rg、含空格路径的最小环境 red/green 通过。

本地重复运行精确捕获同一行 1252，其来源为 inline 加密 → ContentProtection::NotActive →
EncryptionError::NotInitialized。当前 Space 材料更新在 vault I/O 前开启隐藏所有读取的事务，
导致已就绪会话临时不可读。通过真实 vault 安全存储读取 hook 插入内容加密，旧实现 5 毫秒确定性失败。
修复由既有 ActiveSpaceSecuritySession 完整承担：私有快照准备、vault 安装、带 generation/Space 围栏的原子发布。
跨 Space 隔离规则不变；稳定合同见[活动安全材料的发布边界](../../SECURITY.md#活动安全材料的发布边界)。

安全会话、内容保护、清理/关闭和取消回归 33/33 通过。临时诊断已从 Rust 源码全部移除。
原始日志、诊断及 red/green 位于 `target/pr180-ci-diagnosis/`；其余最终本地门禁与线上结果待追加。
该结论对应已复现的 SendFiles 捕获失败，不将所有历史 1108、1452 或 E15 失败自动归为同因。

修复后本地完整门禁（日志均位于 `target/pr180-ci-diagnosis/`）：

| 检查 | 结果 |
| --- | --- |
| Linux CI 工作区环境 | 4205/4205，24 skipped，495.070 秒；`workspace-final.log` |
| 故障与真实依赖证据 | 18/18；`evidence.log` |
| CI 成员 smoke | 16/16，含长分区恢复；`membership-smoke.log` |
| 隔离直连与完整 Relay | 通过；`direct-smoke.log`、`relay.log`，E15 73.910 秒 |
| Relay-only 三进程成员历史 | 通过，16.96 秒；`relay-history.log` |
| 升级 smoke | 5/5，完整矩阵其余 63 格未执行；`upgrade.log` |
| 固定旧 main 双向互通 | 2/2；`fixed-main.log` |
| 根门禁 | metadata、all-targets check、fmt、Rust 风格、仓库架构、日志宏均退出 0 |
| 依赖审计与脚本回归 | audit 退出 0（8 条允许的警告）；JavaScript 24/24；`audit-final.log`、`js.log` |

首轮本地全量遗漏 Linux CI 的 `UC_EXPECT_DIAGNOSTIC_STACK=available`，诊断栈测试按默认完整符号要求失败
（4204/4205）；已补齐环境完整重跑，未修改该测试或放宽 CI。审计初次因工具不在 PATH 未启动，随后使用
既有 cargo-audit 0.22.2 完成。以上过程保留原日志，线上修复后结果由 PR #180 的当前 head 检查记录确认。
macOS/Windows 的修复后运行、物理设备与覆盖率未在本机执行，不计为本地通过。
