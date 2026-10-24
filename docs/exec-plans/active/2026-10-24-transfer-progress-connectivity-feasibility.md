# 第八片阶段 0：受管流、线上合同与登记围栏证明

状态：**单向流、字节合同、登记围栏及输出后采用／ACK 主机链路已证明，阶段 0 整体尚未完成**（2026-10-24）。
需求见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)，推进见[执行计划](2026-10-24-transfer-progress-connectivity-plan.md)。
开工基线为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；本记录描述该基线上的候选变更，
各节分别说明主机探针与固定旧端独立进程基线的证据，不将它们当作完整生产迁移或真机验收。

## 能力缺口及最小改动

原 `ConnectionLease` 只有双向开流／接流。新增探针在原接口上得到 9 个缺少单向方法的编译错误；
这是新增能力缺失证据，不是已经运行的单向功能发生行为回归。

当前候选新增可选 `PeerUnidirectionalConnection` 能力；Iroh bridge 提供原生单向半流的内部包装，
双向 backend 可以明确不支持。`ConnectionLease` 共用准入和出站额度获取，单向租约也持有同一工作使用权。
旧 `/1` ALPN 常量由协议 crate 统一维护，旧帧编码及 golden 向量未变。
新协议类型在不相关组更新处理器中明确拒绝，没有用通配分支放行。

旧进度 reporter、其生产连接池、Engine 发送侧投影和 Router 尚未迁移；新增能力不代表这些任务已经完成。

## 真实主机探针及结果

探针位于 `crates/uc-infra-p2p/src/network/iroh/connectivity/stream_progress_probe.rs`。
使用同进程两个真实 Iroh endpoint、旧进度 ALPN、已建立连接和实际 Uni manager／成员身份准入；
不经过生产 Router，也不证明发现、Relay、独立进程或完整接收业务。

| 验证 | 已观察结果 |
| --- | --- |
| 同连接两条单向流，停止第一条 | 第二条旧进度帧精确读回，两端连接仍打开 |
| 停止网络但保留本地工作使用权 | 第三次开流在额度期限内失败；释放实际工作使用权后可再次开流 |
| 已有连接上的成员资格撤销 | 开流前重新读取权威并拒绝，不因连接已认证而继续发送 |
| 已取消的等待者 | 返回停止，接收端未收到新流 |
| 准入读取被受控暂停后取消 | 调用仍等待实际读取，额度不提前释放；读取结束后返回停止且未开流 |
| 入站额度满时接到另一流 | 仅停止超额流，原流可继续写入并完整读回 |

五个真实探针通过；其中首个同时验证两流隔离及实际工作额度，因此表中共有六个行为断言。
Application 另验证不支持单向流时有来源分类且不消耗单次双向交换资格。
原两个真实 SDK／Uni blob 流测试继续通过，覆盖同内容等待者及普通文件的取消隔离。

## 验证入口

从仓库根目录串行执行并复用既有 target／缓存：

```bash
just cargo test -p uc-application --lib 'connectivity::' --locked
just cargo test -p uc-infra-p2p --lib 'network::iroh::connectivity::stream::' --locked
just cargo test -p uc-sync-protocol --tests --locked
```

本轮结果：Application 连接测试 113／113；真实流测试 7／7（5 个新探针、2 个原 SDK 流回归）；
协议测试 60／60，含原 `/1` golden 向量。原始逐轮日志、源码／锁文件指纹与消费点清单保存在本地工件，
不把第一次编译缺口和后续成功结果混为同一运行。
本轮 metadata、workspace 全目标 check、fmt、Rust style、repository 和 diff 根门禁均通过；
workspace 检查有既有未使用导入／测试字段告警，没有新增告警作为通过条件的豁免。

## 新版线上合同的独立证明

在单向流 PR #235 之上补齐同一个未发布 `/2` 的编码、解码及边界；精确格式只在
[线上合同](../../design-docs/transfer-progress-protocol.md)和源码／向量维护，本记录不复制字节清单。
协议36条消息／状态／原因向量加1条观测载体向量，新增15项测试；包括全部截断前缀、精确大小、
非法选项、最大合法页、伪造计数、重复项、错误来源和身份／载体 Debug 脱敏。
原60项保持，最终协议全集75／75及固定错误分类检查通过，六根门禁通过。未接入生产 Router，没有更改旧 `/1` 字节、blob 或 V3 格式。

