# Issue #34 第六切片：设备可达性、准入与存活检查执行计划

状态：**实现与必需验收进行中，尚未交付**（2026-10-20）。
唯一设计来源：[实现规格](2026-10-14-peer-reachability-connectivity-spec.md)。
前置第五切片 [PR #181](https://github.com/UniClipboard/Engine/pull/181) 已合并。
审计时最新 main 为 `68de511a77dee74b8b6ba0360fa07f9637241c90`；此值不是已冻结的实现／互通锚点，开工时重新确认。
已完成生产迁移、行为／原生／真实 SQLite 回归及根门禁；完整 workspace 4227 项通过、25 项跳过。必需网络矩阵修复后最终重跑中，平台与线上结果尚未核验。
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

* [ ] 重新读取最近 AGENTS、Rust／错误／安全／观测规范；从开工最新 main 建立实现分支，记录精确 SHA 及相对审计基线的相关差异。
* [ ] 清点 coordinator、Core port 所有消费者、native adapter／handler、身份与撤销表、generation、恢复能力及实际 shutdown owner。
* [ ] 冻结旧端：开工最新 main 和 a07 `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6`；实际核对 a07／rc.15 的源码、ALPN 和构建特性。
* [ ] 同一驱动分别链接两个冻结旧库及当前库，记录驱动源码摘要、Engine rev、构建参数和二进制 SHA256；旧 Engine 库不打补丁。
* [ ] 两台真实 rc.15 正常配对，正常关闭并保存原始加密资料；先证明旧旧 Direct／Relay 和真实内容基线可运行。
* [ ] 从原始资料分别复制两套场景，只升级一端；通过既有升级读取原身份、密钥和成员关系，证明新版主动及接收两种角色均可开始真实互通。
* [ ] 保留 E09 现场新旧配对拒绝测试；核对 `--mode legacy` 的结果仅属于该拒绝合同，不登记为 presence 互通通过。
* [ ] 冻结修改前准入／challenge golden 向量、FIN／关闭行为，记录原 coordinator 常量及网络恢复门的计时起点。
* [ ] 建立最小可重复失败信号：无首流 pending 无入口总额、慢查询越过整段期限、本机真实读取故障被发成拒绝／Offline；每项保存输入、命令、固定分类及实际结果。
* [ ] 核对 CI 网络权限、默认运行入口、旧资料独立副本和必要时长；证明不是靠手动刷新／预发送或模拟 Online 才能执行基线。

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
预配对资料选项、presence case 名称和固定旧端包装入口在实现时确定并写回命令清单；当前不得把计划接口写成可运行验收命令。

* [ ] 执行 spec §8 的新新、冻结 main 双角色、rc.15 双角色 × Direct／Relay 全部必需格；各格精确内容读回、实际 ALPN 和路径证据齐全。
* [ ] 强制 Relay 对 IPv4／IPv6 UDP 的规则与计数有效；验证没有直连绕路，双向内容不是仅发出或仅观察 Online。
* [ ] 新新 v2 三秒 Relay 自动恢复 20 轮；冻结 main 实际 v2 的每角色至少 3 轮，全部满足同一计时／只查询条件。
* [ ] v2 静默失联 ≤20 秒、v1 传输关闭 ≤70 秒；旧 v1 正常恢复与内容通过，不为 v1 添加新版三秒条件。
* [ ] 入站满额、慢帧、真实 SQLite／身份读取错误、成员撤销和在途关闭在真实 Engine 路径通过；有限任务／许可无泄漏。
* [ ] 保留 E09 现场配对拒绝；完整 workspace、成员 E2E、升级 smoke、前五切片连接验收和网络恢复原场景全部通过。
* [ ] 将必需矩阵接入 `.github/workflows/pr-check.yml` 默认运行的验证；按需拆分 job，但失败必须阻止本片交付，不能仅靠可选 label 或 nightly。
* [ ] Linux、macOS、Windows 和现有移动平台编译门禁执行；宿主不因缺 `just`／`rg` 等工具静默跳过必需格，脚本依赖明确安装或采用已有可移植入口。
* [ ] 每次本地先跑对应全部必需 CI，再推送；核验最终 head 的线上检查，保留第一次失败及修复后的命令／SHA／结果。

原已有入口仍有效，新增预配对／混合角色入口落地后补足最终可复制命令：

```bash
bash scripts/testing/run-connection-recovery-e2e.sh --suite all --repeat 3
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode relay --case E10-relay-only --repeat 20
```

上述命令不能独自覆盖新增的真实预配对旧新矩阵。必需格尚未执行、缺少资料或环境不支持时，记录真实状态；不得称为通过或完成切片。
额外历史版本及物理设备不属于本片完成门槛，未执行必须记“跳过”。

## 阶段 6：回写稳定合同与收尾

* [ ] 按实现更新 `docs/design-docs/automatic-peer-connections.md`、相关 Uni 设计／ADR 及架构说明；持久规则只维护唯一正文。
* [ ] 对完整恢复动作检查真实观测导出、失败类别、结束边界和列表／详情；按[唯一标准](../../design-docs/observability.md#业务记录组织标准)记录实际验收及跳过项。
* [ ] 最终实现、本地证据与线上必需检查逐项勾选；未测真机／版本矩阵独立列出，不能从编译推定通过。
* [ ] 工件 manifest 与首次失败证据完整，敏感资料／密钥无上传；检查进程并回收任务所建临时源码／fixture。
* [ ] 完成后规格和计划一起移至 `completed/`，更新 `docs/PLANS.md` 与 active／completed 索引、相对链接。
* [ ] 保留 Issue #34 的文件／blob／进度等剩余范围，不将本片完成写成整个 issue 完成。

## 验证层次与证据清单

| 层次 | 必需证据 | 当前状态 |
| --- | --- | --- |
| 真实旧资料前置 | 两个旧端摘要、正常配对、原资料单端升级、双角色独立副本及实际错误／成功 | 真实 rc.15 旧旧及原资料单端升级前置通过；双角色最终矩阵重跑中 |
| 行为／第二 backend | 额度、绝对期限、v1／v2 生命周期、本机读取故障 source、竞态 | 实现及相关回归已通过；最终网络／平台证据见下文 |
| 真实 Iroh／SQLite／独立宿主 | 完整路径和内容、本机实际错误、取消与 shutdown 资源回收 | 实现及相关回归已通过；最终网络／平台证据见下文 |
| Direct／强制 Relay 互通 | spec §8 全部必需格、网络规则／计数、selected path、ALPN、合成内容结果 | 未执行 |
| 自动恢复 | 计时起点、Online 与双向读回时刻、轮数、无人工触发及首次失败 | 未执行 |
| 本地／线上门禁 | 对应命令、退出码、最终 commit SHA、检查链接及首次失败 | 根本地门禁通过；最终提交和线上检查待执行 |
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
- 本片生产实现、阶段 0 真实升级前置、网络／旧新互通、三秒恢复和实现线上门禁均尚未执行；上表按实际状态登记。
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
