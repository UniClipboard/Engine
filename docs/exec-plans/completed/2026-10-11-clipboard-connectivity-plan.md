# Issue #34 第二切片：普通剪贴板连接边界执行计划

状态：完成。生产切换、审查修复、独立进程矩阵及根交付检查已完成；未执行平台/历史发布矩阵明确跳过。最新全套 4225 通过、1 项成员 G2 失败；隔离复核通过，未证实原因的风险保持待查。

基线：`main` / `c2a09cb75bdac966d37eb1d12ec90f6947e5f170`（PR #176）。已确认的需求见[实现规格](2026-10-11-clipboard-connectivity-spec.md)，当前架构正文见[普通剪贴板连接与独立交换](../../design-docs/clipboard-connectivity.md)。本计划保存执行事实，不复制长期契约。

## 完整负责人和调用方

- Application `ConnectivityManager` 负责获取、共享拨号、模式分配、流额度、等待、取消和关闭；Infra IrohBackend 提供原生能力。
- 调用方唯一动作仍为 `ClipboardDispatchPort::dispatch(target, header, payload)`；不选择发现、地址或传输，不拼装准入步骤。
- 成员权威、唯一 AdmittedConnections 和 session generation 保持原负责人；manager 通过专属准入 port 使用它们，不建立第二份授权表。
- 成功仍是原 DispatchReport 与持久接受/去重 receipt；系统写入为原异步续工作。失败保留 source，连接层不重发内容。
- 重试、持久投递记录和进程重启恢复仍归原 clipboard owner；连接和 flight 只在内存中，重启不恢复它们。

## 阶段记录

1. **基线与行为红灯：完成。** 构建并冻结修改前 host/relay 二进制、源码 revision 与 SHA-256；实际执行 direct E02 文本/文件和 relay E10 原恢复断言。旧 /1 handler 同连接两条流，第二份无法在第一份 receipt 结算前交接，记录真实行为红灯，随后 /2 通过。没有把新符号缺失算红灯。原 Node 撤销套件没有单独在修改前运行；它们在迁移后的定向/全套中复跑，不能冒充基线结果。领头取消的旧实现缺陷来自源码与原有回归盘点，未取得独立旧二进制红灯。
2. **Uni 契约与准入：完成。** 按 admission/backend/acquisition/connection/manager/stream/error/tests 拆分，Core 规则与稳定 Engine 入口不变。原生类型仅在 bridge。未移交连接的 RAII guard 覆盖准入失败、取消和 panic；请求冻结自己的 epoch，等待后重检。
3. **生产迁移与 ALPN：完成。** 发送、接收全部注入 manager；生产 Router 优先协商 /2，/1 每连接独占一次交换，/0 仍拒绝。两个 ALPN 均纳入原 generation 和开发拒绝拨号入口。帧、ACK、密文和数据库格式不变。
4. **共享拨号、多流和背压：完成。** 独立等待者、公平有限额度与绝对 deadline 生效；最后等待者退出会取消并 join。旧模式跟随者有界独占拨号。多流在原任务的 FuturesUnordered 内轮询，不引入丢失 task-local 的逐流 spawn。接受 future pin 到完成/超时，慢 ACK 和准入等待回收额度，空闲 peer 不占全局活动额度。
5. **围栏、失败与关联：完成。** 真实生产 Node /2 双流撤销和 quiesce；真实 Engine/SQLite/MLS 在途发送撤销、发送方切 Space、接收方旧续工作排空；endpoint/build count 不变。两个不同 traceparent 和不同 ACK 检测串线。既有五项真实 Engine trace 验收发现并修复系统写入和 resend 的不透明关联丢失；不新增 facade 或业务阶段查询。
6. **删除与回写：完成。** 删除 `iroh/clipboard_dispatch_adapter.rs`、`clipboard_receiver_adapter.rs` 及旧 fixture constructor；原测试迁移。纯 trace helper 移到 `network/trace_context.rs`。保留其他协议仍消费的 connect、唯一登记表、generation、发现和 provider。同步当前设计、导航和架构维护记录。
7. **最终交付：完成。** 最终 host/relay 构建和 14 个独立进程矩阵单元全部通过；workspace 4225 通过、1 失败与隔离复核均已记录，根门禁通过；本计划、规格及[完整审查记录](2026-10-11-clipboard-connectivity-review.md)已归档。

## 验证与证据

匿名工件位于忽略的 `target/clipboard-connectivity-evidence/`，不提交业务内容、资料目录或密钥。基线二进制独立保存在 `baseline/`，最终二进制保存在 `final/`；manifest 记录 SHA-256 与源码基线。Cargo 串行经 `just cargo` 使用共享 target/MBX。