## 登记围栏的真实双端原型

Core `file_transfer/progress_registration.rs` 保存私有登记状态，通过唯一 `apply` 入口返回稳定效果；
无网络、时钟、随机数或持久化，状态空间固定，不保留无界的历史挑战或 producer 列表。
Infra 原生连接已识别 `/2` 为多流协议，仍未把新版注册到生产 Router，也未迁移旧 reporter。

`stream_registration_probe.rs` 使用真实双端 Iroh、当前字节 codec、受管双向流及同一 Core 规则。
复用旧探针的真实成员／身份准入 fixture，按探针用途继续使用 Clipboard admission profile；进度专属 profile 未接线。
以保留旧流并推迟写入／读取控制乱序，不依赖睡眠概率：

| 场景 | 主机结果 |
| --- | --- |
| 新 attempt 先登记，旧描述随后到达 | 当前挑战绑定成功；旧挑战拒收；旧 attempt 的进度及终态均不能匹配新绑定 |
| 同两个 Endpoint 物理重连，保持认证身份 | 复用同一 Core 围栏，较低修订拒收，原修订可重新绑定 |
| 同一连接自报新 producer | 拒收，不能按较小／较大 UUID 推断新旧 |
| 同身份的新运行实例模拟 | 在第三条真实连接完成新挑战，允许新 producer 的较小起始修订；保留旧连接实际送达的旧登记和终态均拒收 |

Core 另验证同修订描述冲突、错误源条目、空计划、旧 binding／attempt／修订及重复描述拒收。
Application catalog 测试通过既有 V3 编解码及真实 `ReceiveSource::planned`，核对混合表示／文件的原始位置、
同名同 ticket 项不合并、不同本地接收条目／attempt 隔离及目录只生成完整单元；未改 V3 字节或 blob ticket。

验证入口：

```bash
just cargo test -p uc-core --lib progress_registration --locked
just cargo test -p uc-application --lib 'transfer::receive::work::catalog::tests::' --locked
just cargo test -p uc-infra-p2p --lib 'network::iroh::connectivity::stream::' --locked
```

本层相关回归23／23通过：Core 围栏3项、Application catalog 11项、真实流9项（含2个新登记探针及7个原流回归）。
metadata、workspace 全目标 check、fmt、Rust style、repository 与 diff 六根门禁通过。
`ReceiverAuthority` 是测试注入的当前描述，不是生产接收 attempt 数据源；运行实例重启通过替换 producer 模拟，
没有实际杀进程。V3 计划关联和网络登记分别验证，尚未组成生产接收 owner→登记的完整链路。
这层单独原型不能证明接收权威已连接；后续组合证明见下文，不能将主机结果冒充生产迁移或完整 AC02。

## 真实输出后的 Application 采用与 ACK

`transfer/progress` 为一个已接受的发送意图保存认证设备、源条目及不可变原始逻辑计划；
登记与终态采用使用同一个 Core 围栏。身份与计划不由第一条远端进度授权，快照可在等待时读取。
Core 保存不可变终态；精确重复允许再次 ACK 但不增加投影版本，冲突不覆盖、不产生 ACK。
目录计划仅有完整单元，单计划数量上限由 Core 与 codec 共用同一常量。

`transfer_progress_v2` 入站适配只负责有界字节交换，等待 Application 成功返回后才能写业务响应。
每次 IO 前等待实际准入读取结束，再核对原截止时间与取消；已开始的业务采用不因网络超时／取消被丢弃，
同一 StreamLease 持有许可直到实际采用及响应处理结束。

`stream_terminal_probe.rs` 在同两个真实 Endpoint 上分别使用 blob 与 `/2` 受管连接：

