# Issue #34 第三切片：组更新收发与主动恢复执行计划

状态：**完成（2026-10-12）**；沿用 `/4`，交付完整 Uni 收发、统一安全应用与主动逐页恢复。物理设备矩阵明确跳过，整个 Issue #34 保持开放。基线：`origin/main c0351c0e`。
唯一设计来源：[实现规格](2026-10-11-group-update-connectivity-spec.md)；本文只记录执行顺序、
门禁与证据，不复制协议和状态规则。最终实现、审查修正与验证证据见本文末尾。

## 责任与调用

- 完整负责人：Application Space 成员模块，内部完整 receiver/apply coordinator 与既有 worker 协作。
- 调用方唯一动作：提交对目标的完整组更新发送，或将完整认证入站交换交 receiver。
- 成功：对应更新经过验证、保存和安装，真实成功 ACK 才结清发送 pending。
- 失败：结构化暂不可用、明确拒绝、带 source chain 的本地/传输失败，不以历史 ACK 代替成功。
- 重启/重试：同一成员 owner 从加密记录、材料与待完成效果恢复，原维护调度负责有界退避。

## 阶段 0：范围冻结

状态：完成（2026-10-12 用户确认沿用 `/4`）。

已核对当前 `/4` 历史后缀限制、组更新双版本、成员授权、材料保存/安装断点、当前 v5 成员
记录及第二切片 G2 证据。已确认的行为按规格保持。

退出条件已满足：用户明确沿用 `/4`，本切片无新历史 wire 与批量交付承诺；减少往返留到后续。

## 阶段 1：先固定失败与兼容事实

状态：完成，定向、workspace 与真实宿主复核通过。

1. 从当前原 adapter 固定 `/2`、`/1` framing/trace/ACK vectors，不以重构后的 encoder 生成全部基准。
2. 写真实 MLS 的缺 4 收 5、同 epoch 非法 payload、保存后安装失败和 ACK 丢失回归，确认能暴露现有问题。
3. 梳理两条 apply 入口与成员 CAS，确定最小应用证据及最终格式清单；记录证据不足的安全恢复方式。
4. 固定当前 v1–v5 格式样本，核对旧材料启动和历史效果重放；不得先实现多个过渡格式。

退出条件：每个问题有可复现输入/预期/实际结果，成功定义不依赖 epoch 快捷路径或历史 ACK。
失败本身是本阶段证据，不能提前把最终验收标为通过。

## 阶段 2：协议提取和 Uni 能力

状态：完成，定向、workspace 与真实宿主复核通过。

1. 机械提取 group_update codec 至 uc-sync-protocol，完成 byte-level 兼容测试。
2. 在 Infra 增加有限协议 profile 和实际协商能力，装配独立 GroupUpdate manager。
3. 双组版本固定 SingleExchange；覆盖同 peer Clipboard/GroupUpdate 隔离、排队取消及准入围栏。

退出条件：冻结向量通过，现有 Clipboard 回归通过；本阶段不启用不完整生产 receiver。

## 阶段 3：完整接收与统一应用

状态：完成，定向、workspace 与真实宿主复核通过。

1. 新建成员内部 group_update 模块，按模型、port、流程、错误和测试拆文件。
2. 实现完整 receive/ACK 生命周期，身份与成员授权来自当前认证会话。
3. 统一直接入站和已验证历史效果的 apply 协调；区分缺前置、损坏、非法、可恢复安装错误。
4. 处理保存后未安装、已安装 ACK 丢失、撤销和 generation 切换，不遗弃 blocking local work。

退出条件：阶段 1 的问题得到行为证明，旧 `/1` 暂不可用关闭不产生明确拒绝。

## 阶段 4：持久主动恢复与调度

状态：完成，定向、workspace 与真实宿主复核通过。

