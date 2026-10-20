# Issue #34 第六切片：设备可达性、准入与存活检查执行计划

状态：**完成并归档**（2026-10-21）。实现、本地验收与九项线上必需检查通过；归档后最新 head 的复核状态见 [PR #223 检查页](https://github.com/UniClipboard/Engine/pull/223/checks)。
唯一设计来源：[实现规格](2026-10-14-peer-reachability-connectivity-spec.md)。
前置第五切片 [PR #181](https://github.com/UniClipboard/Engine/pull/181) 已合并。
审计时最新 main 为 `68de511a77dee74b8b6ba0360fa07f9637241c90`；此值不是已冻结的实现／互通锚点，开工时重新确认。
已完成生产迁移、行为／原生／真实 SQLite 回归及最终根门禁。workspace 4303 项、成员全集 56 项、必需网络矩阵 11 格、前五切片与既有网络回归全部通过；跳过项、首次失败与 fixture 对照见实际记录。Linux、macOS、Windows 两架构及 iOS／Android／HarmonyOS SDK 线上检查通过，物理设备与额外历史格明确跳过。
不得从以下阶段 checklist 或纯文档检查推定功能、平台编译和线上检查通过。

## 完整负责人

- **业务负责人**：Application `space/connectivity/peer_connections`，既有 coordinator 负责调度，内部 transport 负责完整准入、验证、唯一观察表及长期连接。
- **调用方唯一动作**：沿用 ensure／verify、refresh、通信失败报告及生命周期动作；Router 一次交接完整入站。产品不拼接拨号、授权与存活步骤。
- **成功**：当前资格、会话代次与完整验证均有效后，按原合同形成 Online 证据；网络可达、握手或单次发帧成功不等于准入成功。
- **失败／取消**：当前尝试结算并保留来源；本机读取故障保留最后有效显示但不放行、不刷新证据。明确拒绝、网络失败及旧代次依规格分别处理。
- **恢复**：既有 coordinator 负责检查、机会合并和退避；transport 不另建重试。重启从真实加密资料与当前成员范围重新建立内存状态，不持久化连接／Online 表。
- **资源负责人**：独立 presence Uni manager 管理暂态及长期租约，Infra 提供原生事实、身份桥和统一撤销；Engine 只组装完整能力与既有观测。

## 共识定位

| 已确认决定 | 唯一细则位置 |
| --- | --- |
| presence 完整迁移，保留正常在线／重连合同 | spec §1–3、§5 Workflow |
| 独立 manager、同 Endpoint、长期最多 3N，尝试并发 4 | spec §5 资源、期限与保留 |
| 首流前 pending 全局 16、识别后每端 2，满额立即拒绝 | spec §5 资源、期限与保留 |
| 入站绝对 20 秒、每网络步 5 秒；出站及存活原预算 | spec §5 资源、期限与保留 |
| 本机读取失败保留显示，终止交换、不发无资格，保留 source | spec §5 在线、失败与兼容 |
| latest main 与 a07／rc.15 真旧端；旧旧配对后单端升级 | spec §8 Integration Test、§11 |
| 新新／旧新双角色 × Direct／Relay，v2 三秒恢复必需 | spec §8 Integration Test |
| 本地及 CI 必需门禁完成；移动真机未跑跳过 | spec §9 Acceptance Criteria |

## 阶段 0：冻结版本、证明真实资料前置和基线

本阶段允许补测试驱动、fixture 管理和失败用例；不修改生产 presence、配对、准入或资料升级规则。

* [x] 重新读取最近 AGENTS、Rust／错误／安全／观测规范；从开工最新 main 建立实现分支，记录精确 SHA 及相对审计基线的相关差异。
* [x] 清点 coordinator、Core port 所有消费者、native adapter／handler、身份与撤销表、generation、恢复能力及实际 shutdown owner。
* [x] 冻结旧端：开工最新 main 和 a07 `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6`；实际核对 a07／rc.15 的源码、ALPN 和构建特性。
* [x] 同一驱动分别链接两个冻结旧库及当前库，记录驱动源码摘要、Engine rev、构建参数和二进制 SHA256；旧 Engine 库不打补丁。
* [x] 两台真实 rc.15 正常配对，正常关闭并保存原始加密资料；先证明旧旧 Direct／Relay 和真实内容基线可运行。
* [x] 从原始资料分别复制两套场景，只升级一端；通过既有升级读取原身份、密钥和成员关系，证明新版主动及接收两种角色均可开始真实互通。
* [x] 保留 E09 现场新旧配对拒绝测试；核对 `--mode legacy` 的结果仅属于该拒绝合同，不登记为 presence 互通通过。
* [x] 冻结修改前准入／challenge golden 向量、FIN／关闭行为，记录原 coordinator 常量及网络恢复门的计时起点。
* [x] 建立最小可重复失败信号：无首流 pending 无入口总额、慢查询越过整段期限、本机真实读取故障被发成拒绝／Offline；每项保存输入、命令、固定分类及实际结果。
* [x] 核对 CI 网络权限、默认运行入口、旧资料独立副本和必要时长；证明不是靠手动刷新／预发送或模拟 Online 才能执行基线。

驱动复用 `tests/hosts/connectivity` 与既有恢复／升级锚点工具。当前 `--peer-host`／`--peer-side` 混合二进制入口默认现场配对，
不能直接用来验收已配对 rc.15；补明确“真实预配对资料”输入与双方角色控制，禁止手工插入资格或改库。
每角色使用原始资料新副本，旧端不读新版改写的资料；敏感 fixture 不进入 Git 或公开工件，执行结束安全回收。
构建按普通用户复用仓库 target 和共享缓存串行进行；网络 namespace 只运行已构建宿主。
临时旧源码放明确可再生缓存，退出检查进程后回收，并恢复当前宿主；不得新增 `/tmp` 构建 target 或留下后续清理责任。

**退出条件**：两锚点精确冻结；旧旧／原资料单端升级的基线成立；有真实正常信号与可重复 red；默认 CI 环境能够运行必要场景。
若 rc.15 配对资料不能按既有升级契约读取，保留 source 和最小复现，标明此前置阻塞，停止进入生产改动阶段。
不伪造资格、不改配对规则、不静默扩大到资料迁移；先报告实际阻塞及所需前置工作。

## 阶段 1：固定协议与 Uni 生命周期能力

* [x] 提取 `crates/uc-sync-protocol/src/peer_reachability.rs` 为唯一纯 codec；原正常线上字节、ALPN、精确帧／nonce／FIN golden 均通过。
* [x] 在现有 Infra connectivity backend 增加实际协商的 v1／v2 presence 能力及协议 profile，保留原 `/2` 优先／`/1` fallback。
* [x] 增加 Application 专属 wire port 和 Infra 实现；方法使用 Uni 半流，IO／timeout typed source 不丢失。
* [x] Uni 支持首流前的临时准入许可，识别后每端许可，以及当前 grant 下的原子提升；全局 16 不留到长期 record 上。
* [x] 增加明确 presence 长期租约策略：每端 2 入站／1 出站、不使用固定空闲容量；首轮流释放不会关闭保留连接。
* [x] 区分 v1 首轮交换能力与长期保留；不将 v1 伪装为 v2 多流，也不改变其他协议的 SingleExchange 默认含义。
* [x] 用测试第二 backend 通过额度、deadline、取消、提升失败和长期持有测试；现有 Uni 其他协议回归通过。

**退出条件**：暂态与长期资源分开，v1／v2 能力明确，旧字节一致，全部生命周期变化有行为测试；没有复制第二套 manager。

## 阶段 2：迁移完整准入与唯一在线证据

* [x] 新增 `peer_connections/transport/`，分离 owner、admission、liveness、ports、model、error 与 tests；保持模块入口简洁、接口最小。
* [x] 将 native adapter 的唯一显示／观察表及成功修订迁入 Application；删除第二状态表和旁路 Online 更新。
* [x] 主动尝试沿用 coordinator 权威资格、完整 20 秒和 4 并发；同轮先核验已有连接，再在必要时拨号，不另建检查周期或重试。
* [x] 入站从 Router 接到已握手连接起获取全局 pending 许可和 20 秒 deadline；每网络步至多 5 秒，身份／资格、重检、拒绝／关闭都计入。
* [x] 复用现有身份目录、成员权威与统一撤销登记；保留真实下层错误来源，不使用资格 `.is_ok()` 合并拒绝。
* [x] 本机读取错误终止当前交换：临时新连接关闭、现有活性流结束、不发送无资格、不刷新证据，保留最后有效显示。
* [x] 准入确认前／提升前重检 grant、代次和 accepting；移除后迟到结果不发布 Online；第三条已准入入站沿原选择规则替换。
* [x] 证明至少 6 对端分批尝试后全部可在线，长期连接不受 4／16 暂态额度限制。

**退出条件**：完整收发经过 Uni，实际本机故障可追溯且显示／对端回复符合规格，观察修订及准入竞态通过。

## 阶段 3：存活、取消与生命周期收口

* [x] v2 challenge 发起／应答全部使用 Uni；同轮候选并行且共用 5 秒，每连接入站并发 2，超量只拒绝该流。
* [x] 保留真实对端拒绝优先级，畸形帧和本机读取故障不进入明确拒绝分支；v1 保留关闭观察且不伪更新 v2 确认时间。
* [x] 迁移 watchdog、旧失败／新成功保护和检查修订；新连接失败不覆盖其他新鲜有效证据。
* [x] 成员移除／再加、切 Space、锁定／暂停、activate／恢复及 disconnect_all／shutdown 收口现有 owner；暂态未知身份连接也可取消。
* [x] 单个手动刷新 waiter 取消不停止共享核验；队列／尝试取消后不开始下一步骤，真实本地工作结算前持有许可。
* [x] 每个准入、保留、存活 task 都有等待方；退出回收许可、连接、任务，join 失败按固定类别记录，来源可查。

**退出条件**：静默网络失败、生命周期和并发竞态测试通过，没有旧代次重新上线、无逃逸任务或虚报额度释放。

## 阶段 4：生产装配与删除旧路径

* [x] `crates/uc-engine/src/assembly/sync_engine.rs` 构造独立 presence manager 和完整 Application port，全部消费者引用同一个 owner。
* [x] `iroh/node.rs` builder 仅提供依赖和注册 bridge；替换 `install_peer_reachability` 的原生业务构造，不保留新旧任选分支。
* [x] `/2`、`/1` Router 都一次转交完整 owner；保留同 Endpoint、原网络恢复能力及统一撤销登记。
* [x] 删除原生 adapter／liveness 的生产业务实现、废弃构造及混合 IO codec；移走仍有价值的测试，thin bridge 的原生类型有清晰边界。
* [x] 增加架构回归限制 presence 业务直接使用 Endpoint／Connection／native stream；不误禁止 backend／Router 的实际网络桥。
* [x] Engine 只装饰完整既有能力，Application 持不透明关联；默认业务列表不新增例行轮询／正常清理入口。

**退出条件**：生产调用图只有新 owner；不是仅测试接新路径；稳定 facade／绑定无新增内部步骤查询。

## 阶段 5：真实网络、旧新互通与必需 CI

优先扩展已有 `run-connection-recovery-e2e.sh`、`connection-recovery-network.mjs` 及独立宿主，不另建替代网络框架。
预配对资料与固定旧端包装入口已落地，完整必需入口如下；原恢复脚本仍用于既有场景回归。

* [x] 执行 spec §8 的新新、冻结 main 双角色、rc.15 双角色 × Direct／Relay 全部必需格；各格精确内容读回、实际 ALPN 和路径证据齐全。
* [x] 强制 Relay 对 IPv4／IPv6 UDP 的规则与计数有效；验证没有直连绕路，双向内容不是仅发出或仅观察 Online。
* [x] 新新 v2 三秒 Relay 自动恢复 20 轮；冻结 main 实际 v2 的每角色至少 3 轮，全部满足同一计时／只查询条件。
* [x] v2 静默失联 ≤20 秒、v1 传输关闭 ≤70 秒；旧 v1 正常恢复与内容通过，不为 v1 添加新版三秒条件。
* [x] 入站满额、慢帧、真实 SQLite／身份读取错误、成员撤销和在途关闭在真实 Engine 路径通过；有限任务／许可无泄漏。
* [x] 保留 E09 现场配对拒绝；完整 workspace、成员 E2E、升级 smoke、前五切片连接验收和网络恢复原场景全部通过。
* [x] 将必需矩阵接入 `.github/workflows/pr-check.yml` 默认运行的验证；按需拆分 job，但失败必须阻止本片交付，不能仅靠可选 label 或 nightly。
* [x] Linux、macOS、Windows 和现有移动平台编译门禁执行；宿主不因缺 `just`／`rg` 等工具静默跳过必需格，脚本依赖明确安装或采用已有可移植入口。
* [x] 每次本地先跑对应全部必需 CI，再推送；核验最终 head 的线上检查，保留第一次失败及修复后的命令／SHA／结果。

原已有入口仍有效，新增预配对／混合角色入口落地后补足最终可复制命令：

```bash
bash scripts/testing/run-peer-reachability-connectivity.sh
bash scripts/testing/run-connection-recovery-e2e.sh --suite all --repeat 3
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode relay --case E10-relay-only --repeat 20
```

上述命令不能独自覆盖新增的真实预配对旧新矩阵。必需格尚未执行、缺少资料或环境不支持时，记录真实状态；不得称为通过或完成切片。
额外历史版本及物理设备不属于本片完成门槛，未执行必须记“跳过”。

## 阶段 6：回写稳定合同与收尾

* [x] 按实现更新 `docs/design-docs/automatic-peer-connections.md`、相关 Uni 设计／ADR 及架构说明；持久规则只维护唯一正文。
* [x] 对完整恢复动作检查真实观测导出、失败类别、结束边界和列表／详情；按[唯一标准](../../design-docs/observability.md#业务记录组织标准)记录实际验收及跳过项。
* [x] 最终实现、本地证据与线上必需检查逐项勾选；未测真机／版本矩阵独立列出，不能从编译推定通过。
* [x] 工件 manifest 与首次失败证据完整，敏感资料／密钥无上传；检查进程并回收任务所建临时源码／fixture。
* [x] 完成后规格和计划一起移至 `completed/`，更新 `docs/PLANS.md` 与 active／completed 索引、相对链接。
* [x] 保留 Issue #34 的文件／blob／进度等剩余范围，不将本片完成写成整个 issue 完成。

## 验证层次与证据清单

| 层次 | 必需证据 | 当前状态 |
| --- | --- | --- |
| 真实旧资料前置 | 两个旧端摘要、正常配对、原资料单端升级、双角色独立副本及实际错误／成功 | 真实 rc.15 旧旧及原资料单端升级前置、双角色最终矩阵均通过 |
| 行为／第二 backend | 额度、绝对期限、v1／v2 生命周期、本机读取故障 source、竞态 | 实现及相关回归已通过；最终网络／平台证据见下文 |
| 真实 Iroh／SQLite／独立宿主 | 完整路径和内容、本机实际错误、取消与 shutdown 资源回收 | 实现及相关回归已通过；最终网络／平台证据见下文 |
| Direct／强制 Relay 互通 | spec §8 全部必需格、网络规则／计数、selected path、ALPN、合成内容结果 | 最终默认矩阵 11 格通过；清理与明文扫描通过 |
| 自动恢复 | 计时起点、Online 与双向读回时刻、轮数、无人工触发及首次失败 | 新新 20 轮、冻结 main 每角色 3 轮、rc.15 每角色 1 轮通过；首次失败分别保留 |
| 本地／线上门禁 | 对应命令、退出码、最终 commit SHA、检查链接及首次失败 | 根本地门禁与九项必需 CI 通过；精确 head／合成树及检查链接见最终记录 |
| iOS／Android／HarmonyOS 真机切网／前后台 | 实机、版本、操作和实际结果 | 跳过：当前未执行，不从编译推定 |
| 额外历史版本／降级矩阵 | 关联原升级矩阵的实际证据 | 跳过：本片不新增其完成义务 |

计划工件目录为 `target/peer-reachability-connectivity-evidence/`；首次失败、修复后回归及每格脱敏工件分别保留。
保存构建 manifest、每格状态／命令／轮数、脱敏固定分类、路径事实、恢复阶段时刻和失败摘要；运行内的原始身份、地址、邀请和资料只供隔离进程使用，不能进入公开日志／工件。
出现环境失败与行为失败分别记录；重跑成功不能覆盖首次失败。

文档和实现交付均执行仓库根门禁；本片行为实现另须上述功能／网络／互通与对应 CI：

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

Cargo 由一个负责人从根目录通过共享 target 串行运行，保留编译缓存。执行命令适配[测试指南](../../design-docs/testing-guide.md)及仓库实际环境，
不清空 `RUSTC_WRAPPER`，不用 `/tmp` target 或通过 sudo 编译来绕过环境问题。

## 实际执行记录

- 2026-10-14：完成设计访谈与本文／规格编写；审核第五切片已合并及审计 main SHA。
- 2026-10-14 设计交付时：生产实现、阶段 0 真实升级前置、网络／旧新互通、三秒恢复和实现线上门禁尚未执行。后续实际结果见对应日期记录及最终汇总。
- 文档交付检查通过：`just cargo metadata --locked --format-version 1`、`just cargo check --workspace --all-targets --locked`、`just cargo fmt --all -- --check`、Rust style、Engine repository preflight 和 `git diff --check` 均通过；新文档单独核对空白及文件结尾。
- 检查时本地代码 HEAD 为 `6bc4d791cd9606604f29b073b77e6512c6ee05a4`；四个文档入口的 53 条相对链接均存在，规格 11 节完整，实现 checklist 均保持未勾选。Cargo check 仍有既有未使用导入／函数警告。
- 以上是本轮纯文档交付检查，不能作为阶段 0 或第六切片功能／线上门禁已通过的证据。

### 2026-10-14 实现中的实际证据

- 开工冻结 main：`b6fa0ce6b2efd1c1764e76c0a76537bdeead4e1b`，较设计审计基线增加 PR #184 日志目录保留修复；已合入当前分支。rc.15 锚点仍为 a07 的固定 SHA。
- 生产改动前，`d2::a07_to_head` 通过（真实旧旧配对、内容和单端升级）。rc.15 两个升级位置的 Direct／Relay 原资料前置通过；角色控制与最终源码矩阵另行重跑。
- 纯 codec 固定向量、Uni v1 保留、pending 共享／提升释放、慢 grant 读取保留许可、取消后真实 acquisition 结算测试通过。
- 46 项 coordinator／完整 transport 回归通过，包含本机故障显示与 source、scope 四实际工作额度、移除再加、shutdown typed join、透明关联与私有错误正文不泄露。
- 原生五项回归通过：17 条无首流上限、第三入站替换、同连接双向 v2、读取故障只结束流、真实拒绝优先于并行成功。原拒绝 FIN 遗失及测试过早 drop 的首次失败保留。
- 真实 SQLite 表读取故障与真实 Iroh／生产 Node bridge／完整 owner 的组合回归通过，完整 source 可追到 Diesel；该证据不是独立 Engine facade 的全部故障矩阵。
- 本机范围读取失败原会清空已显示 Online：新 red 已保存，修复后保留显示并暂停新尝试，有效范围恢复后继续核验。
- 20 轮必需新新 Relay 首跑在第六轮失败。修复把调用取消传入原网络验证，防止旧 actor 长期占检查锁；保持真实本地工作许可，三秒门未放宽。最终 20 轮与两个旧锚点双角色矩阵待后续证据。
- 默认 PR 工作流已接入完整网络升级矩阵及 iOS／Android／HarmonyOS 编译门禁；本地完整门禁、最终 SHA 线上结果尚未核验。
- 计划继续 active，移动真机仍为跳过；未勾选或未执行项目不登记通过，不归档此切片。

### 2026-10-15 续接验收

- 上轮全工作区 `workspace-final-fixed.log`：4227 passed、25 skipped；本轮原生 `resume-native.log`：7 passed，涵盖同连接双向 v2、真实拒绝优先、本机读取失败只结束流、无首流限额、第三条入站替换及第三条活性流回收。
- 本轮根 metadata、全 workspace 全目标 check、fmt、Rust style、repository architecture 和 diff check 均通过；既有未使用代码警告仍在。
- rc.15 receive／升级 Joiner 的首次失败保留：旧 Sponsor 的原资料只持有 Joiner 启动直连地址，重启后在强制 Relay 中握手超时。最终双角色分别升级 Sponsor（receive）和 Joiner（initiate），独立原资料保持，未改旧库或人工补地址。增加升级前后 transport endpoint 身份一致断言。
- rc.15 receive／升级 Sponsor 准入及双向内容通过。首次恢复因驱动统一 20 秒 Relay 重连观测超时失败；旧 Iroh 自身有最长 16 秒退避与 10 秒连接期限，v1 不适用新版恢复时限，改为最多 90 秒只查询等待底层重连。70 秒关闭门和所有 v2 门限保持原值。
- `resume-rc15-v1-readiness`：关闭识别 64741ms，Relay 重连 13050ms，就绪后 Online 347ms、双向精确读回 588ms，无会话重建，资料清理与明文扫描通过。首次失败不覆盖。
- 完整默认矩阵在 `resume-required-matrix/` 重跑；成员全集、升级 smoke、前五片及原网络场景、平台和线上检查尚待结算。计划继续 active，不归档。

### 2026-10-20 续接中的实际证据

- 上次 `resume-required-matrix` 在第二轮底层 Relay 就绪观测处超时，尚未进入双方就绪后的三秒门。新旧 Iroh 都有最长 16 秒退避及 10 秒连接期限，驱动统一最多观察 90 秒并单独记录底层重连耗时；失联 20／70 秒及就绪后三秒门均不改变。失败工件保留。
- `oct20-relay-probe` 三轮通过，就绪后 Online 为 58／364／209ms，双向精确读回为 128／534／297ms；无会话重建，资料清理与明文扫描通过。20 轮最终回归运行中。
- 上轮成员全集 56 项均在邀请阶段返回 1227（没有可发布地址）。本轮同源单项入组通过，全集重跑中；当前不据此认定原失败根因或已修复。
- 审查合并 node 的同源名称引入并删除无同名冲突的别名；无行为变化。HarmonyOS 真实 SDK runner 当前 online，平台仍须等待最终提交的实际 CI。

- `oct20-relay-final` 新新 Relay 20 轮全部通过：Online 最慢 354ms、双向读回最慢 569ms，底层重连观测最慢 18924ms，清理／明文扫描通过。
- 宿主网络成员全集：55 passed、1 failed、14 skipped。唯一失败为本地六位码 mDNS；同二进制在宿主三轮有一次失败，独立 user/net namespace 三轮全通过。完整隔离网络 nextest 运行中，不延长解析 5 秒期限，不改配对生产逻辑。
- 真实观测验收发现旧入站成员历史用途描述落在 decorator 创建前的外层，完整 handler 节点仍是通用名称。现有 clipboard trace 测试 red 已保留；用途描述移到 Application 完整处理入口，沿用既有不透明观测能力，不扩大 Engine 接口。最终矩阵在该修正后重建重跑，已中断的运行不记通过。
- 主线整合：保存迁移检查点 `543d00cc`，以 `a854e895` 合入 `f0627a8b0bcc274707035a86ab2df47154d1e536` 的 32 项更新，包括 ADR-034 的 Iroh 固定 Git 来源、日志及构建缓存。开工旧端互通锚点仍固定 `b6fa0ce6`，不得随 main 前移。
- 完整隔离网络成员全集通过：56 passed、14 skipped（670 秒）；宿主 mDNS 的首次失败仍保留，未据隔离结果宣称宿主根因已解决。
- 整合后 metadata、workspace 全目标 check、fmt、相对 main 的 Rust style、架构及 `cargo audit --deny unsound` 通过。相关回归 108 项中 107 通过；唯一失败发生在本机 Collector 回放，根路径探测被一并回放导致 404，已限定仅回放通过隐私断言的 OTLP POST traces/logs，完整重跑待结算。
- 新增完整 owner 时序回归 red／green：资格读取期间最后连接关闭已发布 Offline，迟到读取错误不得静默恢复旧 Online。删除旧显示快照恢复，保留最新有效状态和原错误来源。
- 已用真实浏览器打开 Jaeger business/test 列表及 clipboard.copy_and_sync 详情；完整父子动作、根结果及正常轮询／清理不进入业务列表的截图和 DOM 证据保存在本地工件。Docker 无权限时使用已校验摘要的同版本本机 Jaeger／Collector；不修改仓库隐私规则。
- `oct20-final-related.log`：109 项全通过，包含新显示竞态、7 项原生 presence、5 项显式真实 trace 及 Collector 回放。最新浏览器详情展开确认根结果；官方工具摘要与隐私配置一致性证据保留，临时服务／浏览器／下载已回收。
- 整合 main 后 workspace 首轮为 4300 passed、1 failed、1 timed out、27 skipped（627 秒）。两项为本地 reset 的 20 秒外层超时和 repeated_leaves 资源波动；宿主与独立网络的重复实验显示默认公网 Relay 的后台连接／重试影响完成时刻与资源样本。
- 两项本地测试使用已有 dev-tools 配置关闭公网 Relay，保持所有原超时与资源阈值；定点两项通过，再各三轮全部通过（reset 3.98–4.59 秒，leave 9.06–9.26 秒）。生产默认 P2P／Relay 行为不变，真实 Relay 仍须完成默认矩阵。完整 workspace 正在 `oct20-final-workspace-fixed.log` 重跑，首次失败和全部对照工件保留。
- `oct20-final-workspace-fixed.log` 最终为 4302 passed、27 skipped（585.950 秒），原两项失败均通过，JUnit 独立保留。完整默认矩阵在 `oct20-final-required-matrix` 运行；当前仅开始构建，不将尚未完成的互通格登记通过。

### 最终默认网络矩阵（2026-10-20）

`oct20-required-final-incoming-control/matrix-summary.json` 校验 11 个必需格全部通过，所有记录均有非空成功结果，`cleaned` 与 `plaintext_clean` 均为 true。开工冻结 main 仍为 `b6fa0ce6`，rc.15 为 `f6f305d9`，旧库不打补丁。旧旧配对关闭重开后，原 Space／传输身份与双向内容基线全部通过。

| Relay 组合 | 轮数 | 最慢失联识别 | 原生就绪后最慢 Online／双向精确读回 |
| --- | --- | --- | --- |
| 新新 v2 | 20 | 17145ms | 372／577ms |
| 冻结 main v2 接收 | 3 | 15172ms | 330／504ms |
| 冻结 main v2 主动 | 3 | 14816ms | 370／522ms |
| rc.15 v1 接收 | 1 | 64802ms | 422／580ms（不作为 v2 三秒门） |
| rc.15 v1 主动 | 1 | 64718ms | 256／417ms（不作为 v2 三秒门） |

双方底层重连观测另计，新新最慢 19027ms，rc.15 最慢 25712ms；事件发生时间定义和三秒门保持规格原值。纯 TCP 提前计时、旧库刚配对运行态内容失败及固定端口实验的首次工件保留，未计作通过。最终 workspace／成员／升级／原场景回归和平台 CI 仍未结算，计划继续 active。

- 2026-10-21：`oct20-final-workspace-with-incoming.log` 最终 4303 passed、27 skipped，测试执行 633.977 秒，含构建总耗时 1108.291 秒；新增原生 incoming gate 与两项生命周期回归全部通过。JUnit 单独保留；成员全集已开始，其余回归与线上平台门禁仍待结算。

### 成员全集的历史恢复 fixture 对照（2026-10-21）

最终成员全集两轮均为 55 passed、1 failed、14 skipped（746／735 秒含构建）；同一缺失前序恢复场景在 B 已追上 epoch 5 后，C→B 确认未在原 120 秒内完成，投影为 ReceiverUnavailable，第三次尝试的下一重试时刻接近测试结束。日志中的握手超时未按测试节点关联，不能据此把刻意离线的 A 与 C→B 混同。

同一原二进制的隔离／宿主／隔离三轮均通过（36–40 秒），并发 g4 与目标两项也通过；这些结果不替代完整组验收。沿用[既有组更新 CI 缺口 fixture 前置](../completed/2026-10-11-group-update-connectivity-plan.md#ci-独立进程缺口场景修正)，仅目标 B 使用既有测试 bind port 配置固定重启地址，保留真实关闭重开、原发送者离线、第三方历史、epoch、实际 ACK 与内容断言，原 120 秒不变。自动连接与网络换址场景保留原配置。

固定端口目标单项 36.447 秒通过，在完整 ci 二线程组中 36.925 秒通过；最终全集 56 passed、14 skipped（680.799 秒），56 个场景结果工件与独立 JUnit 保存。此项只记录 fixture 前置收敛，不声称已完整证明并修复首次失败的生产根因。原二进制、首次日志和 JUnit 分别保留。

### 最终本地门禁汇总（2026-10-21）

`oct21-local-final-summary.json` 校验 20 个本地阶段均已通过，保留三个首次失败阶段的原记录。最终原网络四种模式共 22 个场景通过，清理与明文扫描全部通过；已知设备换址另跑三轮，自动上线 3597／3587／3758ms，旧端口关闭、新端口绑定、发现阻断和实际双向内容均通过。

| 验证 | 实际结果 |
| --- | --- |
| workspace | 4303 passed、27 skipped |
| 完整成员组 | 56 passed、14 skipped；56 个场景结果均 passed 且 cleanup completed |
| 升级 smoke | 5 个断言通过；矩阵保留 3 pass、1 rejected、1 known-incompatible，额外历史格未执行 |
| testkit 证据 | 18 项通过，正／负例工件存在 |
| 当前剪贴板 Direct／强制 Relay | 各四类完整内容读回通过 |
| 成员历史 Relay 跨轮复用 | 真实三进程通过，UDP 阻断有计数 |
| 固定旧主线组更新／剪贴板 | v1／v2 双向两项通过；剪贴板双方角色与完整内容通过 |
| 原网络／额外换址 | 四模式 22 个场景通过；额外换址三轮通过 |
| 最终根门禁 | metadata、workspace all-targets check、fmt、Rust style、架构、25 项脚本测试、audit、diff check 均通过 |

原网络首次失败由扩展双栈工件时误把 `counters` 字段加到通用分区与 IPv4 对端隔离造成；已删除这两处无定义且语义不实的字段，强制 Relay 的实际 IPv4／IPv6 计数保留。首次日志不覆盖，不修改 Engine 或任何期限。

确认无本任务活动网络／Cargo 进程后，回收 a07 与 a13 的本次源码快照和临时观测缓存；保留共享 target、编译缓存、旧端二进制、脱敏证据及隔离 runner 源码快照。cleanup 证据为 `oct21-local-cleanup.json`。最终提交／PR、平台 SDK 与线上检查尚待执行，规格和计划继续 active。

## 最终线上验收与归档

[PR #223](https://github.com/UniClipboard/Engine/pull/223) 的实现提交 `42fff70404356472bcdf07b9228d1b68123a7da7` 已通过全部九项必需检查：

- [PR Check](https://github.com/UniClipboard/Engine/actions/runs/39468554241)：macOS 仓库／诊断栈、Linux Engine tests、升级 smoke、完整 presence 矩阵、iOS、Android、HarmonyOS SDK 均成功。
- [Windows Build](https://github.com/UniClipboard/Engine/actions/runs/39468554023)：x64 与 arm64 全目标编译及原生存储持久性测试均成功。

本轮 CI 的基线为 `2956f3dba1ef545132550afa737b2462df96d98f`，实际合成提交为 `5c1f09962d5db9dd63379c4d6aa1a03659bc3cc0`。本地随后以 `807e13eaad9e866b9d797eea616d9c8fbc5da737` 合入同一主线：仅承接 rc.23 版本与 MBX 包装器修复，整个已提交树与 CI 合成树完全相同，树对象为 `17db85df3e6ec0b79b605f3178cf9c4416cdb119`。本片没有新增持久格式；开工旧端锚点保持原 SHA。

下载的线上矩阵工件逐格校验通过：11 格记录非空且全部成功，cleaned／plaintext_clean 均 true；新新 20 轮双向读回最慢 499ms，冻结 main 两角色各 3 轮最慢 469／439ms，rc.15 两角色各 1 轮也完成原关闭及恢复合同。核验保存在 `oct21-pr223-matrix-verified.json`，任务本地证据只保留脱敏内容。

可选 coverage 与额外 isolated-networks job 未执行，明确记为跳过；原网络四模式已在本地完整执行。CodeRabbit 的 draft 跳过标记不作为审查通过证据；本片已进行完整调用链和最终差异手工审查。物理设备切网／前后台及额外历史矩阵保持跳过。

规格与计划一起移至 completed，稳定设计与索引同步；文件／blob／进度等后续范围继续留在 Issue #34。归档提交与已验证实现的程序输入保持一致，末次提交的线上状态以 [PR 最新 head 检查](https://github.com/UniClipboard/Engine/pull/223/checks)为唯一动态来源，通过后交付。

## 归档 head 网络失败后的验收驱动修正

归档提交 `fa3b4e087a0449bca4fd6c00d9901d357dc7341c` 的[第二轮 CI](https://github.com/UniClipboard/Engine/actions/runs/39475310370)中，八项其他必需检查成功，网络矩阵在 rc.15 Direct receive 失败：旧旧重开及双向内容通过，随后旧 B 再次打开并执行显式 unlock 返回 1213，新 A 尚未启动。首轮通过不替代这次失败；工件及 job 日志保留。

核对旧库启动链可知 Engine 已尝试恢复会话，而 P00 驱动仍无条件再次 unlock；已有升级矩阵的 `unlocked_observation` 会先读取 `session_ready`。P00 改为同样使用公开 observe：已恢复会话直接使用，仍锁定才执行一次 unlock 并复核就绪。查询、解锁和仍锁定的错误均继续失败，不重试、不更改旧库、资料或任何期限；工件记录每个节点是否在启动时恢复。边界测试以原无条件调用产生 red，修正后三项通过。

原驱动本地 15 轮未复现；额外带就绪观测的对照确认会话已恢复后仍被重复解锁。稳定错误接口不暴露旧版内部 source，未证明 1213 的具体内部根因；本次修正的是驱动多做的解锁动作。必须在新提交上重新完成默认矩阵和全部九项线上检查后交付，最新状态仍以 PR head checks 为准。

修正后的本地默认 11 格全部通过并逐格核验工件：新新 Relay 20 轮关闭最慢 19071ms、双向读回最慢 581ms；冻结 main 双角色各三轮读回最慢 531／525ms；rc.15 双角色各一轮关闭 64767／64673ms，读回 508／536ms。身份、成员、精确内容、真实角色及每格清理／明文扫描均通过；旧版配对拒绝仍通过。原失败格还额外连续通过三轮。根 metadata、workspace all-targets check、fmt、架构、Rust 风格、29 项脚本测试和 diff check 通过；运行进程与本次临时旧源码已回收。最终提交仍须等待九项线上检查，结果保存于 PR 最新 head 检查与 PR 描述。