| 检查点 | 证据 |
| --- | --- |
| 实际接收与输出 | 固定 SDK 传输 256KiB 文件内容，BAO 校验接收，导出目标文件后逐字节读回；文件本体路径不适用额外内容解密 |
| FIN 到达而 Application 采用暂停 | 快照无终态，真实响应读流未收到 ACK；FIN 仅证明传输结束 |
| 采用完成 | 完整快照可读终态，然后返回绑定、scope、owner 修订和完整结果精确一致的 ACK |
| 重复与冲突 | 精确重复再次 ACK，投影版本不增长；冲突拒收、无 ACK，文件与既有结果保留 |
| 取消／原网络期限到达 | 实际采用继续持有租约，完成后不发送过期 ACK；结果保留，后续同结果交换仍可确认 |
| 帧后的额外字节 | 在采用前拒收，不先确认再检查尾随输入 |

验证入口：

```bash
just cargo test -p uc-core --lib 'file_transfer::' --locked
just cargo test -p uc-application --lib 'transfer::progress::' --locked
just cargo test -p uc-infra-p2p --lib 'network::iroh::connectivity::stream::' --locked
just cargo test -p uc-infra-p2p --lib 'transfer_progress_v2::mapping::' --locked
just cargo test -p uc-sync-protocol --tests --locked
```

本轮105项相关回归通过：Core 12项、Application 5项、真实流12项、原因／字段映射1项及协议75项。
原始协议／流回归仍执行，未以单个新增探针代替。metadata、workspace 全目标 check、fmt、Rust style、repository 与 diff 六根门禁全部通过。

该证明闭合阶段 0 的“输出后采用 ACK”最小能力，不代表生产接收 attempt／SQLite 提交交接、
最终通知调度、独立进程、进度专属 profile／Router 或完整阶段 3 已完成。
登记当前描述仍由测试响应提供，真实接收权威及真实进程重启继续保持未验收。

## 实际接收权威交接与进程寿命

接收负责人在持久 CAS 返回 `Begun` 后调用 `ReceiveAction::accept_source`，向 progress 模块交接
认证来源设备、源条目、当前 attempt 及不可变原始位置。预备计划不产生当前描述；拒绝 CAS 不产生映射。
同一个 attempt 只交接一次，已完成或仍活动的旧对象重复交接都不能替换后来接受的同源尝试。
本机失败回退所保留的旧重试来源，不重写当前接收意图的反向登记身份。

新激活拉取链路已有发送条目标识，但此前在 store/apply 交界丢失；现在保留该认证提示，空目录无需从不存在的 blob 引用猜测身份。
旧 bulk 没有提示时，只在全部 blob 引用指向一致、可表示的源条目时建立关联；不以内容摘要、名称或 provider 身份合并来源。
无法可靠关联只退休旧进度映射，不删除接收来源、不阻塞实际文件下载或本地结算。
V3／blob 字节、ticket 和既有持久化字段不变。

`ProgressSourceCatalog` 只保存身份和原始位置：全局256／每来源设备64个当前任务，超量淘汰最早映射；
重复交接不刷新 owner revision，新用户尝试推进修订，普通网络重建不创建新 producer。
同条目的删除、清空及会话封口清除映射；清空保留运行期修订，会话退休生成新 producer。
完整保存后可以释放原始内容和重试来源，登记元数据在自身有界生命周期内保留。

证据包括：

- 实际接收 use case 受控暂停持久 CAS：返回前没有关联，`Begun` 后、物化开始前已有与已接受 attempt 相同的描述；`Superseded` 不产生描述。
- 原始混合文件／表示位置、空目录、同源新旧 attempt、旧活动对象重复交接、错误来源提示、旧 bulk 的一致／歧义来源及删除围栏。
- per-peer／global 容量及淘汰；相同 attempt 幂等，不按 UUID 大小判定新旧。
- 实际启动两个 Application 测试进程，父进程逐次 `kill`／`wait` 后重启：首次查询都未保留旧关联，接受后 producer 不同。
  临时文件仅回传本测试的合成运行身份与断言标记，没有生产持久化或用户数据。

本轮 Application 全集1295项通过；另一个必须独占日志捕获的既有用例在独立进程精确运行通过，合计1296项。
真实流12项与协议75项回归通过；metadata、workspace 全目标 check、fmt、Rust style、repository 与 diff 六根门禁通过。

该进程证据证明 Application 登记资料的寿命，不冒充完整 Engine／SQLite profile 重开矩阵。
该层完成接收交接和进程寿命的独立证明；它们与 Iroh 的组合由下文独立入口验证。