1. 将恢复需求并入原成员记录/CAS，复用真实历史流程与因果效果队列。
2. 必须新增字段时，统一最终 v6 加密格式和 v1–v5 迁移；验证损坏、generation 和旧材料证据。
3. 四类明确事件合并唤醒，原 scheduler 管理 2/4/8/16/30 秒退避，无任务无额外周期。
4. 明确拒绝保留阻断；正确历史恢复后由实际应用与成功 ACK 完成发送结清。

退出条件：真实 SQLite/MLS 连续恢复、部分进度、崩溃重启及 no-progress 退避测试通过。

## 阶段 5：沿用 `/4`

状态：沿用 `/4`；真实连续缺口、原来源离线与接收者重启验收通过。

2026-10-12 用户确认本次沿用原逐页同步，不新增批量协议版本。验证主动恢复与可靠性；
减少往返与一次发送 bundle 明确留到后续，不用冲突证据通道代替普通后缀。

## 阶段 6：生产接线与删除旧路径

状态：完成，定向、workspace 与真实宿主复核通过。

1. Engine 只组装完整 owner/port；Node 注册 handler 并按会话 generation 管理关闭。
2. 删除被取代的组更新原生拨号、wire 和接收业务实现，不保留生产双轨。
3. 搜索所有组更新发送、入站注册及历史本地效果入口，确认实际生产调用均进入新 owner。
4. 观测沿用不透明上下文和 Engine 领域 port decorator；不扩 facade 作测试查询。

退出条件：真实 Engine 路径验证通过，关闭、超时、源错误链与跨协议隔离具备证据。

## 阶段 7：系统验收与归档

状态：完成；保留首轮失败、最终通过与设备跳过记录。

按规格第 8 节执行。G2 先证明真实篡改接收和纯联系不能解除拒绝，再释放正确历史恢复并
证明实际成功；不增加 tamper 次数或放宽时间预算掩盖问题。

| 项目 | 当前状态 | 实施退出条件 |
| --- | --- | --- |
| Codec、授权、分类、退避、协议隔离 | 通过 | workspace 最终 4182 项通过，保留旧向量来源与真实协议隔离证据 |
| 真实 MLS/SQLite 部分进度和崩溃恢复 | 通过 | 真实子进程在保存后安装失败、安装 4 未确认两点被 kill；重开 SQLite 重装 4 并继续 5 |
| Linux 独立进程真实网络 A/B/C | 通过 | 三进程拒绝恢复与五进程缺口、原来源离线、B 重启通过 |
| 新端/固定 main 旧端双方向 `/2`、`/1` | 通过 | 未修改旧端 c0351c0e，二进制摘要与实际 ACK 模式保留 |
| 批量历史互通/减少往返 | 不在本次范围 | 沿用 `/4` 逐页同步，明确未交付 |
| G1–G4、确定性 G2 | 通过 | 完整成员组 56 项通过、14 项原配置跳过；确定性 G2 31.5 秒 |
| 第二切片 Clipboard 回归 | 通过 | 多流/重启/准入保持；显式 ignored 场景单列 |
| 桌面、iOS/Android/HarmonyOS 真机 | 跳过 | 有设备时单列真实结果 |
| 真机网络切换/休眠/前后台 | 跳过 | 有设备时单列真实结果 |

完整 workspace 原有 G2 失败历史不能静默删去。每次保存命令、退出码、失败用例与相关日志；
隔离通过不替代全套失败解释。长期事实按实际结果回写对应设计文档，更新维护记录，完成后
将 spec/plan 归档 completed，保留 Issue #34 后续切片。

## 验证命令与证据

所有 Rust 命令从仓库根运行，使用 `just cargo`（封装原 cargo 命令），共享 `target` 和环境
编译缓存，不使用 `/tmp` 构建目录，不清空 wrapper。Cargo 由一个负责人串行执行。

先执行修改域的定向测试，再执行真实系统矩阵及仓库既有 workspace/成员测试组。
实际筛选参数以测试清单为准；不能用 `cargo check` 或无匹配的筛选冒充行为测试。

