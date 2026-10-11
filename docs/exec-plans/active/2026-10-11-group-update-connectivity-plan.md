# Issue #34 第三切片：组更新收发与主动恢复执行计划

状态：**实施中；已固定原 wire 并提取 codec，批量范围待选择，完整收发与恢复未完成**。基线：`origin/main c0351c0e`。
唯一设计来源：[实现规格](2026-10-11-group-update-connectivity-spec.md)；本文只记录执行顺序、
门禁与证据，不复制协议和状态规则。本文档交付不等于第三切片实现完成。

## 责任与调用

- 完整负责人：Application Space 成员模块，内部完整 receiver/apply coordinator 与既有 worker 协作。
- 调用方唯一动作：提交对目标的完整组更新发送，或将完整认证入站交换交 receiver。
- 成功：对应更新经过验证、保存和安装，真实成功 ACK 才结清发送 pending。
- 失败：结构化暂不可用、明确拒绝、带 source chain 的本地/传输失败，不以历史 ACK 代替成功。
- 重启/重试：同一成员 owner 从加密记录、材料与待完成效果恢复，原维护调度负责有界退避。

## 阶段 0：范围冻结

状态：进行中。

已核对当前 `/4` 历史后缀限制、组更新双版本、成员授权、材料保存/安装断点、当前 v5 成员
记录及第二切片 G2 证据。已确认的行为按规格保持。

退出条件：回答规格第 11 节的批量范围问题，并冻结其 wire 与回退策略；未确认前可以继续
独立测试/设计核对，不能实施新的历史协议。若选择延期，删除本切片批量交付承诺、明确后续项。

## 阶段 1：先固定失败与兼容事实

状态：待实施。

1. 从当前原 adapter 固定 `/2`、`/1` framing/trace/ACK vectors，不以重构后的 encoder 生成全部基准。
2. 写真实 MLS 的缺 4 收 5、同 epoch 非法 payload、保存后安装失败和 ACK 丢失回归，确认能暴露现有问题。
3. 梳理两条 apply 入口与成员 CAS，确定最小应用证据及最终格式清单；记录证据不足的安全恢复方式。
4. 固定当前 v1–v5 格式样本，核对旧材料启动和历史效果重放；不得先实现多个过渡格式。

退出条件：每个问题有可复现输入/预期/实际结果，成功定义不依赖 epoch 快捷路径或历史 ACK。
失败本身是本阶段证据，不能提前把最终验收标为通过。

## 阶段 2：协议提取和 Uni 能力

状态：待实施。

1. 机械提取 group_update codec 至 uc-sync-protocol，完成 byte-level 兼容测试。
2. 在 Infra 增加有限协议 profile 和实际协商能力，装配独立 GroupUpdate manager。
3. 双组版本固定 SingleExchange；覆盖同 peer Clipboard/GroupUpdate 隔离、排队取消及准入围栏。

退出条件：冻结向量通过，现有 Clipboard 回归通过；本阶段不启用不完整生产 receiver。

## 阶段 3：完整接收与统一应用

状态：待实施。

1. 新建成员内部 group_update 模块，按模型、port、流程、错误和测试拆文件。
2. 实现完整 receive/ACK 生命周期，身份与成员授权来自当前认证会话。
3. 统一直接入站和已验证历史效果的 apply 协调；区分缺前置、损坏、非法、可恢复安装错误。
4. 处理保存后未安装、已安装 ACK 丢失、撤销和 generation 切换，不遗弃 blocking local work。

退出条件：阶段 1 的问题得到行为证明，旧 `/1` 暂不可用关闭不产生明确拒绝。

## 阶段 4：持久主动恢复与调度

状态：待实施。

1. 将恢复需求并入原成员记录/CAS，复用真实历史流程与因果效果队列。
2. 必须新增字段时，统一最终 v6 加密格式和 v1–v5 迁移；验证损坏、generation 和旧材料证据。
3. 四类明确事件合并唤醒，原 scheduler 管理 2/4/8/16/30 秒退避，无任务无额外周期。
4. 明确拒绝保留阻断；正确历史恢复后由实际应用与成功 ACK 完成发送结清。

退出条件：真实 SQLite/MLS 连续恢复、部分进度、崩溃重启及 no-progress 退避测试通过。

## 阶段 5：批量范围分支

状态：等待阶段 0 的选择。

若选择扩展：先补齐规格中可执行的 wire、边界、去重、确认及协商定义，再实施现有历史通道
的有界批量能力；旧 `/4` 字节和页语义保留。至少两个缺失更新一次请求的真实证据必需，
批量失败和历史 ACK 先于安装的用例必需。

若选择延期：继续使用原逐页同步，验证主动恢复与可靠性；把减少往返登记为明确后续工作。
不得为了宣称 bundle 已完成而用冲突证据通道代替普通后缀。

## 阶段 6：生产接线与删除旧路径

状态：待实施。

1. Engine 只组装完整 owner/port；Node 注册 handler 并按会话 generation 管理关闭。
2. 删除被取代的组更新原生拨号、wire 和接收业务实现，不保留生产双轨。
3. 搜索所有组更新发送、入站注册及历史本地效果入口，确认实际生产调用均进入新 owner。
4. 观测沿用不透明上下文和 Engine 领域 port decorator；不扩 facade 作测试查询。

退出条件：真实 Engine 路径验证通过，关闭、超时、源错误链与跨协议隔离具备证据。

## 阶段 7：系统验收与归档

状态：待实施。

按规格第 8 节执行。G2 先证明真实篡改接收和纯联系不能解除拒绝，再释放正确历史恢复并
证明实际成功；不增加 tamper 次数或放宽时间预算掩盖问题。

| 项目 | 当前状态 | 实施退出条件 |
| --- | --- | --- |
| Codec、授权、分类、退避、协议隔离 | 未执行 | 相应单元/回归通过 |
| 真实 MLS/SQLite 部分进度和崩溃恢复 | 未执行 | 每个断点实测，不以 fake 成功替代 |
| Linux 独立进程真实网络 A/B/C | 未执行 | 完整主动恢复与结果证明 |
| 新端/固定 main 旧端双方向 `/2`、`/1` | 未执行 | 记录旧端 commit、二进制与实际协商 |
| 批量历史互通/减少往返 | 待范围决定 | 扩展则实测；延期则明确未交付 |
| G1–G4、确定性 G2 | 未执行 | 全套结果与隔离结果分别记录 |
| 第二切片 Clipboard 回归 | 未执行 | 多流/重启/准入保持 |
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