## 实际当前权威与真实登记的组合

Application `TransferProgressProducer::register_current` 负责原五秒期限内的准入连接、开流与完整登记。
明确协商旧 `/1` 才返回 Unsupported；新协议损坏、网络或当前准入失败保留原错误，不转换成能力不支持。
Infra 在新 Challenge 到达后实时读取 `ProgressTaskAuthorityPort`；收到 Bound 时核对完整挑战与描述，
Application 等实际连接保留读取结束后再次检查当前 attempt／producer／revision，迟到 Bound 不直接激活旧任务。
借用的 StreamLease 由 Application 持有至这些实际工作结束，取消或期限到达后不提前归还原额度。

组合入口为 `scripts/testing/run-progress-registration-proof.mjs`。它串行构建当前 Application 与 Infra 测试二进制，
从 Cargo JSON 精确取得产物路径，先核对必需用例均存在，再显式执行，避免零用例退出成功。
报告记录源码提交／工作区状态、两个二进制 SHA-256 和必需用例清单，位于
`target/test-artifacts/ci/progress-registration/report.json`；CI 的 Engine tests job 必需执行此入口。

真实 Iroh 双端每次新挑战通过测试 IPC 读取独立 Application fixture 进程里的实际接收权威。
该 fixture 执行真实接收 use case 与 CAS 后交接，存储 port 受控；IPC 只传递合成身份、位置及控制数据，
没有另建静态 descriptor 来替代原接收负责人。

| 必需场景 | 组合结果 |
| --- | --- |
| 普通文件及无 blob 的空目录 | 线上 Bound 与实际接受的 attempt／原始逻辑计划一致 |
| 预检后、Challenge 前替换 attempt | Challenge 实时读取后来接受的描述，不使用拨号前快照 |
| 描述发出后、Bound 返回前替换 attempt | Application 后置检查返回 Stale，不激活旧绑定 |
| 保留旧流描述，新 attempt 先完成登记 | 旧描述拒收，当前投影保留新绑定；旧终态没有 ACK |
| 真正 kill／wait 接收 fixture 并重启，重建实际 QUIC 连接 | 新 producer 可从修订1开始，旧连接未完成描述和旧 producer 终态均退休 |
| 明确旧 ALPN 与损坏的新帧 | 前者 Unsupported；后者保留 Exchange 错误及 source，不降级 |

最终候选组合证明4／4通过；Application progress 8项、原流12项、映射1项及协议75项回归通过，合计100项。
原流测试中的4个专用ignore用例由driver另行显式执行，不将ignore本身记为通过；六项仓库交付门禁通过。

该组合闭合阶段 0 的登记顺序与原始项／attempt 关联可行性，覆盖真实进程寿命及迟到旧流。
它没有证明真实 SQLite 提交／profile 重开、完整 Engine 生产 Router、Relay、三端或设备矩阵；这些仍按后续阶段验收。

## 固定旧端旧旧文件与旧进度向量

切片 8 的开工前 main 固定为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；历史锚点沿用
`a07`，Engine `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6`。后续把开发 stack 接到新 main，
不改变这两个兼容基线。两个修订的旧进度 codec 源文件逐字节相同，SHA-256 为
`4f8130692441496a436be085f09691cd60236b9c3cc509803a56c504c6d61d0b`。
`transfer_progress_legacy_vectors` 在当前协议 crate 上双向验证 9 种状态 × 已知／未知总量的
18 条字面 golden 向量；原 `/1` 帧、blob、V3 与 ticket 字节没有修改。

从仓库根目录运行：

```bash
just cargo test -p uc-sync-protocol --tests --locked
bash scripts/testing/run-progress-legacy-baseline.sh
```

无 sudo 的 Linux 可使用既有 `UC_NETWORK_UNPRIVILEGED=1` 用户／网络 namespace 入口。
驱动复用固定 main 与历史锚点构建器，只替换测试宿主，不补丁旧 Engine 库；显式复用仓库 target。
每次运行创建独立工件目录，保留固定源／源码 tree／锁文件及驱动输入摘要、宿主构建输入、二进制摘要和原始用例报告。
既有网络 harness 的 `--old-pair-only` 要求同一个固定旧宿主用于两端，旧库完成正常配对、关闭重开和
双向文本／文件读回后结束，不进入单端升级。不会用新端能力补齐旧端结果。