| 规格 | 实际场景与证明 | 当前结果 |
| --- | --- | --- |
| U1/U2 | manager 独立等待者、领头取消、最后取消 join、各自 deadline | 定向通过；全套中相关用例通过；G2 失败另记 |
| U3/U4 | 小额度/等待队列、Busy、过期 deadline、流 Drop、SingleExchange 二次开流拒绝；空闲 peer、挂起 ACK/准入回收后健康流继续 | 定向通过；全套中相关用例通过；G2 失败另记 |
| U5/U6 | adopt 前撤销、旧请求借新 epoch、额度排队后撤销、保留 backend 引用时取消关闭、shutdown 发布竞态、panic source 与重复 shutdown、Io source/公开错误脱敏 | 已有用例定向通过；最后新增两项在全套中通过 |
| U7 | 双工测试 backend 执行同一 dispatch/receive 代码并结算 Duplicate receipt | 定向通过；只证明可替换边界 |
| I1/I2/I7 | 生产 ProtocolRouter 实际 /2 优先与 /1 选择、原 codec/golden、旧模式独占与真实旧基线二进制互通 | native/codec 定向通过；最终网络 14/14 单元通过 |
| I3/I4/I9 | 同一 native connection 两流、先第二 Accepted 后第一 Duplicate；坏帧与单流取消不误伤；独立远端 parent、健康后续接收 | 定向通过；全套中相关用例通过；G2 失败另记 |
| I5/I6 | 生产 Node /2 双流移除/换代；Engine 发送方在途取消；接收方实际 Application 关闭等待已持久接受的系统写入续工作 | Node 定向通过；3 项 Engine 独占进程验收通过 |
| I8/观测 | 真实 SQLite/MLS 内容读回，取消不撤销远端提交；复制、显式发送/重发、系统写入失败/慢写和多目标部分投递关联 | 5 项既有 Engine trace 验收通过；业务全套中相关用例通过；G2 失败另记 |
| 修改前 direct E02 / relay E10 | `c2a09cb7` 独立宿主、双向文本/文件读回、原故障与恢复断言 | 通过，`baseline/` |
| 最终新新 | direct E02/E14/E04；relay-only E02/E14/E10 | 通过；IPv4/IPv6 直连阻断、持久内容读回、恢复与清理 |
| 最终新旧/旧新 | 旧宿主 side 0/1 × direct/relay-only × E02/E14 | 通过，8 单元；真实 /1，不能当已发布版本互通 |
| 物理 Desktop/Mobile、切网/睡眠 | 实体设备/移动宿主 | 跳过；本次 Linux namespace 场景不能替代 |
| 已发布版本与发布资产 | 发布/平台验收 | 跳过；本切片没有发布操作 |

定向回归曾通过 132 项（1678 项被筛选未执行）；工件为 `targeted-final-nextest.log`。覆盖 manager、clipboard、生产 Node/Router、拒绝诊断、身份唯一性、发现和全部 sync-protocol。最后新增的排队撤销与保留引用取消用例不计入该旧次数。

真实 Engine 独占进程验收命令：

```bash
just cargo nextest run --workspace --all-targets --locked --features uc-engine/dev-tools \
  --run-ignored only \
  -E 'package(uc-engine) & binary(space_membership_auto_pairing_e2e) & (test(clipboard_connectivity::)|test(clipboard_trace::))' --no-fail-fast
```

最终修复后结果：`Summary [  14.697s] 8 tests run: 8 passed, 61 skipped`；`engine-cutover-final-nextest.log`。原 8 项 / 45.140 秒日志另保留。这些用例在默认全套中 ignored，以上精确执行的结果单独计入验收。

最终网络矩阵使用 `run_matrix.py` 的 14 单元列表，每单元独立 `unshare -Urnm`、私有 `/run` 与 loopback，最大并发 2。重放的实际 harness 命令形状：

```bash
node scripts/testing/connection-recovery-network.mjs \
  --host target/clipboard-connectivity-evidence/final/uc-connectivity-host \
  --repeat 1 --mode direct --case E14 --evidence <该单元目录>
```

relay 单元另外提供 `--relay <最终 relay 二进制>`；混合单元提供 `--peer-host <基线 host>`、`--peer-side 0|1`。最终 14/14 单元均通过，汇总 `final/matrix.json`，各单元 `.log` 与匿名时间线保留。原 E04/E10 断言未放宽。E14 通过既有稳定发送操作并发发出四份内容并读回全部负载。有限额度、取消和满载错误由受控合同/native/Engine 围栏测试证明，四份健康发送不冒充全部故障证明。

## 全套首次执行的失败与复核范围

首次直接运行所有 all-targets 得到 4300 项：4230 通过、69 失败、1 超时、35 跳过（`workspace-nextest.log`）。其中 66 个升级矩阵用例报告 `anchor-host-missing`；这些需要独立构建历史版本宿主，不能计为产品通过。本切片批准范围中的发布旧版本矩阵保持跳过。原 `scripts/testing/run-test-group.sh` workspace 分组本就排除 bench 和 upgrade-matrix，基准只检查编译。