根交付门禁：

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

文档另查相对链接、孤儿入口和旧路径；实施有行为改动时执行规格里的行为测试。
独立进程 host 只扩 devtools 控制，旧端不得要求支持新命令。无发布操作，不执行发布打包验收。

每次验收记录模板：

```text
日期 / 基线 / 测试二进制：
场景与输入：
命令或设备动作：
预期：
实际 / 退出码：
证据位置：
通过 / 失败 / 跳过及原因：
```

## 本次文档交付记录

- 2026-10-11：从最新 main 创建文档分支；仅撰写 spec/plan 与导航，不实现第三切片。
- 发现历史 `/4` 无单请求多更新能力，已向用户询问范围；在获得选择前保留明确开放问题。
- 文档交付门禁通过：locked metadata、workspace all-targets check、fmt、Rust style、Engine repository preflight、相对链接、11 节结构与新增文档空白检查、diff check。workspace check 保留现有 unused-import/dead-code 警告，未为文档任务改代码。
- check 日志位于本地 `target/group-update-connectivity-spec-evidence/check.log`，metadata 位于同目录 `metadata.json`；这是可再生本地证据，不作为提交源文件。
- 行为/系统测试未执行；preflight 自带的 6 项 OpenMLS 可执行检查通过，不等于第三切片系统验收。

## 实施记录（2026-10-11）

- 用户调用 implement，完整实现与当前分支提交已授权；第 11 节的批量范围再次通过问题卡片待答，没有默认新增历史协议。
- 原 Iroh adapter 的无 trace 与有 trace 请求向量实测通过：`UCT1` + postcard，长度前缀为大端 u32；规格的 JSON carrier 表述按源码纠正。原测试退出 0，日志 `target/group-update-connectivity-spec-evidence/original-wire.log`。
- `uc-sync-protocol::group_update` 已提取请求、ACK、边界与泛型流 framing，原 adapter 已使用唯一 codec；完整 Uni receiver、apply coordinator、持久恢复及调度尚未实现。
- TDD：新 topic 未实现时编译失败（退出 101），实现后三个向量测试通过；尾随字节回归旧解析语义失败（4 通过、1 失败），随后改为完整长度解析。合法帧不变。
- 同 epoch 非法 payload：使用原真实 MLS sponsor fixture，在当前材料 epoch 发送 version=1、commit/catalog=[0xff]，断言不可接受；原实现返回成功，回归测试退出 101，证实 epoch 快捷成功问题。临时失败探针的源码与日志保留在 `target/group-update-connectivity-spec-evidence/same-epoch-regression.rs` 和 `security-red-with-feature.log`；正式修复及保留回归测试待统一应用切片，当前没有把失败标成通过。
- 安全测试首轮没有开启 test-util，导致已有辅助方法缺失、70 个编译错误（退出 101）；补齐 feature 后才执行上述真实行为探针。命令为 `just cargo test -p uc-infra-security --lib --features test-util same_epoch_unverified_update_must_not_be_acknowledged --locked`。
- code-review 技能在本地技能目录未找到；独立 codec 切片按仓库约束审查，完整实现完成后仍需最终审查。
- 独立 codec 切片验证：`just cargo test -p uc-sync-protocol --locked` 全套 54 项通过（含新增 6 项）；`just cargo test -p uc-infra-p2p --lib network::iroh::group_update_adapter::tests --locked` 11 项通过，退出均为 0。日志为证据目录内 protocol-tests.log、adapter-tests.log。
- 根门禁通过：locked metadata（25 个 workspace member）、workspace all-targets check、fmt check、Rust style、Engine repository preflight、相对文档链接和 diff check；workspace check 保留现有警告。独立 codec 差异审查未发现新增问题，重点核对合法旧字节、长度分配边界、错误 source、实际版本 ACK、上下文注入边界和唯一 codec。
- 完整 workspace 行为套件、Linux A/B/C 独立进程、旧端二进制互通、G1–G4、恢复断点与设备矩阵尚未执行；不能以 codec 与 adapter 通过替代第三切片验收。第 11 节待答，完整功能未交付，不归档计划。