| 固定两端 | Direct | 强制 Relay |
| --- | --- | --- |
| 开工前 main × 同一 main | 通过 | 通过 |
| a07 × 同一 a07 | 通过 | 通过 |

四格均实际重开旧资料、保持原传输身份并逐字节读回双向文件，报告核对两端真实宿主修订和 selected path。
两格 Relay 在两端实际命中 IPv4／IPv6 UDP 丢弃规则；资料明文探针及网络／进程清理通过。
该基线没有执行新版进度协议、终态 ACK 或查询，不声明历史端具备新版 ALPN 观测能力；
新旧双角色和新新完整进度矩阵仍留在阶段 5。

协议全集76项通过；旧旧4格通过。CI 在原必需 blob matrix job 中先执行此入口，
`target/progress-legacy-evidence/runs/` 的 JSON／文本／日志随原 artifact 上传。远端新候选 CI 尚待执行。

## 补发调度与容量规则的极限输入

`uc-core::file_transfer::progress_delivery` 提供纯规则：`TerminalRetransmit`（0／2／10／30／60／120／300／540 秒串行补发，
单轮截止 `min(start+5s, t0+600s)`，挂起恢复合并已过时点且不延长保留，网络恢复至多提前消费一个时点）、
`TerminalLedger`（全局 256／每端 64，先淘汰同端最早，再淘汰全局最早，重复键不占容量）、
`ExchangeQueue`（在途全局 4／每端 1，等待全局 64／每端 16，按对端轮转）。时间读数由调用方注入；补发状态只通过 `RetransmitInput` → `apply` 推进，返回 `RetransmitEffect`。
预算逐次核对观察读数，不可用、任一次倒退或已到期后永久退休；重新获得有效读数不能复活旧预算。
12 项逻辑测试覆盖：1024 项清单只保留同端最新 64 项且不影响他端、5×64 全局超额淘汰最早、一个永不结束的对端只占自己的在途额度与队列、
挂起跳过 5 个时点只补一轮、恰好过期、8 次耗尽、恢复机会合并，以及等待期间观察回退和不可表示的绝对截止。

协议侧最大合法页、帧上限和 1024 项描述沿用 `transfer_progress_v2` 既有用例，没有新增线上字节。
这些只是纯规则与注入时间的证据，**未**接入 Application 调度，也**未**用真实消息／文件负载、真实网络或资源采样验证；
spec §5.6 的起始策略因此仍是起始值，不记为冻结。

## 完整单轮终态通知与原预算

Application `TransferProgressProducer::report_settled` 接收认证目标、源条目、产生结果的原 attempt 及已结算结果。
它统一负责连接、实时登记、终态与精确 ACK；不能只接收源条目后给旧结果重新选择一个新 attempt。
登记前、Bound 后、终态开流后及 ACK 后均核对当前权威，逻辑项必须属于原计划；
收到的 ACK 必须完整回显输入终态并精确结束，错误回显或尾随字节不报告成功。
通知失败不回滚实际文件输出；本动作只执行一轮，不自建重试或持久化 outbox。

`ProgressExchangeDeadline` 将包含挂起的经过时间与原受管等待截止组合；两条流共用同一对象，登记不会为终态重建五秒。
原生字节 IO 每次重新 poll 前检查预算，并在返回后再检查；准入及保留读取仍 await 到实际结束，
同一流在本机工作及后置检查结束前持有许可。系统时钟 port 和错误随消费者归入 Application progress，
纯 `ElapsedBudget` 归入 Core 进度规则；Infra-local 实现该能力，没有新 Engine 查询／步骤接口。

新增真实 Iroh 双端覆盖：

- 实际 BAO 接收 256KiB 内容、导出并精确读回后，由一个 Application 动作完成两条流与采用后的 ACK；最终快照和本地文件一致。
- 旧 attempt 在开流前拒收；预检后、Challenge 期间切到新 attempt，当前绑定保留新描述，旧结果不发送。
- 对端真实采用后，故意返回改过结果修订的 ACK 或带额外字节的 ACK，本动作拒绝成功且不回滚已经采用的事实。
- 注入登记消耗4秒、终态前到第6秒：原预算返回过期，未收到合法终态帧且未采用。已经打开后被重置的空流不当作业务终态。