其余四项逐项复核：selected path 诊断发现本机 fake-IP 按生产规则应为 Unknown，用例改为真实 loopback 并同步实际 selected 发布；重复退出同二进制直接执行 35.06 秒通过，仅设置有限 60 秒期限；进程中断文件恢复两次失败，定位并修复活动旧连接与同条内容尝试写入围栏；堆栈沿用已有 Linux PR CI 的 `UC_EXPECT_DIAGNOSTIC_STACK=available`，测试源码不放宽。首次日志保留。

中间全套 4225 项：4223 通过、1 失败、1 超时、35 跳过，耗时 541.138 秒（`before-liveness-workspace-scoped-nextest.log`）。崩溃文件恢复已通过 26.307 秒；新增 native 回归错误要求关闭后的旧 ACK 通道仍存活，修正测试假设；正常重启文件恢复被默认 20 秒截断，同二进制直接执行 21.077 秒通过，为该项设置有限 60 秒。最终 context 复审又确认原游标积压与准入暂停旧 flight 的两个边界；两项真实行为红灯在 `liveness-boundaries-red.log`，修复后 2/2 通过，`liveness-boundaries-green.log`。随后 12 总执行槽全套为 4226 项：4225 通过、1 失败、35 跳过，276.151 秒（`concurrent12-workspace-scoped-nextest.log`），唯一失败是既有待确认入群用例的 RefreshPeerConnections `scope_timeout` / 1382。当时同一源码隔离执行该项 22.799 秒通过（`before-admission-sync-pending-confirmation-isolated.log`），4 个执行槽的全套仍复现同样 scope 超时（541.215 秒，`before-admission-sync-workspace-scoped-nextest.log`）。确认 fixture 重启后未同步冷账本加载；追加既有公开诊断等待两名正式成员恢复，再执行原 Refresh expect，确认暂停和禁止提前发布断言保持。修复后隔离场景 26.425 秒通过（`pending-confirmation-isolated-after-sync.log`），最新全套该项 26.783 秒通过，成员流程及原一秒 scope 预算没有改动。最终全套采用已有测试组的并发限制与 4 个总执行槽，下面结果才是最终源码验证。

最新完整运行的唯一失败为既有 G2 密码学拒绝场景：预期拒绝投影为 None；同一源码隔离 27.183 秒通过，前两次全套也通过。篡改一次性消费及并行历史同步存在源码可见竞态风险，日志尚不足以确认本轮原因。未更改该场景或组更新生产路径，不把它标为已修复；负责人、后续证据与退出条件见审查第 36 项及技术债。

复验结果：`Summary [ 560.072s] 4226 tests run: 4225 passed (9 slow), 1 failed, 35 skipped`（`workspace-scoped-nextest.log`）。全套存在一项失败，不能计为全部通过；覆盖成员多设备测试，范围比原 workspace 分组更广；仅排除 bench 与需历史产物的升级包。最终全套重新执行重复退出、文件崩溃恢复和路径用例；Linux 堆栈按现有 CI available 配置执行。首次失败未删除。

## 最终交付检查

根门禁全部通过，结果保存于 `root-verification.json` 和各同名 `.log`：

```bash
UC_EXPECT_DIAGNOSTIC_STACK=available just cargo nextest run --workspace --all-targets --locked --features uc-engine/dev-tools -E 'not kind(bench) & not package(uc-upgrade-matrix)' --test-threads 4 --no-fail-fast
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

另检查新增/修改 Markdown 相对链接、旧源码路径仅剩实施历史引用、生产 clipboard 原生依赖命中为零。审查发现及处理全部以编号保存在审查记录；shutdown 发布竞态回归修复前也通过，发现依据是静态时序审查，不虚构红灯。

## 相对已确认规格的实施组织调整

- 消息发送前最多更新一次已确认失活的连接；准入等待和迟到共同结果保留原快照、deadline 和有限额度，不重发写出的消息。新增两个 native 回归先红后绿。
- 每流并发采用原 generation task 内 FuturesUnordered，保持显式单负责人，避免为任务传播另建 port；行为保持批准契约。
- 入站有限连接处理槽为 128，包括拒绝 ACK 等待；全局活动交换 16、每 peer 4，出站等待每 peer 32/全局 128、flight 64。没有空闲池或消息重发。
- 接收方换代用真实宿主系统写入 gate 阻塞已持久接受的续工作，并同步到实际 Application shutdown；不用 SQLite 写锁猜测 drain，因为它会先阻塞 JoinSpace 自身的准入提交。
- 没有业务格式变更或新增明文持久字段；稳定 Rust facade/移动绑定不变。物理峰值内存和移动网络表现没有验收，保持平台跳过项。