## 2026-10-12 继续实施

- 用户确认沿用 `/4`，批量协议不在本次任务范围。范围选择已完成，无需再次确认。

- 最终持久形状（首个写入切片前确定）：成员记录只新增最终 v6，增加单一可合并恢复附录（可信 base_position、target_epoch、payload_digest），旧 v1–v5 迁移时附录为空。应用证据随现有加密 SpaceKeyMaterial 的 JSON 保存为 epoch→确切 payload digest，默认缺失不视为已验证；证据与验证后材料同一次加密写入，安装失败可从该材料重装。沿用现有可缺省 JSON 字段演进，不增加 SQLite 明文列或索引，不产生成员 v7。旧证据只能由当前可信历史匹配与真实材料校验、重装恢复。
- 暂存材料兼容补充：现有准入/分支恢复 staged_state 是 postcard SpaceKeyMaterial，不能直接套 JSON 缺省字段规则。新增单一最终本地材料 envelope V2（UCM2 非碰撞标记），保留旧无标记布局解码；只影响本地加密暂存，不修改网络 /4、/2、/1。旧布局恢复时应用证据为空，不伪造已验证。

- 后续统一应用切片：直接接收与可信历史由同一协调器串行；本机存储/材料/安装失败保留 source 并关闭交换，不形成对端安全拒绝。缺前置或未知历史重放才登记恢复需求；未验证包摘要不参与历史授权，按可信祖先顺序补齐。
- 最终 v6 附录补充（仍未发布，不新增 v7）：签名移除携带生成的公开 MLS update，另以 digest→本机私有暂存材料随历史同一 CAS/AEAD 保存；效果真正保存与安装后清除暂存。准备期间不安装安全材料，协调锁持有到历史提交；取消或提交失败不产生安全推进。
- 新增 ACK 后有界清理，完成计时截止实际 ACK；远端 trace 在认证和读取完成后绑定尚未进入的完整接收 span，不增加 Engine 阶段查询。
- 安全模块全套 116 项通过（security-suite-latest.log，含本地暂存旧布局/V2 回读）；这是移除准备切片前的结果，后续修改仍须重跑。G2 首轮全局门控阻断了健康成员之间的后续历史，已改为仅门控 B 的双方向后缀/证据，保留摘要与健康成员同步，旧失败证据保留 g2-gated.log。


## 已完成的关键验收（2026-10-12）

