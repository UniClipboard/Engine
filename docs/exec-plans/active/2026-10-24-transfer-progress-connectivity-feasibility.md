# 第八片阶段 0：受管流、线上合同与登记围栏证明

状态：**单向流、字节合同、登记围栏及输出后采用／ACK 主机链路已证明，阶段 0 整体尚未完成**（2026-10-24）。
需求见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)，推进见[执行计划](2026-10-24-transfer-progress-connectivity-plan.md)。
开工基线为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；本记录描述该基线上的候选变更，
不将主机探针当作完整生产迁移、独立进程网络矩阵或真机验收。

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
因此阶段 0 对应登记任务仍未勾选，不能将本原型冒充生产迁移或完整 AC02。

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
真实 Iroh 乱序探针与此权威交接目前仍是分开的证据；新协议实际读取该当前权威的完整组合尚未闭合，
因此阶段 0 登记任务继续保持未验收。

## 尚未证明的范围

- 登记围栏、接收 CAS 后交接及 Application 真实进程寿命已有证据；新权威与真实双端登记的完整组合、Engine／SQLite 重启矩阵仍未完成。
- 输出后 Application 采用／ACK 的最小主机链路已有证据；生产接收输出／SQLite 结算交接、有限补发和状态核对尚未接线。
- 最大负载、分页、终态淘汰、公平调度及新旧双角色 Direct／Relay 尚未执行。
- Linux 当前头文件合同及 BOOTTIME syscall 已核对；真实系统挂起和其他平台尚未验证，不记跨平台经过时间证明通过。
- 完整 workspace 行为回归、平台编译与设备矩阵仍按正式计划执行；本片尚未整体完成。