App 的逻辑时间跳变测试验证 pending 网络 future 再次被 poll 前先拒绝旧预算；不可读时保留错误来源，之后读数恢复仍不能复活该预算。
这些注入读数不证明物理系统挂起；暂停／恢复的生产生命周期和设备矩阵仍按后续阶段分别验收。
原独立 Application 接收 fixture 组合入口增加必需第五项：真实 use case／CAS 后的当前 attempt 通过完整动作上报受控物化失败，
发送侧采用后才返回精确 ACK。该 fixture 使用受控存储 port，不冒充真实 SQLite 提交或完整 Engine 结算。

复跑沿用本记录的 Application progress、真实流及协议入口，并运行
`scripts/testing/run-progress-registration-proof.mjs`。完整有限补发、公平资源调度、生产 Router 与实际接收结算接线仍未完成。

## 尚未证明的范围

- 当前权威与真实双端登记的组合已完成；Engine／SQLite profile 重开及生产 Router 的完整迁移仍未完成。
- 输出后 Application 采用／ACK 的最小主机链路已有证据；生产接收输出／SQLite 结算交接、有限补发和状态核对尚未接线。
- 新旧双角色 Direct／Relay、真实消息与文件负载下的资源占用尚未执行。
- 包含挂起的经过时间：Application progress 的 `ElapsedClockPort` 与 Core `ElapsedBudget` 固定预算不可重置；每次观察通过 `apply` 推进，起点／当前读数不可用、相邻观察回退、已到期或截止溢出后永久按耗尽处理；
  `uc-infra-local::SystemElapsedClock` 在 Linux／Android／HarmonyOS 读 `CLOCK_BOOTTIME`，macOS／iOS 读 `CLOCK_MONOTONIC`（手册规定睡眠期间继续递增），
  Windows 读 `GetTickCount64`，其余平台返回明确不支持的来源错误。时钟 port 的 `Result` 保留系统调用、整数转换或非法计数的具体来源，不将读取失败降成无来源的 `None`。
  预算用模拟挂起跳变的读数测试，Linux 真实时钟的非递减与线程等待推进已运行；
  `x86_64-pc-windows-gnu` 与 `aarch64-unknown-linux-gnu` 仅通过编译检查。
  真实系统挂起、Apple、Android、HarmonyOS 及 Windows 运行时尚未验证，不记通过。Core 当前全集356项及 Linux 时钟4项通过，后者包含真实无效 clock ID 的 errno／source 校验；Windows GNU lib 编译通过。
  单轮原生收发已接入同一经过时间预算；60 秒投影等待与10分钟终态补发调度的完整接线仍属后续任务。
- 完整 workspace 行为回归、平台编译与设备矩阵仍按正式计划执行；本片尚未整体完成。


### 有界终态通知监督原型

Application 新增一次原子领取通知意图与唯一 worker：终态保留 256／64、交换等待 64／16，
当前每端仅一个候选，实际在途 4／1；Core 检查资格和实际启动分开，等待额度不消费时点。
原接收权威的有界逐项领取标记阻止缓存淘汰后重复输入重新起预算；没有新增持久化。
worker 持有全部单轮 future，取消／淘汰后等真实完成才归还额度；原五秒绝对截止贯穿登记、终态和 ACK。
共享经过时间连续性世代阻止一轮读到故障后其他旧轮恢复发送。

实际验证包括 1024 项输入、100 个候选对端、超额淘汰、原八时点、关闭等待和时钟故障；
共用本机 Endpoint 的两个真实已认证对端中，一端采用卡住时另一端完成精确 ACK。
独立 Application fixture 进程的真实 CAS／失败结算也通过同一监督入口领取并投递，重复领取被拒绝。
具体结果以本层验证日志和 PR 为准。该能力尚未接生产 Router／结算 hook；最大原始清单分页、
生产全局资源采样与物理挂起仍未验收，阶段 0 和整片任务计数维持原状。