- 确定性 G2：`g2-gated-5.log`，一次实际 wire 篡改，真实拒绝；纯联系仍阻断；放开 B 的双方向正确历史后实际保存/安装和 ACK 结清，32 秒通过。
- 真实缺前置与重启：`gap-restart-engine.log`（106 秒）及 `gap-restart-process.log`（独立进程、36.7 秒）通过。B 在 3 收到 C 的 5，4 被实际网络隔断；A 停止、B 重启后沿 `/4` 从 C 补齐，实际 Crypto 5 与发送 ACK 验证。
- 独立进程拒绝恢复：`three-process-6.log`，10.6 秒通过。测试等待公开加入状态 Active，不能把短暂 Completed 展示当作加入已完成。
- 固定旧端：未修改的 main `c0351c0eb48ac87abb93d9fc3941bdee5dc3a005`，SHA256 `0e9f853304ccbec203efd7b2fb25445b7cf9be2df39f74c551e126526c54f495`。`fixed-main-interop.log` 中 /2 通过，`fixed-main-v1-3.log` 中 /1 双方向真实发送与 ACK 通过；/1 测试同时限制当前端 TLS 公告，避免固定 Router 仍宣布 /2。共享 target 的临时 worktree 已回收。
- 安全全套最新 119 项通过：`security-final-119.log`。包含真实 vault 安装失败、已保存材料不能快捷成功、实际恢复安装、旧暂存读取、分支恢复自身更新的精确证据与迟到准入不倒退。
- App 组更新相关 12 项、Profile v6 迁移/回读 12 项通过；当前 v6 同存单一需求与本机移除私有材料，旧记录不补造证据。
- 首轮 P2P 同进程全库：297 通过、2 失败、2 忽略；失败为 subscriber span ID 克隆/释放，完整日志 `p2p-suite-final.log`。两例在 workspace nextest 隔离运行均通过，未删除失败记录。
- 首轮 workspace nextest：4180 执行，4178 通过、1 堆栈符号断言失败、1 五进程场景超时、23 跳过（`workspace-nextest-2.log`）。Linux 堆栈沿用现有 PR CI 的 available 模式，测试源码不放宽；五进程测试独占并发槽，保留原 60 秒期限。最终门禁执行中。
- 本机原 nextest 0.9.143 不符合仓库固定 0.9.145，已使用共享 target、保留 MBX 串行安装 0.9.145（`nextest-install.log`），没有临时完整构建目录或清空缓存。
- 分支恢复收件人本机 opaque V2 保留实际生成的 external commit，读取旧 V1 不补造证据；用实际 MLS epoch/exporter、密封包与 catalog 校验后记录确切 payload digest。网络 /1、/2、/4 未改变，成员格式仍只有最终 v6。
- 最终 workspace 门禁通过：4181 项通过、23 项跳过（`workspace-nextest-final.log`）；五进程场景 38.2 秒，原期限不变。之后补充的真实 SQLite 测试和本机目录分类修正另行复核。
- 完整成员组通过：56 项通过、14 项跳过（`membership-e2e-final.log`），G1–G4、长分区与拓扑均通过。14 项为原 ignored 场景；其中 Clipboard 独占场景另执行，其余显式配对性能/观测及专用 rendezvous 场景记录跳过。
- 新增真实 SQLite/MLS 集成回归 2 项通过（`sqlite-mls-restart-2.log`）：4 保存后真实 vault 安装失败不能成功，释放原仓储、会话和 vault 后重新打开数据库，重装 4、再重启后继续 5，重复 4 不倒退。首轮错误引用旧 Passphrase 类型导致编译失败，已改为领域类型；失败日志保留 `sqlite-mls-restart.log`。
- 最终审查发现确切重复包校验本机目录时误用 ValidateUpdate 分类，已改为 LoadState，并补本机损坏保留 source/不拒绝远端的回归。安全全套复核中；未以新分类放宽远端非法更新。
- 独立进程验收补为直接 kill B，不执行 Engine shutdown。首轮在 kill 前因只读诊断 1212 不可用中断（`gap-crash-final.log`）；源码契约将该查询的 Ledger/Security 失败映射为可重试 Unavailable。测试只在原 30 秒期限内重读该查询，不重试非法状态 1213，不扩大生产接口或等待期限；最终 kill 场景复核中。


## 最终交付与审查（2026-10-12）

- 范围冻结：成员历史仍为 /4 逐页交换，组更新仍为 /2、/1；未交付新批量版本、一次发送 bundle 或减少往返。
- 最终接线的宿主组 7 项通过、无跳过（host-assembly-final.log），包含未修改 main 的 /1、/2 双方向实际发送与 ACK、三进程拒绝恢复及五进程缺口/kill 重启恢复。
- 完整成员组 56 项通过、14 项原配置跳过；其中 8 项 Clipboard 在途、撤销、切换、持久工作收束与 trace 独占回归另行全通过（clipboard-regression-final.log）。剩余 6 项显式配对性能/观测与专用 rendezvous 场景跳过，不作为本次第三切片通过项。
- 安全全套 119 项通过（security-reviewed-final.log），包含审查修正后的本机目录损坏分类，保留完整 source。真实 SQLite 集成 2 项通过（sqlite-physical-crash.log）：内部子进程真实保存/安装后等待父进程 kill，未运行 Runtime/vault/仓储的 Drop；密钥只经匿名 stdin 管道传递，未写明文文件或输出。保存后安装失败、安装 4 未确认、重新打开并继续 5/重复 4 均有实际材料和 MLS epoch 断言。
- 最终工作区：4182 项通过、24 项默认跳过（workspace-assembly-final.log），实际耗时 257.8 秒。24 项中内部强杀子进程由父测试驱动执行，另有需要显式条件的原 ignored 用例。基准只编译、完整成员二进制另测、历史 upgrade-matrix 需其他旧锚点而未执行，不能将这三个过滤范围称为本次 workspace 行为通过。
- 最终工作区日志前缀混入一次只读 shim 失败及 NUL；原始文件保留。主进程退出 0、4182 项汇总与独立 JUnit 的 tests=4182/failures=0/errors=0 一致，JUnit 副本及结构化摘要保存在证据目录，不把该前缀称为真实测试失败或干净日志。
- 根门禁全部退出 0：locked metadata、workspace/all-targets check、fmt、Rust style、Engine repository preflight、diff check。直接 tracing 日志宏检查默认与 LAN 特性均通过，0 处；preflight 的 6 项 OpenMLS 可执行验收及全部负向 fixture 通过。命令与退出码见 root-gates-results.json。
- 架构门禁首轮发现新增第二个 membership 观测导出；同名原始日志被最终复跑覆盖，首轮会话工具输出转录保留于 architecture-first-failure-transcript.txt，明确不冒充原始日志。最终按既有规则合并至唯一 observe_membership，返回完整接收能力并在 Node 注册；未新增观测接口豁免、公开步骤查询或可见 decorator。修正后真实宿主、workspace 和架构门禁复核通过。
- 强杀场景首轮 ACK 等待遗漏了测试宿主事件映射，补齐后仍因最终发送状态等待而在 60 秒外层期限超时。定点诊断证明 B 已完成持久需求 ACK、kill 重启和 epoch 5 安装；C 的两次失败已按原投递规则排到 60 秒后重试。只将新增强杀场景的 nextest 外层期限设为 120 秒，覆盖准备、重启和已有退避，保持内部等待预算、实际 ACK 断言、G2 期限与故障次数。临时 DEBUG-GU-FINAL 输出已删除；最终该场景定向 44.8 秒、宿主全组 52.9 秒、workspace 复核通过。
- code-review 技能缺失，最终差异按仓库所有权、加密、格式迁移、错误来源、准入/世代围栏、取消收束及唯一生产接线约束自审。审查修正及回归均如上记录，没有用失败重试或忽略断言替代修复。
- 桌面产品与 iOS/Android/HarmonyOS 真机、跨网络切换、休眠及前后台矩阵跳过；Linux 本机 QUIC/SQLite/MLS 和独立进程通过不替代这些项目。完整历史发布升级矩阵、macOS 符号模式和发布打包跳过，本次没有发布操作。
- 长期事实回写成员历史职责文档，spec/plan 归档 completed；Issue #34 其余切片与批量优化保持后续任务。共享 target 和编译缓存保留，固定旧端 worktree 已回收。

## PR #179 的 CI 修正（2026-10-12）

- CI 的 Rust 风格检查按 PR 基线比较全部已提交差异，此前本地默认只比较 HEAD 与未提交工作区，漏掉 9 处正文完整路径和 1 个内部纯转调方法。用 `RUST_STYLE_BASE_SHA=c0351c0eb48ac87abb93d9fc3941bdee5dc3a005 node scripts/architecture/check-rust-style.mjs` 本地复现同一组 10 条错误。
- 集中引入连接类型与唯一 `observe_membership`；两个组更新能力入口显式持有同一技术互斥锁，内部应用实现要求持锁参数，保存与安装仍在同一锁范围内。不扩大公开接口，不改变协议或持久格式。
- 修正后安全模块全套 119 项、Application 完整装配测试 1 项及 workspace all-targets check 通过。按 PR 基线执行的 Rust 风格检查与 Engine repository preflight 均通过；日志分别为 ci-fix-security.log、ci-fix-application.log、ci-fix-workspace-check.log 和 ci-fix-repository.log。

### CI 独立进程缺口场景修正

- `2a8921ce` 的远端仓库规则、Windows x64/arm64、升级兼容、成员 smoke 和隔离网络 smoke 均通过；workspace 实际为 4181 项通过、1 项超时、24 项跳过。后续 smoke 设置为 `success() || failure()`，不能用进入后续步骤推断 workspace 成功。超时场景为独立进程缺前置与强杀重启，原始日志保留于 ci-fix-remote-engine-failed.log。
- 原场景本地 48.4 秒通过；受控历史暂停后随机端口重启出现真实目标已安装、发送待办转为不可达、尝试次数增加的失败（ci-gap-delivery-probe.log，169.5 秒）。暂停期间的预算实验也分别触发 120 秒与 240 秒外层超时，不能称为通过；仅延长期限不能解决地址变化。
- 宿主显式关闭 relay，而原接收方重启随机更换 UDP 端口，混入了地址重新发现的不确定性。本次只给该缺口场景的接收方固定重启端口，沿用既有 test bind port 配置和宿主启动通道；保持真实 kill/wait、SQLite 重开、原发送者离线、第三方 `/4`、实际 epoch 与 ACK 断言。真实网络地址变化仍由网络恢复矩阵验收，本场景不替代它。
- 相同 30 秒受控暂停，固定端口后恢复与实际 ACK 在 51.4 秒内完成（ci-gap-fixed-address.log），原 120 秒外层限制通过。最终删除所有临时暂停及 DEBUG-CI-GAP 标记，未修改 nextest 超时或生产退避；只保留固定阶段、耗时及公开发送摘要的测试诊断。
- 最终宿主组 5 项通过、2 项原 ignored 跳过，缺口场景 35.9 秒（ci-gap-final-suite.log）；固定旧端 `/1`、`/2` 双方向另行执行，2 项均通过（ci-gap-old-interop.log），没有将默认跳过记为通过。场景固定端口只使用既有测试配置，不新增生产能力、地址持久化例外或网络降级路径。

### CI 宿主资源隔离补充

- `23616c4c` 的缺口强杀重启场景已在 CI 中 40.7 秒通过。该轮 workspace 的唯一失败改为三进程拒绝场景：在重启前等待明确拒绝，实际仍为 retryable_failure；仓库检查、Windows 两架构、升级兼容、成员与隔离网络 smoke 均通过。完整日志为 ci-gap-latest-job.log。
- 三进程场景单跑 16.5 秒、两个 CPU 的串行对照 16.0 秒通过；与真实大历史场景并行时增至 32.9 秒，但未稳定复现 CI 的同一失败，因此不把资源竞争记为已完整证实的具体失败根因。两个 CPU 压力下的大历史激活回执测试超过其原 120 秒限制，压力组合不记为全通过，亦不为此修改 Core 测试期限。
- 原 process 组 max-threads=1 只排除同组并行，不能排除其它 Engine 与大历史组。为整个 space_device_update_recovery 宿主组预留全部 nextest 槽位，建立其真实交换预算所需的独占验收环境；保持原 10/30/60/120 秒各层期限和全部实际拒绝/ACK 断言。失败时另输出已查询的公开组更新摘要，区分不可达、暂不可用与拒绝，不输出设备标识。
- 本地宿主组连续三轮均通过，15 次执行、每轮 2 项原配置跳过（ci-three-isolated-suite.log，249.4 秒）。该调整最终仍需远端完整 workspace 验证，不能以压力对照中单个场景通过替代整轮 CI。
