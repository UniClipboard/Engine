# 第八片：受管进度迁移与验收证据

状态：**生产受管接线与主机回归已有证据；经过时间实机、设备／产品宿主与最终候选CI仍未闭合，整片保持active**（2026-10-27）。
需求见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)，推进见[执行计划](2026-10-24-transfer-progress-connectivity-plan.md)。
开工基线为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；各节保留对应候选当时的验证范围与未完成项。
当前生产受管接线见[接线核对](#当前生产受管进度接线核对)，正式进度只在执行计划维护；早期原型段落中的“待迁移”表示当时边界，不能覆盖后续生产证据。主机、原型和固定旧端结果均不替代真机验收。

## 阶段 0 结论

结论基于实现提交 `4e4a441d943754e50ba8688a5334dd407790eb82`（PR #256），
源码 tree `92b9c31770ceb80ce6c8e081451e5c9e8afba6cc`；旧端来源保持下文固定锚点。
该候选本地主机验证2151项通过，包括完整Application、P2P、原两轮1GiB负载、独立接收权威／实际Iroh组合、Core及协议；
14项检查含Windows GNU测试代码编译和workspace全目标检查。
各轮原始日志、源码／lockfile／二进制指纹按执行计划规则保存在本地忽略目录或CI Artifacts，本文保留可重复入口与事实边界。

| 阶段0事项 | 结论 | 可核对的证据及限制 |
| --- | --- | --- |
| 开工来源与规则 | 已证明 | 固定第七片合并SHA及原格式边界，见本文开工来源和[规格](2026-10-24-transfer-progress-connectivity-spec.md)；没有撤销第七片 |
| 完整负责人及消费者盘点 | 已证明 | 规格§4与计划责任表覆盖旧reporter／邮箱／投影／Router／生命周期／查询／绑定；盘点完成不表示已迁移 |
| 固定旧端与旧字节 | 已证明 | [固定旧端基线](#固定旧端旧旧文件与旧进度向量)：18条旧向量、两种固定旧端各自旧旧Direct／强制Relay四格；不是新旧完整矩阵 |
| 新版精确字节合同 | 已证明 | [协议合同](../../design-docs/transfer-progress-protocol.md)及协议76项；不改blob／V3／ticket，编解码不代替业务采用 |
| 当前权威与登记顺序 | 已证明 | [独立Application／Iroh组合](#实际当前权威与真实登记的组合)：真实CAS、延迟、替换、重连、kill／relaunch；存储port受控，未冒充Engine／SQLite |
| 输出后采用与精确ACK | 已证明 | [输出后采用](#真实输出后的-application-采用与-ack)：实际BAO／文件输出、采用暂停、重复／冲突／取消／超时／尾随字节；生产交接仍未迁移 |
| 旧单向受管流 | 已证明 | [真实主机探针](#真实主机探针及结果)：同连接两流、独立停止、实际许可与当前准入；未代替生产Router或独立进程旧新矩阵 |
| 包含挂起的经过时间 | 部分证明，仍缺实机证据 | [平台审计](#各平台经过时间审计与物理挂起入口)及[空闲期限](#进度连接空闲期限证明)：原生API合同、逻辑跳变、无socket唤醒和原子复用已验证；Linux原生执行、Windows GNU及HarmonyOS SDK编译；物理挂起及其他平台执行尚未证明 |
| 最大清单与有界资源策略 | 已证明 | [资源覆盖表](#阶段0资源策略覆盖与冻结)：实际1024项CAS／16页、终态超额／阻塞端、17真实对端、31秒工作与30秒回收；部分线上状态是合成值，未代替生产负载验收 |
| 本阶段结论交付 | 已完成 | 本表逐项保留通过范围、缺口与未执行项；结论存在不代表阶段0整体通过 |

**阶段0整体仍未完成。** 按用户2026-10-26继续执行指示，后续生产实现先以未合并stack PR推进；实机证据仍是生产迁移合入前必需验收。当前剩余技术证据是经过时间的真实系统挂起：
须在可用测试设备／环境按[人工入口](#各平台经过时间审计与物理挂起入口)实际采样，区分线程等待、主机系统睡眠和物理设备，
记录跨5秒／60秒／600秒的各自结果及原预算不重置；未执行的平台继续列为跳过。
时钟不可读或回退的保守路径已验证，不能用它替代正常平台的挂起能力证明。

后续实现候选 `358e0a24c09b530b92cf78021f3125731e293c2a` 的[完整CI](https://github.com/UniClipboard/Engine/actions/runs/40211514497)最终13个作业均成功。
workspace实际4849通过／41跳过；覆盖率实际4849通过／39跳过；另行执行原接收权威8项、原资源与生命周期8项（包含两份1GiB文件）、证据分组18项和成员smoke16项。
Blob工件核对固定旧旧四格、必需十格及74次文本／文件精确读回通过，原两个verifier重验通过；前一轮固定rc.15旧旧Relay反向文件`apply_failed`本轮未复现，不能登记根因已修复。
上述CI对应精确实现提交，不代表后续Router修复或文档head已经通过完整CI；绑定编译也不代表实机挂起通过。

实现提交的[跨平台CI](https://github.com/UniClipboard/Engine/actions/runs/40136018862)已结束，结论为失败：13个作业中10个成功、3个失败。
它验证的是上述实现提交，不能声称验证了后续修复或文档head。

| 实际CI结果 | 结论与范围 |
| --- | --- |
| iOS／Android／HarmonyOS绑定编译 | 成功；已核对实际构建步骤，只有编译证据，不代替设备运行／系统挂起 |
| Direct／Relay／Legacy／Known-peer连接恢复四组 | 成功；是现有连接能力回归，不代替新进度生产接线或完整角色矩阵 |
| 必需Blob网络与升级矩阵、presence矩阵、升级smoke | 成功；保持各自原文件、路径与资源断言，不将旧端回归推广为新版进度验收 |
| Engine tests作业 | 失败；workspace实际4747项中4746通过、1超时、37跳过。超时是31秒活动＋30秒空闲证明被Nextest20秒终止线打断 |
| Rust coverage作业 | 失败；实际4747项中4746通过、同一个用例超时、35跳过；未完成coverage交付，不能把通过用例数当覆盖率通过 |
| Engine repository checks作业 | 失败；macOS全目标编译、独立Engine Git消费者及前置仓库检查成功，依赖审计命中RUSTSEC-2026-0387，之后的检查未执行 |

长测试预算漏配已在提交 `38cbafb8f375eb79af459d2ad2513532113b4186`（PR #258）修复：
原用例先本地复现20.008秒超时，随后在Nextest `ci`／`default` 分别61.034／61.040秒通过。
仅为精确用例登记120秒runner预算；测试内部90秒、产品30秒、全局默认20秒和零重试不变。
该修复的六项本地门禁通过，不能冒充上述旧实现CI已经重新通过。

Engine workspace失败后仍实际完成必需Blob生命周期资源8项、testkit证据18项、成员smoke16项及隔离网络配对smoke。
失败导致实际接收权威登记组合、relay历史复用、Direct／Relay完整活跃内容验证等后续步骤跳过；跳过不记通过。

上述旧CI的依赖审计失败来自：[RustSec公告](https://github.com/RustSec/advisory-db/blob/main/crates/crypto-bigint/RUSTSEC-2026-0387.md)
要求 `crypto-bigint >=0.7.6`，旧0.5.5由OPAQUE／VOPRF的elliptic-curve0.13依赖链引入。
当前可用上游版本和兼容更新dry-run尚未提供直接修复路径；处理需保持现有密码协议与持久资料兼容。
[仓库规则](../../design-docs/engine-repository-checks.md)禁止忽略漏洞／unsound公告，当前没有新增例外或放宽门禁。
用户随后明确批准纳入兼容修复；候选按[OPAQUE依赖记录](../../design-docs/opaque-dependency-compatibility.md)维护受限来源，真正移除旧EC／大整数链，原套件、协议、资料格式和清零能力保持。
公开crate归档校验和及原源码已逐文件核对，保留消息展开算法经逆向模块适配后与原版逐字节相同。
本地完整crypto／profile433项通过（9项跳过），包含消息展开6项、RFC9807精确向量1项及认证边界9项；真实Iroh准入32项通过（其余357项因本次过滤未执行）。
固定旧提交重新构建的旧旧／混合两轮兼容验证通过，每轮12次实际握手覆盖双向角色、新旧资料来源、错误密码及损坏消息；资料重编码逐字节相同、继续凭据一致。
Linux初跑诊断符号未满足默认captured要求，按既有CI的 `UC_EXPECT_DIAGNOSTIC_STACK=available` 重跑后完整433项通过；macOS强制captured验收保持不变。
`cargo audit --deny unsound` 实际刷新1352条公告并扫描803个依赖后通过，无新增ignore；9项既有允许警告不记作漏洞修复或额外豁免。
六项根门禁、源码清单及独立Git消费者为本候选交付要求；跨平台远端CI结果须另行核对。上述旧CI失败保留为失败，不能冒充候选修复已通过远端CI。

仍未执行或尚未接线的范围包括：生产Router／旧reporter切换、实际下载状态／SQLite结算交接、
逐项手动重试与批次汇总、稳定Rust／移动宿主视图、完整状态核对与生命周期、
新新／旧新双角色Direct／Relay及真实产品观测／设备矩阵。这些继续按阶段1—6交付，不能从本阶段原型推导完成。

当前证据未要求修改blob／V3格式、新增持久化或弱化登记隔离；没有因此批准上述范围扩张。
后续若发现必须突破这些边界，先修订设计并按原批准要求处理，不直接扩大实现。

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

组合入口为 `scripts/testing/run-progress-registration-proof.mjs`。它遵守既有 `UC_TEST_BUILD_SCOPE`：
CI 的 `workspace` 统一使用整个工作区所有目标，复用前后验收的特性解析与构建产物；
默认 `group` 仍串行构建当前 Application 与 Infra 测试二进制。
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


### 最大原始清单与单轮完整分页

新增单轮读取能力从已登记描述和原始计划出发：Application 独占连接、顺序分页和完整返回，
Core 只核验跨页覆盖，Infra 只进行单页有界交换、精确挑战和完整响应结束检查。
64项页上限由Core统一导出给codec，不改任何线上字节、既有向量或V3／blob格式。
原始计划顺序是分页位置的唯一来源；最大1024项允许16个64项页，不保留页面历史或按序号累积记录。
暂存向量只预留1024项，全部覆盖前不返回完整结果；原绝对期限贯穿所有页，不为后续页重起五秒。

`run-progress-registration-proof.mjs` 新增强制最大清单组合：当前Application进程解码最大V3引用清单，
执行真实接收use case和持久CAS port后交接1024个原始逻辑项，再通过同进程真实已认证Iroh连接逐页读取。
状态载荷为合成数据，包含各活动阶段、成功／失败／取消及数值边界；不冒充实际1024文件输出或生产查询权威。
普通网络回归另覆盖重复／跳页、修订／尝试混合、跨页重复身份、断流、错误挑战、尾随字节、超限帧和原期限耗尽。
明确资料缺失、旧版不支持、授权拒绝、已替换和繁忙与上述失败分开，不因IO失败返回NotRetained。

强制入口将页数、帧大小、实际暂存容量及描述性主机RSS写入 `snapshot-capacity.json`，并核对该产物和当前二进制来源。
RSS不可用的平台登记为空，描述性采样不替代阶段5的1GiB硬资源门禁。
本层没有生产Query入站或实际结果port组装，snapshot round现已接进同一监督worker的共享交换额度，见下节；
完整资源策略冻结和阶段0结论仍需后续任务闭合，不能只凭该分页能力勾选阶段0整项。


### 共享查询与终态的实际监督

进度模块统一为 `ProgressExchangeService`／`ProgressExchangeWorker`，入口只保留模块声明与导出，
handle、worker、model、query、state和retry按完整负责人内部职责组织，不保留旧名称别名或独立查询循环。
终态与snapshot round在同一队列、4／1活动账本和64／16等待额度运行；查询满时明确Busy。
同目标合并成一个watch结果，最多16个并发观察者，阻止重复请求形成无限等待者。
原五秒包括排队，重复不刷新；较早观察者也有独立原截止，不借后到请求延长等待。
终态在同端普通查询前启动，跨端仍保持轮转。

Application实际监督测试覆盖4个混合活动future、16／64排队边界、同目标合并与取消隔离、
最后观察者离开、原期限、时钟错误完整source、迟到采样及关闭时等待不可中断工作。
同一localEndpoint的两个真实已认证对端另覆盖查询成功、过期及取消三个路径：
真实成员准入port被暂停时，查询调用已返回但其round仍占实际寿命；另一端终态登记、采用及精确ACK正常完成。
root停止后仍等待成员读取实际返回，未因调用结束释放活动工作；封口期间重复查询不新建round。

本层没有生产Query入站、真实结果数据库读取或发送侧采用围栏；合成状态载荷不代表文件实际交付。
阶段0完整资源策略与经过时间平台证明仍按剩余任务逐项验收，不将本层局部证明记为整片完成。


### 中间值纵向能力的验证边界

中间值使用同一个Application监督器，独立256／64最新值缓存，当前任务元数据256／64且每任务最多1024项。
Core统一活动／终态序号规则，Application统一按任务5Hz资格、终态撤下中间值及实际round寿命；
Infra实现Update编码、完整流结束和认证后采用，不增加业务ACK。
验证入口为Application `transfer::progress::delivery::live::tests` 与原生
`stream::outbound_probe::live_probe`；真实状态载荷为合成值，不声称这些字节来自实际文件接收回调。
Contact及失联判断另见下节；生产Router及实际下载hook仍未接线，不能据此勾选阶段0完整资源证明或生产迁移项。


### Contact与有效联系视图的边界

Application原任务接受后可跟踪等待状态，Contact在10秒无其他通知时由既有唯一worker调度，
不新增心跳循环、队列或活动额度。任务级联系序号独立于单项进度，全部终态后停止联系。
接收完整快照以原发送意图建立首次60秒期限，只有当前身份内有效的新通知续期。
Core承担纯时间／序号规则；连续性时钟能力由Application统一消费，不可读保留完整错误来源且不复活旧联系。

验证入口为Application `transfer::progress::tests`、`delivery::live::tests::contact_tests`，
以及真实受管Iroh的 `stream::outbound_probe::live_probe`。
新增证明覆盖等待任务Contact、延迟实际结束、重复跟踪／帧不续期、60秒未知、新联系恢复及终态不重开。
时钟回退后的旧轮次迟到结束不得清除新世代状态，独立序号还按producer寿命隔离。
中间状态和联系均为合成fixture，不声称已迁移生产回调、发送侧全局投影注册表或宿主显示；
平台物理挂起矩阵和阶段0完整资源结论仍待后续任务闭合。


### 全局发送投影的有界原型

Application投影注册表从原发送意图开始建立等待项，同一已接受意图重复不刷新首次期限。
每端64／全局256按原接受顺序淘汰，跨端／同内容身份独立；每个当前投影最多1024个原始逻辑项。
被退休对象立即清空原计划和项状态，原挑战仍实际持有4／1登记采用额度至真实返回，迟到描述不能换贴同键新实例。
这些额度只保护本机采用，不启动第二套网络调度器，也不修改文件接收或通知补发责任。

验证入口为Application `transfer::progress::projection::tests` 与真实Iroh
`stream::outbound_probe::projection_probe`。实际构造256×1024已采用项并验证容量淘汰和被钉住对象状态释放，
128条事件提示丢失后完整快照恢复；私有worker在没有快照查询时发布60秒未知，已经未知／全部终态时停止计时轮询。
独立验证全局满、全局有空位时同端仍Busy、旧实例钉住quota、源条目／对端退休及关闭等待实际挑战返回。
真实流覆盖原意图授权后的Update／Contact／终态，以及暂停挑战期间替换和关闭后不得迟到Bound。

本层原型不等于生产原发送hook、Router或稳定绑定完成；真实中间状态仍为合成fixture。
当前网络fixture明确使用Clipboard profile和默认无空闲保留策略，
专属progress profile、每端每方向1连接、30秒／16保留策略仍缺实际证明，不能勾选阶段0完整资源项。
首次关闭竞争测试曾揭示重复close自唤醒忙循环，已修复并保留首次中止日志；不以此前部分绿测替代最终候选。


### 专属profile与真实连接保留

新增进度profile通过真实Iroh backend拨号并使用当前身份、成员与统一撤销登记；
区别于前文预连Clipboard fixture，本证明实际消费Application的专属连接策略。
一次原生拨号提供`/2`与`/1`，不运行会产生额外候选连接的通用交错拨号器；
Router以`/2`优先注册，双版本端选择`/2`，旧单版本端仍交换原始单向帧。
当前进度grant校验实际协议，不能把Clipboard连接错贴进度准入。

验证入口为 `stream::profile_probe`：实际终态登记、采用、精确ACK和跨请求复用；
17个真实对端的16保留与最早空闲淘汰、全活动不能为新连接腾位；
入站方向拒绝第二连接、出站方向在旧工作未归还时继续Busy，原工作结束后可建立新连接；
进度退休不关闭Clipboard连接，统一成员撤销同时封口两种profile。
协议错配用受控身份读取暂停保证两端实际准入竞态可重复，不把未启动的接收回调算作已验证。

真实计时用例保持31秒实际本机工作使用权，随后验证原30秒空闲回收，总耗时约61秒。
网络I/O已停止时，工作使用权仍必须保留资源；QUIC保活不续期管理器空闲时点。
本证明不包括系统实际挂起或真机、Relay矩阵，也没有生产Router／旧reporter切换。
前文Clipboard fixture仍只证明各自列出的能力；专属profile证据独立于它们，不追溯改记。
阶段0整体资源策略和平台经过时间能力仍须逐项核对后作最终结论。


### 阶段0资源策略覆盖与冻结

当前技术策略按spec §5.6冻结，证据按能力组合，不将单个测试泛化为整个生产迁移：

| 策略 | 当前证据 | 边界 |
| --- | --- | --- |
| 每任务5Hz、最新中间值256/64 | Application实际监督压力及真实Update合并 | 回调接线未迁移 |
| 终态256/64、600秒／8时点 | Core预算／账本与实际监督、超额及已认证卡住端隔离 | 跨进程不恢复通知 |
| 同一主动交换4/1、等待64/16、查询16观察者 | Application混合监督及真实共享查询／终态暂停证明 | 不是生产Query授权及采用证明 |
| 全局投影256/64×1024、提示128 | 实际262144项采用、淘汰／事件丢失恢复及真实迟到挑战 | 稳定宿主接口未接线 |
| 原清单1024、每页64、整轮原5秒 | 实际Application CAS计划经专属profile／真实backend读取16页；driver强制校验profile标记 | 状态载荷合成，不是1024文件输出 |
| 每端每方向1连接、空闲16／30秒 | 真实17对端、方向Busy、全活动不淘汰、31秒工作+30秒回收 | Direct主机证明，非实机挂起／Relay矩阵 |
| 新旧受管流／当前准入／关闭寿命 | 原单向流隔离、实际专属双版本协商及统一撤销、实际本机工作不提前归还 | 旧reporter/生产Router尚未迁移 |

最大原清单报告现包含由实际测试网络构造分支生成的`connection_profile`，
验证器拒绝缺失或非`transfer_progress`的报告，不能退回Clipboard fixture仍记本项通过。
原预算、帧上限、计划身份和内容断言未放宽；没有新增blob／V3格式、持久化outbox或生产旁路。
各平台经过时间能力和阶段0结论仍单列未完成，物理设备未执行不能记通过。


### 各平台经过时间审计与物理挂起入口

`SystemElapsedClock` 的原生选择保持不变。以下为PR #255时的源码／官方语义和本地主机审计，不能代替平台编译或设备执行；后续CI结果在[阶段0结论](#阶段-0-结论)登记：

| 目标 | 包含挂起的读取 | 探针排除挂起的对照 | 本轮执行边界 |
| --- | --- | --- | --- |
| Linux | `CLOCK_BOOTTIME` | `CLOCK_MONOTONIC` | 原生时钟及普通线程等待检查；物理挂起跳过 |
| Android | `CLOCK_BOOTTIME` | `CLOCK_MONOTONIC` | Rust cfg 选择核对；目标编译／真机跳过 |
| HarmonyOS | `CLOCK_BOOTTIME` | `CLOCK_MONOTONIC` | `aarch64-unknown-linux-ohos` 为 `target_os=linux,target_env=ohos`；目标编译／真机跳过 |
| macOS／iOS | `CLOCK_MONOTONIC` | `CLOCK_UPTIME_RAW` | 官方睡眠语义核对；目标编译／实机跳过 |
| Windows | `GetTickCount64` | `QueryUnbiasedInterruptTime` | GNU目标检查；本机执行／物理挂起跳过 |

依据：[Linux kernel 时钟语义](https://docs.kernel.org/core-api/timekeeping.html)、
[Apple Libc 时钟手册](https://github.com/apple-oss-distributions/Libc/blob/main/gen/clock_gettime.3)、
[Windows tick](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-gettickcount64)及
[排除睡眠的 interrupt time](https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryunbiasedinterrupttime)。
Apple 的 `CLOCK_MONOTONIC` 在系统睡眠期间仍递增，不能套用 Linux 同名时钟的语义。
[OpenHarmony musl 头文件](https://github.com/openharmony/third_party_musl/blob/master/include/time.h)声明 `CLOCK_BOOTTIME=7`，
[实现](https://github.com/openharmony/third_party_musl/blob/master/src/time/clock_gettime.c)传递 clock ID；这不证明所交付设备内核支持，读取失败仍保留原始来源且拒绝继续旧预算。

本轮新增回归证实：如果 socket 不再唤醒、Tokio 时间仅推进100毫秒、原生经过时间已从0跳到6秒，旧版五秒网络等待不会结束。
`ProgressExchangeDeadline::network` 现以100毫秒有界唤醒重新检查同一原生预算；到期先于下一次网络 poll，不重新创建预算。
该唤醒只包围可中断网络 IO，不改变本机读取／采用必须监督至实际结束的责任；不增加后台执行承诺。
五秒交换、60秒投影与600秒终态保留使用包含挂起的读数；投影／补发 worker 同样在活动时以100毫秒重新观察。
该轮的 `ConnectivityManager` 空闲30秒尚使用 Tokio `Instant`；后续进度profile已改为注入包含挂起的时钟，见[空闲期限证明](#进度连接空闲期限证明)。

人工物理挂起复跑（不会自动挂起系统）：

```bash
node scripts/testing/run-progress-suspend-proof.mjs --physical
```

看到测试开始后，由操作者使系统实际挂起至少10秒再恢复。探针用“包含睡眠时钟差值减去清醒时钟差值”的保守下界识别挂起，
采样窗口只降低该下界；线程等待、进程暂停和人工声明不能直接记为系统挂起。
180秒清醒时间内没有观察到至少10秒挂起则失败；原生读取失败、回退也失败。
记录原始差值、原5／60／600秒预算结果、源码状态、lockfile和测试二进制指纹于 `target/test-artifacts/ci/progress-suspend/`。
仅挂起10秒不证明60／600秒已经到期，报告分别保留其实际结果；需要相应时长的实机用例才能证明跨越这些边界。
此入口直接执行桌面当前测试二进制；移动设备仍须将同一原生时钟与实际 progress owner 接入平台验收宿主，不能将该脚本称为移动真机入口。
普通CI默认跳过人工测试，不接受旧报告；本轮未执行人工挂起，因此阶段0经过时间整项继续未勾选。

阶段0当前结论：登记、字节、输出后采用、旧流和资源策略已取得所列主机证据；
包含挂起的原生能力有源码依据与逻辑边界证明，物理挂起和移动目标编译尚未闭合；技术空闲恢复的后续主机证据见下节。
阶段0整体仍未完成，不能据此开始生产迁移或标记设备矩阵通过；无需改变blob／V3格式或扩大持久化边界。


### 进度连接空闲期限证明

此前空闲回收只依赖Tokio计时，且复用只检查原生连接是否关闭，可能在回收任务运行前重新激活过期缓存。
现有manager保留唯一连接与方向额度负责人；进度profile注入与业务期限相同的原生经过时间能力，
并用Core `ElapsedBudget` 固定最后活动结束后的30秒期限。通用能力的时钟port／来源错误归入connectivity；其他协议显式保持原计时。
后台回收和复用前检查共用同一活动锁与预算，不能“先看见空闲、再关闭已被重新占用的连接”。

定向主机证明覆盖：

- 经过时间0→30秒而Tokio仅推进100毫秒，空闲缓存到期；
- 不推进Tokio、不让后台先执行，请求也不能复用已经过期的旧连接；
- 已分离IO但保留实际工作租约时不回收，即使原生读数暂不可用；最后活动结束才固定新空闲预算；
- 原生不可读保留IO来源并退休旧缓存；相邻读数20→19即使仍晚于起点也不能续期；
- 真实Iroh字节交换与工作租约在注入经过时间跳变后仍存活，真实释放后按原30秒边界关闭；
- 既有真实31秒活动＋30秒空闲、17对端容量和方向／撤销证明继续回归。

这些是原生连接与逻辑时间组合证明，注入读数不代表物理系统挂起。人工挂起入口和平台跳过项保持上一节边界，
不能因空闲修复勾选阶段0实机经过时间任务。生产Router、宿主生命周期与实际通知hook仍未迁移。


## 旧协议入站与原意图的保守投影

受管旧单向流由`TransferProgressInbound::serve_legacy`执行一条完整入站动作，沿用原34字节codec与精确FIN。
实际ALPN必须为`/1`，帧／FIN共享原期限，随后重查当前连接资格；真实`/2`连接传入旧动作会被拒绝，不能据此降级。
认证设备身份来自原连接grant，源条目只用于定位原发送动作已经接受且仍保留的意图，不从网络创建条目。

同一Application投影负责人实现独立的`ProgressUnattributedReceiverPort`。Core登记规则只允许无尝试关联通知
将尚未确认的意图显示为结果未确认；旧字节／总量／状态不进入逐项事实，不刷新联系或制造binding。
已登记、查询确认但无binding以及更换连接后保留描述的任务都不能被旧通知覆盖；待完成挑战仍可正常完成，
后续新版有效活动可以恢复已确认的事实。删除、退休、封口和幂等事件继续使用原注册表围栏。

Core／Application的6项定向验证及真实专属profile的10项验证通过，覆盖全部旧状态、损坏／截断／尾随字节、
实际FIN、认证设备隔离、FIN期间取消／原经过时间到期／会话退休／意图删除，以及已开始采用在取消后仍等待实际结束并保留下层失败来源。
真实Iroh验证使用实际原投影负责人和受控意图／时钟，不冒充生产Router、SQLite或宿主接线；
完整新旧设备矩阵与物理挂起仍单独验收，阶段2的旧版回退整行保持未勾选。


## 固定旧端的单格诊断重放

`scripts/testing/run-progress-legacy-baseline.sh`不传参数时继续执行两个冻结旧端×Direct／Relay四格；
`--case history-relay`等显式选择只用于诊断，仍核验原配对、资料重开、双向文本与文件精确读回、实际路径及清理。
单格报告使用不同scope、`full_matrix=false`并列出未执行格，不能替代完整旧旧基线。非法格名在创建证据目录或构建前拒绝。

GitHub Actions的`Engine real environment`提供`legacy-blob`手工模式，`case_prefix`选择上述四格之一，
默认`history-relay`，`repeat`为1／2／3个独立进程，每轮保留独立证据并在首个失败停止。
预配对入口本身只跑一轮，所以重放由工作流外层明确执行，不能把原`--repeat`值当作已完成轮数。
此入口只准备选中旧端与既有relay宿主，使用原共享构建与固定来源，既有必需CI不传选择参数，仍执行完整基线。

该入口用于定位run40187851498中固定rc.15旧旧Relay的`apply_failed`；本机ARM普通12轮及受控两CPU12轮均通过，
尚未捕捉到同一失败。工具可用不代表旧端间歇问题已修复，远端反馈、物理挂起及生产迁移继续分别记录。


### 发送与读回失败的诊断边界

run40204732042 的 Engine、覆盖率及移动绑定检查通过；完整 Blob 矩阵仍在第十格固定
rc.15 旧旧 Relay 的反向文件发送失败，升级动作尚未执行。原工件只有 `file not accepted`
和接收侧 `apply_failed`，不足以判断发送侧收到拒绝、传输错误还是离线结果；该故障保持未修复。

原网络 runner 现在将每次既有文本／文件发送及精确读回写入同一测试 JSON 的 `deliveries`：
只保留合成节点标签、原场景、固定失败分类、既有计数和发送／读回耗时。发送拒绝或异常停在
`dispatch`，发送已接受但读回失败停在 `readback`，只有精确读回后才为 `verified`；失败仍使原场景失败。
未知错误正文、未知 kind、设备／条目标识、内容、文件名和路径均不复制；原错误继续抛出。
既有业务调用、文本／文件读回期限及旧端二进制不变，也不增加宿主查询或重试。

验证入口为 `node --test scripts/testing/clipboard-delivery-evidence.test.mjs`，并接入原 PR Check。
6 项分类、隐私和失败边界测试及相关 15 项测试通过；复用固定来源的预构建宿主执行完整四格，
16 次原文本／文件精确读回及新增证据核验通过。原 rc.15 同角色两 CPU 重放三轮及带诊断一轮均通过，
仍不能用本地主机成功覆盖远端首次失败或据此完成生产迁移。


## 原投影退休与重建的字节流组合

专属 progress profile 的第二 backend 复用实际 codec、`TransferProgressProducer` 和
`ProgressProjectionRegistry`，在原登记 Challenge 已发出、当前事实读取尚未返回时，分别退休源条目关联、
对端投影关联、全部投影或整个投影负责人，再由完整发送意图重新接受相同设备、源条目和原计划。
旧登记必须经真实拒绝帧返回 `UnknownTask`，不能把迟到 Bound 交给同键新意图；投影负责人关闭后，
原 worker 继续等待实际登记返回，旧 handle 的新意图仍被拒绝。

四项组合保持 producer、attempt、修订和原计划完全相同，重新登记仍取得不同 binding。
旧 binding 的终态经原帧到达后，两端均返回 `Stale`，没有终态 ACK、新变化提示或新投影修订；
新 binding 的同一终态则实际采用并取得精确 ACK。同方向始终只有一次拨号，
避免把换连接误当作意图退休隔离。入口为
[内存 backend 退休组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_memory_backend_retirement_probe.rs)。

本组只证明原投影关联的退休、重建和实际字节流采用，固定身份与事实不代替真实密码认证、
接收 SQLite 权威、成员撤销或生产会话装配；阶段0物理挂起及生产迁移门槛保持原状态。


## 阶段1身份与精确ACK校验

本项完成Core／Application身份规则与既有主机原型验收；生产发送／接收hook、SQLite及稳定宿主迁移仍按后续阶段执行。
设备身份来自连接，通知中的字段不能反向赋予设备或任务权限。各维度使用同一注册和当前权威：

| 维度 | 规则及实际证据 |
| --- | --- |
| 设备、源条目 | Application登记／活动／终态拒绝错误peer和source；源目录以设备／源条目保存当前任务，拒绝输入不改变绑定、项或修订 |
| 逻辑项 | 原计划按kind和原始index关联；File同位置的Representation／DirectoryUnit与未知index均被拒绝 |
| attempt | Core注册描述围栏、Application旧尝试拒绝及实际接收CAS后的当前挑战／迟到Bound组合 |
| round、sequence | Core逐项投影区分真实新round、旧序号和旧round；Application实证较大sequence仍不能覆盖旧round，旧round终态不封口当前项 |
| producer及修订 | Core同producer修订单调且相同修订冻结完整描述；进程重启的新producer不按UUID大小或跨进程修订排序，实际接收重启／新连接组合验证退休 |
| 完整结果ACK | binding、source、attempt、owner_revision、kind、index、round、sequence、result_revision、bytes、total、outcome及原因均经实际字节流核对；None／Some(0)双向、错误帧型和尾随字节分别拒绝 |
| 同一权威 | 原Producer的登记／Contact／Update／终态共用同一authority；原ReceiveWorkRegistry同锁保存接收目录，真实挑战重读当前任务，晚到Bound再次核对 |

[真实ACK矩阵](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_outbound_ack_probe.rs)
通过专属profile的真实Iroh backend，在原Application采用之后只改变回执字段；21个字段／边界场景和14个完整结果词表均执行。
每个场景再次由原完整入站负责人处理正确重试，核对原结果保持且精确ACK成功，避免把传输FIN或错误确认当作业务成功。
[Application围栏组合](../../../crates/uc-application/src/transfer/progress/tests.rs)
核对错误源、设备、逻辑项、尝试、连接世代、旧round和旧sequence的拒绝分类及无副作用。
Core登记、逐项投影和不可变终态规则与[实际接收权威组合](#实际当前权威与真实登记的组合)共同提供其余维度的证据。

本地主机五项定向用例、Core／Application／P2P完整2164项及强制实际接收权威8项通过；
13个默认跳过项中8项已由专用入口另行执行，其余外部凭据／工具项目不记作通过。
本项不改变blob／V3、线上字段或持久化，不替代阶段0物理挂起和后续生产／设备矩阵。

## 阶段2生产Router版本选择

真实握手回归在修复前证明：生产Router先注册`/1`时，双方支持`/2`仍选中`/1`。
Router现在统一设置已安装进度协议的服务端偏好，调用方无需维护注册顺序。
`protocol_router::tests::progress_negotiation_chooses_current_regardless_of_registration_or_offer_order`
覆盖两种注册顺序、两种提供顺序及双方单版本能力的16个真实握手组合；14个共同版本组合精确核对ALPN，2个无共同版本组合明确失败。
专属profile的终态采用／精确ACK／连接复用、分页及旧版完整摘要fixture改用同一生产Router；完整P2P404项通过／11项默认跳过，原实际接收权威专用8项另行通过，六项根门禁通过。
本层只证明版本选择与现有交换；未注册的新版handler不会被自动发布。生产handler、接收hook、生命周期与完整Direct／Relay矩阵仍需后续迁移验收，阶段2对应整项保持未完成。

## 阶段2完整入站连接负责人

Application `transfer/progress/inbound`统一原生连接交接后的准入、真实协议选择、逐流采用、保留与实际关闭；Iroh handler只桥接完整能力。
[真实入站用例](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_inbound_transport_probe.rs)通过生产Router和实际backend验证登记／精确终态ACK／单页查询、旧绑定退休、旧`/1`保守采用、成员撤销、收流后当前准入读取期间消耗原经过时间，以及取消shutdown等待仍监督实际采用。
同一投影在两次连接后重新组装transport曾真实返回`Stale`；世代命名空间改由原投影注册表持有后，同一producer修订仍可登记，新旧binding保持隔离。取消监督与实际能力返回相同`Stopped`只表达同一次取消；期限与迟到真实IO失败仍分别保留来源。两项回归均有行为失败及修复后通过证据。

最终候选Core／Application全量1766项通过、2项默认跳过；P2P全量410项通过、11项默认跳过，包含新增6项真实入站组合；原实际接收权威专用8项另行通过。
源码冻结包含子模块及软链接共3343项。查询提供使用明确的受控本地事实，暂停及经过时间采用可控port；不是Engine／SQLite、物理系统挂起或完整Direct／Relay角色矩阵。
此层交付可安装的完整入站能力；节点／会话协议安装、Engine组装、原发送意图／真实接收hook及设备验收仍未迁移，对应整项继续保持未完成。


## 生产原意图关联的待补项

接入实际对象图时发现，原型中纯空目录的源条目由测试显式注入
`source_entry`；生产 `ClipboardInboundRuntime` 当前没有从消息传递该字段，而纯空目录
又没有可回读源条目的 blob 引用。因此，既有空目录登记证据仅证明“已取得可靠源身份后”的
权威、登记与退休规则，不能证明生产端已经取得该身份。不能以内容摘要生成替代 UUID、
借用 tracing 字段或放宽旧头尾随字节检查来填补该缺口。

用户于2026-10-26明确批准纳入兼容消息头扩展。实现按`clipboard-applied/3`协商可靠源身份，
旧`/2`与`/1`保留原格式，不修改blob与V3；版本选择与源身份传递的真实证明仍待完成。
原发送意图与接收hook的生产验收保持未完成。


## 生产会话与稳定查询的主机验证

2026-10-26，未合并任务分支`feat/transfer-progress-session-runtime`基于`47ef60b65fce66f6208a391ab8d87c1d98ff4150`：

- 原接收CAS接受后一次冻结完整计划，后续dirty提示只读取同一权威事实；原始索引、来源设备、逻辑项、attempt与实际round进入同一通知负责人。源消息头使用已批准的clipboard-applied/3，旧头和blob/V3不变。
- 原Native进度adapter、旧Application通知邮箱、Core reporter trait及Engine业务投影/推断取消已删除。生产节点同一Application端点注册新2/旧1；Application监督通知、投影、接收事实与宿主失效提示，真实关闭等待实际工作。
- `just cargo test -p uc-application --lib transfer:: --locked`：188通过。包含冻结descriptor、旧producer拒绝、真实CAS、自动恢复分支及原高水位/期限、压缩表示总量、目录轮次、完整查询分类和预期尝试拒绝。压缩大小测试先复现明文1024被错误冻结为线上总量，再修复为实际32；没有借比例字节或格式变更绕过。
- `just cargo test -p uc-infra-p2p --lib member_is_removed --locked`：4通过，真实节点成员撤销关闭旧进度连接、等待应用结算的剪贴板连接，并停止在途发送；旧帧先触发原意图变化，以此证明实际授权。
- `just cargo test -p uc-engine --features dev-tools --lib real_engine_offline_cancellation_ignores_permanently_blocked_progress --locked`：1通过。来源离线后准确缓存重试，进度backend被屏障阻塞；本地取消、实际输出清理、原成功文件/路径及完整关闭仍完成。屏障只配合原CancellationToken结束，不替代本地流程。
- `just cargo test -p uc-engine --features dev-tools --lib real_engine_outbound_progress_reports_actual_partial_result_and_reconciles_without_download --locked`：1通过。真实双Engine、认证P2P、SQLite及文件输出后，发送端收到失效事件并读到精确两成功/一StorageUnavailable失败；源与本机接收entry独立，attempt一致。完整核对返回Settled/旧预期Superseded，实际export次数不增加，原文件字节和路径保持。
- `just cargo test -p uc-ohos-napi --lib outbound_view_preserves_large_integer_positions_and_unknown_totals_for_javascript --locked`：1通过，`u64::MAX`修订和字节位置保留十进制字符串，None总量不被改为0或100%。UniFFI/NAPI与Rust同版本完整查询入口均通过workspace全部targets编译。
- 本次候选Rust源码通过metadata、workspace/all-targets、fmt、Rust style、Engine repository及diff检查。这里的定向结论不冒充最新源码的完整Core/App/P2P测试全集，也不复用父PR的CI作为当前head结论。

尚未完成：原再次发送动作内核对、删除/成员/会话退休、实际表示解密暂停的全链路证明，以及新新/固定旧端/三端/Direct/Relay/1GiB、Collector和设备矩阵。物理挂起及移动产品宿主未执行，保持跳过及合入前必需验收，不据此登记整片完成。


## 成员发布与在途意图退休的主机验证

成员正式提交、数据库切换和丢弃可信缓存时，同步退休发送侧派生关联；原发送在首次等待前取得的意图
不能在目标退休后重新建卡。成员签名事实仍由原 Owner 提交，派生清理不产生第二份成员账本或文件取消结果。

- Application 完整库测试：1384 通过、2 跳过。新增回归先证实原 `remove_peer` 允许旧在途动作重新建卡，再验证单目标退休、其他目标保留及长期阻塞动作的有界退休记录。
- [正式成员组合测试](../../../crates/uc-application/src/space/membership/remove_space_member/tests.rs)不消费 watch，验证移除、同设备重新加入及单次输入更换实例、数据库世代变化、失败提交封口与重载。普通成员修订保留其他设备结果。
- `just cargo test -p uc-engine --features dev-tools --lib real_engine_membership_removal_and_space_reset_retire_outbound_progress_and_keep_saved_files --locked`：1 通过（两种实际动作，10.90 秒）。真实双 Engine、P2P、SQLite 和文件输出证明原 `RemoveMember`／`ResetSpace` 后列表清空、旧目标查询明确拒绝，接收端既有成果保持。

此证明覆盖成员发布与投影清理；暂停／恢复、后台、物理挂起、全部网络及设备矩阵仍按计划单独验收。
正式计划保持 29/59，不以派生关联清理代替网络工作、原接收及资源的完整停止证明。


## 原再次发送动作的主机验证

`ResendEntry` 内部核对原文件／大图blob任务，仅显式确认及允许的结果进入原重投递；
数据缺失或旧端提示重复风险，离线和未完成不变成 Missing。按原本机owner解析保留的协议source，
没有增加持久映射、远端强制下载、第二份接收结算或补发预算。

- 完整Application：1387通过、2跳过。七类结果×确认意图、头已送达仍核对、读取失败source chain、确认不绕过查询失败、大图原blob任务及脱敏Debug均覆盖。
- `just cargo test -p uc-engine --features dev-tools --lib real_engine_resend_ --locked`：2通过。真实双Engine、P2P、SQLite和文件，运行及结算不新下载；明确重投只再导出原失败项，产生新attempt并保留两项成功路径；接收端实际关闭后，即使确认新传输也没有新导出，返回未核实结果／对应安全不可用错误。
- 完整outbound场景5通过，原手动发送入口1通过，原流隔离／恢复／部分资源4通过。原成功结果、取消和资源监督保持。
- Engine公开契约54通过，重发结果／观测分类6通过；UniFFI47、N-API21、移动验收宿主26通过。N-API逐目标结果复用完整视图映射，最大64位修订／字节保持字符串，未知总量仍为None。

以上是主机功能及绑定映射证明，不是移动设备执行、物理挂起、完整旧新Direct/Relay或Collector可视化证明。
固定旧端无法核对的决策已由明确Unsupported结果验收，完整旧端网络及设备矩阵仍按阶段5单独执行。


## 会话退休后的确认验证

原真实双Engine接收重试在第四次实际输出处暂停；接收端锁定等待结算并清理未提交输出，
解锁后原完整再次发送查询返回NotRetained。修复前源端仍有1项运行，修复后应立即未知，
保留2项已保存成果，不重新下载；若真实取消／失败已先确认，则保持真实终态。
完整负责人仍为Application transfer/progress，接收结算和文件清理由原接收负责人承担。

复跑：`just cargo test -p uc-engine --features dev-tools --lib outbound_progress::lifecycle --locked -- --nocapture`。
该组同时覆盖源端Engine.suspend/resume后的投影退休、同一设备身份、已保存文件及默认不重下。
这些是进程内真实Engine动作，物理系统挂起、完整进程重开、移动设备与Collector矩阵尚未执行。

Core注册回归覆盖旧绑定、在途挑战失效及修订高水位；Application完整核对回归覆盖
NotRetained／Unsupported、初始等待、全量终态、重复回复、相同快照恢复但不续期、跨范围和新意图隔离。
实际结果：Core 394通过，Application 1389通过／2跳过；上述真实Engine组2通过（11.07秒）。
正式30/59不因独立修复提前增加。


## 暂停恢复与同资料重开的主机验证

真实双Engine生命周期组扩展为4种动作：源端暂停恢复、源端完整关闭后同资料目录和安全存储重新Engine::start、
接收端活动输出期间锁定解锁、接收端活动输出期间暂停恢复。源端设备身份与接收端成功文件均保持，
丢失原发送关联时默认再次发送返回NotRetained并提示重复风险，不新增投递或导出。

接收端暂停期限耗尽明确返回DeadlineExceeded，仍在导出的输出继续受监督，不能提前发布Suspended；
归还输出后才完成暂停，恢复后通过完整查询采用实际已结算的取消或失败，保留两项成功结果。
锁定会封口退休来源目录，暂停则保留同一Application的接收事实，因此分别核对NotRetained和Settled，
不能用统一“未知”覆盖仍能核对的实际终态。新尝试登记先于活动通知，测试等待完整活动视图后才发起生命周期动作。

复跑沿用`outbound_progress::lifecycle`组，2测试覆盖上述4动作，23.12秒通过。
相邻发送侧实际网络回归7测试全部通过（71.89秒），同时覆盖删除、成员/Space退休、离线与再次发送。
这是同进程不同Engine实例的资料重开证明；独立进程崩溃、物理挂起和移动设备矩阵仍未执行。
正式30/59保持，完整生命周期项不提前勾选。


## 普通重连与原预算的主机验证

生产恢复观察者复用原Space可达性事件，在同一Application运行期内将Online交给原交换服务。
实际worker组合覆盖Offline／Unknown及无关peer不授予当前项机会，重复与丢失提示只合并机会；
原终态t0不变，原600秒退休后不能复活，已开始的本地收尾仍受监督。组合测试0.16秒通过。

原生真实网络测试经生产Router、handler、受管manager、producer及接收投影完成结果采用后丢ACK，
实际关闭旧连接，再由同一原通知worker在逻辑595秒补发同一producer／attempt／result；
断言新connection id不同，逻辑599秒原deadline有效、600秒失效，此后20次恢复机会仍不再发送。
真实采用共2次且原成果保持，真实网络用例0.19秒通过。没有重新submit终态或新建意图伪装重连。
复跑：`just cargo test -p uc-infra-p2p --lib real_reconnection_keeps_original_terminal_retention_and_does_not_replay_expired_result --locked -- --nocapture`。

这里使用可注入经过时间，实际连接确已断开／重拨，600秒不是墙钟等待，也不代替物理系统挂起。
Application完整1390通过／2跳过；原生入站7通过（0.43秒），真实Engine发送侧7通过。
这些Engine场景同时验证原运行期的启动／关闭、删除、成员退休、暂停恢复、资料重开和再次发送。
正式30/59保持，普通恢复机会不授予新成员权限、不恢复文件任务、不更改逐项事实或下载预算。


## 原接收owner交接验收

复用完整ApplyInboundClipboardUseCase，将已有CAS先后场景扩为有／无消息源条目提示×接受／拒绝四组合。
接受前没有当前进度任务或变化通知，拒绝后不调用材料化、不产生通知资格；接受后材料化入口的attempt
与原BeginReceive结果一致，本机接收条目区别于源条目，三个原ticket及数组顺序完全保持。
受控材料化能力明确把三个opaque ticket映射到两个provider角色，Application不解析或据此替换消息来源。

材料化失败后，三个原逻辑项如实失败，变化目标与可查询范围仍是消息源；provider角色查询NotRetained。
有新消息源提示时，以提示的源UUID关联，而不误用不同的blob归属条目UUID；旧格式仍要求一致blob来源。
交接四组合通过（0.09秒），接收目录21测试通过，覆盖原计划／实际保存事实及跨attempt的旧动作围栏。
源码核对首次接收仅在Begun后accept_source，手动重试在原expected-attempt CAS接受后才继承同一来源与成功项。

复跑：`just cargo test -p uc-application --lib real_receive_handoffs_only_after_cas_begun_and_before_materialization --locked -- --nocapture`；
目录：`just cargo test -p uc-application --lib transfer::receive::work::catalog --locked`。
本项使用受控存储／材料化port，证明完整用例的交接与权限行为，未证明实际三端拨号或SQLite输出提交。
真实Engine文件成果链和实际三端矩阵仍分别按阶段3下一项及阶段5验收，不能据此勾选；正式31/59。


## 放弃删除等待的调度边界验证

#299的Engine CI在`real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state`的20秒完整清理巡检超时，
首次失败保留。当前来源原用例独立进程运行23次通过，未据此认定已修复；随后只延后等待future首次poll 75毫秒，
保留原50毫秒/20秒及全部删除条件，得到3次稳定失败。进一步通过原完整核对确认，导出仍持有时操作已报告取消，
原接收来源仍保留为Running/Superseded，条目、接收状态及全部文件保持，说明受控复现没有进入删除负责人。
旧CI未记录这些细分条件，因此不能从其泛化超时独立判定每次失败的全部原因。

测试修正先以既有完整核对确认原来源，再与原删除并发观察NotRetained，确认删除已经退休该来源；
随后才放弃50毫秒等待，放行实际导出后仍在原20秒内核对条目NotFound、无接收状态及全部缓存文件移除，
并等待Cancelled完成事件后验证重复删除。覆盖首次立即调度与75毫秒延后，未增加稳定API或改变生产实现。

复跑：`just cargo test -p uc-engine --features dev-tools --lib real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state --locked -- --nocapture`。
原完整修正场景通过（8.36秒），相邻7个真实Engine接收回归通过（46.06秒）；同一固定二进制再重复3次通过（7.883／7.757／7.059秒）。
该修复验证已开始动作的等待者退出语义，不扩大跨进程恢复或物理挂起证明；正式31/59保持。


## 真实文件成果与精确ACK链路

复用真实双Engine的原发送／接收动作、原SQLite和磁盘导出，在第三项实际输出屏障持有期间，
核对第一项已提交且文件可读、第二项实际存储失败、第三项仍处理中；原生进度wire完成两份精确ACK。
放行后核对第三项实际成功及文件读回，原发送端完整查询采用Completed／Failed／Completed，
同一源条目、实际接收attempt及原始索引不变，三份ACK的round／bytes／total与采用视图一致。
仅测试装配观察原wire返回值；生产传输、采用、确认策略和接收流程没有替代实现。

首轮只能收到第一项ACK。查询回归进一步证明，原快照把单项已实际清理结算的失败也等待父批次结束。
原ReceiveBatch增加私有内存结算事实：实际单项失败／取消结算后可发布，仍持有本地工作的失败继续Processing；
目录失败仍须等待整体回滚。待清理→已结算的失败和取消两格红→绿，目录负向及目录组23项通过。
没有新增持久化字段、稳定查询或通知预算；重启仍按原未保留事实处理。

最小真实链通过（9.43秒），复跑：
`just cargo test -p uc-engine --features dev-tools --lib real_engine_file_is_durably_saved_while_batch_and_later_file_are_still_receiving --locked -- --nocapture`。
完整Application1392通过／2跳过（141.27秒）；
`just cargo test -p uc-engine --features dev-tools --lib receive_retry:: --locked -- --nocapture`通过42项／1跳过（325.95秒）。
Engine跳过项需要本机严格Collector与Jaeger，不记业务观测验收通过；首次Engine编译重复导入已修正后完整重跑。
本项完成阶段3最小成果链，正式32/59；各输出阶段受控停顿、实际三端、物理挂起和移动矩阵仍保持原要求。


## 真实本地结算边界与通知隔离

沿用原双Engine、SQLite和磁盘输出，用既有完整查询核对消息源侧采用事实。
测试装配只持有原能力返回权或注入实际存储／文件系统故障，不扩大稳定Engine入口。

| 真实边界 | 屏障及核对 | 放行或失败后的结果 |
| --- | --- | --- |
| 表示AEAD校验前 | 原SDK已经完成校验接收，读取能力持有实际密文字节，原解密器尚未执行；线上大小已知且字节100%，源端完整核对仍Running／Processing，无本项终态ACK | 正常密文由原AEAD解密、原保存流程提交且完整读回；只篡改交付副本时原AEAD实际拒绝，接收分类及精确ACK均为IntegrityCheckFailed；原SDK数据不被修改 |
| 文件输出返回前 | 实际文件已可读，原export能力还持有完成权；源端核对Processing，无本项终态ACK | 放行后原流程提交文件，核对同一路径、逐项采用和三份精确ACK |
| 网络终态SQLite提交 | 原成功／失败网络事实的UPDATE被实际触发器拒绝，原输出／终态修复仍在执行；源端Processing，无本项终态ACK | 移除触发器后由原负责人补记原网络事实并监督完成，不能改写原成功为网络失败或重新导出既有成果 |
| 失败输出清理 | 实际导出或保存失败后，把预留文件替换成目录使remove_file真实失败；原父尝试未结算，源端Processing，无本项终态ACK | 清除文件系统阻碍后，原修复结算完成，SQLite与实际路径一致，原成功文件保持 |
| 通知永久等待 | 来源离线且反向connect实际进入永久等待；原失败项缓存重试暂停在实际导出返回前 | 放行后本地输出及提交仍完成，三项均成功，原两项文件和路径保持；关闭无需解除通知屏障。原离线取消场景另核对取消／清理同样独立 |

新边界组复跑：
`just cargo test -p uc-engine --features dev-tools --lib receive_retry::settlement_boundaries --locked -- --nocapture`，
3测试通过（32.96秒），其中表示测试包含正常／篡改两格。
原清理组`receive_retry::save_failure::cleanup`3项通过（28.48秒），
原提交组`receive_retry::save_failure::terminal`3项通过（32.94秒）；都保持原故障条件、操作期限和资源清理断言。

以上证明主机上的原完整接收流程与进度结算围栏；实际三端、设备物理挂起、完整旧新网络矩阵及Collector业务观测仍按原计划验收。

当前候选完整接收回归`receive_retry::`45项通过／1项按原条件跳过（355.00秒），包含原取消、生命周期、保存修复和流隔离。跳过项需要本机严格Collector与Jaeger，不记业务观测通过。

同一候选nextest ci模式新组3项全部通过（33.865秒），分别6.857／9.086／17.908秒；该命令筛选外342项未运行，不是额外跳过失败。精确表示用例含两套双Engine流程，default／ci仅为该例设90秒外层预算，原单次操作期限与零重试保持。正式33/59。


## 逐项结果与批次汇总的主机验证

沿用原双Engine、SQLite和磁盘流程，从原消息源的完整查询及原生ACK核对：

| 场景 | 实际源端事实 |
| --- | --- |
| 首／中／末项保存失败 | 各自2成功、1失败，零运行／等待／取消／未确认；逐项StorageUnavailable、原索引及三份ACK的轮次／字节一致，末项成功不覆盖前项失败 |
| 单项重试运行→取消 | 原两项成功完全保持，当前尝试先2成功＋1运行，再2成功＋1取消；源端取消原因转换为RemotePeer，原文件和路径保持 |
| 明确拒绝与未确认 | 锁定期间完整查询明确Unauthorized，不能冒充不可达；锁定／解锁退休目录后NotRetained，未结算项变未确认，原两项成功保持，失败数为0 |
| 两成员目录成功 | 保存后实际URI只指向一个完整目录，两个成员字节完整读回；源端1项成功、1份DirectoryUnit ACK，不按子文件计交付 |
| 目录事务失败与新尝试 | 原触发器真实拒绝capture，原目录回滚；首轮NoEntry后另一条已接受入站建立新attempt，旧查询Superseded，新尝试最终只有1项失败，已完成子文件不算目录成果；失败后未开始成员不被强制启动，既有独立成功文件不重做 |
| 纯空根目录 | 不含blob引用，实际创建空目录；源端1项成功、1份零字节DirectoryUnit ACK，不因没有网络下载项变成空批次或未确认 |

目录NoEntry捕获失败目前公开的失败原因是Unknown，仍是已确认Failed；它与Unconfirmed状态不同，前者计失败、后者不计失败。测试逐字段核对接收事实和源端采用，不制造更具体的失败原因。

真实空根路径首次拒绝暴露两个实现缺口：捕获端既有的EmptyDirectory／`.`根标记被物化路径拒绝；完整入站port实现又把已认证消息传来的source_entry覆盖成None。
根路径修复只允许规范的空根标记，拒绝文件`.`、空路径、嵌套点分量、父路径、带blob引用或同根同时列出其他成员；原身份和线格式不变，见[ADR-010](../../design-docs/decisions/010-directory-sync-as-file-set-manifest.md)。
桥接改为保留原source_entry。原有／无源提示×CAS接受／拒绝四组合现在经过真实InboundClipboardApplyPort实现，红→绿；仍验证消息源与两种provider角色、blob归属条目及本机条目独立，不由provider推断来源。
旧原型清单也使用捕获端的规范根标记，避免只验证不能实际物化的空路径输入。

根空目录3测试通过，包含路径负向和矛盾清单；真实port交接4组合通过。
相关Engine的nextest ci模式8项全部通过（59.005秒），复跑：
`just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib -E 'test(/receive_retry::batch_outcomes::/) | test(/save_failure::real_engine_(first_save_failure|last_save_failure|post_download_save_failure)/)' --locked`。
完整Application回归1395项通过／2项按原条件跳过；完整Engine接收回归50项通过／1项按原条件跳过（384.77秒）。Collector跳过项不记业务观测通过；未将设备、物理挂起、独立进程或完整三端矩阵记为完成。


## 生命周期资源报告覆盖修复

#304 的 Engine CI 中，完整 nextest 与8项外部资源测试本身通过，随后报告汇总失败。
原始 artifact12651152583 的7份生命周期报告覆盖6个场景：删除场景按即时／延迟调度各产生一份；原脚本固定要求6份，稳定复放同一断言失败。
因此不能把它记为资源泄漏，也不能用放宽文件数量绕过覆盖校验。

[原资源脚本](../../../scripts/testing/run-blob-lifecycle-resources.sh)继续执行原8项真实测试，
报告校验提取到[同一入口](../../../scripts/testing/verify-blob-lifecycle-resources.py)，
按既有case身份严格核对6个场景及各自次数，删除必须2份，其他各1份。
每份报告的FD／线程／任务及原生句柄归还检查、SDK tag、两份1GiB文件和下载配额断言保持。
汇总分别记录场景数6和报告数7，不能用重复报告抵消缺失场景。

[回归检查](../../../scripts/testing/verify-blob-lifecycle-resources.test.py)覆盖正确七份、遗漏第二次删除、六份重复掩盖缺场景、未知身份、额外重复及任一删除报告资源失败。
旧实现6项中3项失败，修复后6项通过；同一CI原始产物完整重放也通过。
当前候选原8项资源验收全部通过；含编译总478.663秒，实际报告严格核对6场景／7次，汇总与全部原16条资源断言通过。双1GiB两轮传输、实际provider配额及各生命周期均由原真实Engine产生证据，不据此宣称设备或完整网络矩阵完成。


## 手动重试与旧尝试围栏的主机验证

原双Engine／SQLite／文件成果场景沿用单一RetryEntryReceive动作，只选择原失败项。
原尝试三份精确ACK确认后，暂停新导出；接收CAS接受不同attempt，源端完整核对显示2成功＋1Processing，
原两项成功的完整字段保持。旧attempt取消和核对均为Superseded；放行后新attempt完整Settled／3成功，
原成功文件内容及路径保持，保护与导出各只增加一次且只属于原失败项。
新增用例`real_engine_manual_retry_replaces_source_attempt_and_preserves_unselected_successes`在nextest ci模式通过（4.591秒）。

[实际专属连接验证](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_retry_fence_probe.rs)
复用真实身份准入的ProfilePair、生产Producer／Wire／Inbound及全局ProjectionRegistry：

- 新描述取代旧描述后，旧尝试再次完成挑战也被拒绝；旧状态带更高序号／字节仍被拒绝，旧终态得到明确Stale拒绝而非ACK。当前binding、attempt、完整逐项事实、汇总、版本和事件均保持；随后当前尝试仍取得精确ACK。
- 原结果已通过Application采用时，真实入站port暂缓返回，待接收权威切换attempt后才让原生ACK写回。Producer在取得精确ACK之后再核对当前权威并返回Stale；新尝试再通过完整登记／终态链取得自己的精确ACK和不同字节结果，不继承旧终态。

专属连接中的当前描述由可控authority夹具提供；真实持久CAS及成功文件保留由前述双Engine场景证明，不将二者冒充同一个SQLite故障注入。
原Application的迟到挑战、状态采用及通知监督围栏仍由完整进度测试组验证。
原未选取消项保留、调用者放弃与失败清理，由既有manual_retry完整负责人测试验收，受控capture／fetch port范围保持原说明。

同一候选相邻原生outbound组nextest ci模式12项全部通过（0.724秒），包含逐字段错误ACK及旧挑战／替换意图；
Application完整进度组78项全部通过（16.21秒）；完整手动重试组30项通过（42.22秒），接收目录组23项通过（0.02秒），包含未选取消、旧交接及原始位置保持。完整Engine接收51项通过／1项按原Collector条件跳过（384.27秒），不记业务观测通过；设备与完整网络矩阵保持原责任。

复跑入口：

```bash
just cargo nextest run --profile ci -p uc-infra-p2p --lib -E 'test(/outbound_probe::/)' --locked
just cargo test -p uc-application --lib transfer::progress:: --locked
just cargo test -p uc-application --lib clipboard::sync::apply_inbound::tests::manual_retry:: --locked
just cargo test -p uc-application --lib transfer::receive::work::catalog::tests:: --locked
just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib -E 'test(real_engine_manual_retry_replaces_source_attempt_and_preserves_unselected_successes)' --locked
just cargo test -p uc-engine --features dev-tools --lib receive_retry:: --locked
```


## zstd依赖审计与冻结旧压缩载荷

#308 的仓库CI审计暴露原锁图的4条漏洞与10条unsound阻断；本地同一公告库提交
`a78282a1414d6c28cc228c4eae3570451affb0dc`重放确认。
按官方[RUSTSEC-2026-0391](https://github.com/RustSec/advisory-db/blob/main/crates/zstd/RUSTSEC-2026-0391.md)、
[RUSTSEC-2026-0403](https://github.com/RustSec/advisory-db/blob/main/crates/zstd-safe/RUSTSEC-2026-0403.md)、
[RUSTSEC-2026-0398](https://github.com/RustSec/advisory-db/blob/main/crates/zstd-safe/RUSTSEC-2026-0398.md)和
[RUSTSEC-2026-0410](https://github.com/RustSec/advisory-db/blob/main/crates/zstd-sys/RUSTSEC-2026-0410.md)修复范围，
两个直接依赖最低改为zstd0.14.3，唯一生产锁图仅更新zstd／zstd-safe／zstd-sys到0.14.3／8.2.0／2.1.2+zstd.1.5.7。
同一公告库下`just cargo audit --deny unsound --json`转为0漏洞、0unsound并退出0；原非阻断的5条unmaintained和5条yanked保留，没有增加忽略或减少审计范围。
本仓使用bulk压缩／解压，不将公告中stream／seekable／wasm32特定路径描述为本仓已证实触发的线上故障。

既有内容保护负责人、MasterKey AEAD、压缩等级、解压上限及线上／持久化封装保持，底层Zstd C仍为1.5.7。
复用仓库共享target的一次性主机探针对冻结旧链0.13.3／7.2.4／2.0.16+zstd.1.5.7与新链进行实际双向互读，
空、文本、可压缩、字节循环、二进制5类数据共10次精确读回，非空数据容量少1字节的8次解压均拒绝且没有输出文件；
五类实际压缩字节也逐一相同。探针显式冻结三个版本，避免把重新解析旧宽范围后得到的新底层库当成原锁图；没有把旧依赖加入生产图或创建独立构建目录。

[旧压缩向量及来源](../../../crates/uc-infra-content/src/clipboard/chunked_transfer/fixtures/README.md)
保存由冻结旧链实际生成的178字节合成载荷。
原`compressed_legacy_v3_content_remains_readable`改为读取该固定向量，经过既有UC3加密封装及实际TransferCipherAdapter解密／解压，
避免用当前编码器生成的“旧”载荷自证兼容。
此候选内容模块80项全部通过；Profile库测试249项通过／3项按原条件跳过（33.72秒），跳过项为需要显式现场资料的两项读取与一项大状态性能探针。完整Engine接收51项通过／1项按原Collector条件跳过（385.86秒）。另跑真实SQLite密文接收／文件与目录提交、资料升级及独立进程升级恢复共5组集成，60项通过；2项历史资料fixture未提供，子进程入口按原安排默认忽略但由父例实际启动，不据此替代原设备／完整网络矩阵。


## 进度生命周期完整场景的验证预算

原#305覆盖率[任务121568556606](https://github.com/UniClipboard/Engine/actions/runs/40321868739/job/121568556606)
在`real_engine_receiver_session_retirement_does_not_leave_unretained_retry_running_on_sender`运行20.021秒时由nextest外层终止。
该例包含接收端锁定／解锁和暂停／恢复两套真实双Engine流程；相邻
`real_engine_sender_resume_and_profile_reopen_retire_unpersisted_progress_without_redownload`
也包含源端暂停／恢复与同资料关闭重开两套流程。

先保存原二进制及构建元数据，Rust源码与测试体保持；冻结二进制SHA-256为
`5a417556898c83cf2266a22fa29e3de4c877552626056c7efb564115eb950821`。
正常CPU原单项ci通过；限制本机CPU0后，原20秒配置单项在20.020秒复现TIMEOUT，
同文件两项再次运行分别20.011／20.025秒被终止。
仅在实验配置延长外层后，原二进制、同CPU、全部原断言分别20.024／22.562秒完整通过，
表明原20秒外层会截断仍能正常完成的完整场景，不据此声称修复任何内层业务故障。

实际配置只为上述两个完整函数名在default／ci设置120秒外层期限；
遵循[Nextest单项设置覆盖规则](https://nexte.st/docs/configuration/per-test-overrides/)，
原engine-host-contract串行组、各内层期限、原断言、其他测试20秒默认线及零重试保持。
结构化比较确认其余全部配置相同，没有扩大包级测试预算。
同一冻结二进制验证实际配置：单CPU ci两项通过（17.305／21.700秒），
单CPU default两项通过（20.964／22.529秒），正常CPU ci两项也通过。

复跑（本机允许CPU0时）：

```bash
taskset -c 0 just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib -E 'test(/receive_retry::outbound_progress::lifecycle::/)' --locked
taskset -c 0 just cargo nextest run --profile default -p uc-engine --features dev-tools --lib -E 'test(/receive_retry::outbound_progress::lifecycle::/)' --locked
```

本任务只调整完整验证用例的外层预算，正式35/59保持；原成员撤销查询失败及设备／完整网络矩阵仍独立验收。


## 成员撤销确认等待的暂态查询拒绝

原#307的[Engine任务121578635878](https://github.com/UniClipboard/Engine/actions/runs/40324797401/job/121578635878)
在真实撤销期间，对QueryDeviceGroupChoices直接unwrap，遇到1211／Unavailable／retryable=true后失败（5.039秒）。
现有稳定查询将该类拒绝声明为可重试；它不能被当作撤销已确认，也不必让原有有界确认轮询立即失败。
原成员准备轮询已经尊重这个契约，本例的撤销确认等待却假设查询永远成功。

[原真实场景](../../../crates/uc-engine/src/testing/host_adapter_contract/receive_retry/lifecycle.rs)
每次仍执行真实Query，只将首次成功响应受控替换为观察到的可重试1211；原真实错误和其他响应不被覆盖。
旧unwrap在同一完整场景3.92秒红测复现相同公开错误；这只证明消费者对该响应的处理缺口，不声称注入了实际SQLite故障或定位原CI的全部下层来源。
修正只在原10秒撤销确认内重查精确的1211／Unavailable／retryable组合，其他错误或结果立即失败。
必须由后续真实完整查询确认本机仍Active、旧源已非Active后才释放导出；持续不可用或撤销未完成仍按原期限失败。

原100毫秒只放弃等待、实际operation继续受监督、未归还导出仍存在、最终输出清理、原成功成果保持及缓存重试AccessDenied断言均保留。
新增故障注入必须实际执行的检查，没有新增生产重试、稳定入口或持久化字段，也没有放宽原期限。
修正后原完整场景3.37秒通过，nextest ci模式3.308秒通过；原8项资源验收全部通过（396.141秒含编译），严格6场景／7报告及FD／线程／任务、SDK tag／原生句柄、双1GiB与实际provider配额均通过；设备与完整网络矩阵保持原验收责任。


## 终态丢失后的核对与同绑定重放验证

本项沿用原接收成果负责人、Application 原子投影和有界通知负责人；没有修改生产实现、消息头、协议、持久格式或稳定 API，也没有新增测试专用生产入口。

原双 Engine 案例 `real_engine_missing_terminal_converges_from_unknown_by_full_query_without_redownload` 先完成两项真实文件、保留一项 SQLite 保存失败，再只重试失败项。使用已有 wire 屏障阻止终态写线，实际接收仍完成 SQLite 提交与第三份文件保存。源端按原系统时钟等待 60 秒新鲜度到期，保留两项成功、将缺失结果的一项显示为未确认且不计失败；原 `ReconcileOutboundProgress` 随后取得同一尝试的完整真实结果，恢复三项成功。核对前后文件字节、既有路径、四次获取及四次导出保持一致。该场景在 Nextest CI profile 下实际通过 63.372 秒；仅为此精确函数配置 default／ci 的 120 秒外层预算，原内层期限和其他测试配置保持。

原生 `ProfilePair` 案例 `lost_native_ack_and_event_replay_exact_result_without_duplicate_changes_or_terminal_rollback` 使用生产 Wire、Inbound 和全局 ProjectionRegistry：同一绑定中重复活动不增加版本／提示，旧轮次或序号被拒绝；Application 采用后销毁尚未编码 ACK 的真实入站流，发送方取得失败而接收投影已结算。随后三次重放均取得原精确 ACK，版本和事件不重复；字节、结果版本、轮次、序号及成功／失败冲突均不能改写原事实，迟到活动也不能重开终态。分别保留提示订阅者和丢弃全部订阅者，证明提示发送失败不回滚已采用结果，重新读取完整快照仍可恢复。两个模式在单个案例内实际通过 0.08 秒。接收侧冲突按既有协议编码 `InvalidScope`，发送侧映射 `InvalidPlan`，没有扩大线上错误词表。

现有 Application `transfer::progress` 组还验证完整快照原子采用、后项冲突时整页不部分提交、重复核对不发布变化、迟到核对不跨越登记／意图替换，以及缺失／旧版回复不擦除真实终态；原生逐字段错误 ACK 和旧尝试围栏仍作为本项相关证据。本项主机执行不等于移动设备、物理挂起或固定旧新端完整网络矩阵通过。

当前候选的相关验证共 98 项通过：Application `transfer::progress::` 78 项，原生 outbound 既有 12 项与新增真实重放 1 项，Engine `receive_retry::batch_outcomes` 既有 6 项与新增收敛 1 项。后两组按已执行的新场景与剩余回归分开运行，未把过滤项计为执行；Engine 明确启用 `dev-tools`，Nextest 使用 `--no-tests fail`。可复现命令分别为 `just cargo test -p uc-application --lib transfer::progress:: --locked`、`just cargo test -p uc-infra-p2p --lib lost_native_ack_and_event_replay_exact_result --locked`、`just cargo nextest run -p uc-infra-p2p --lib --profile ci --locked -E 'test(outbound_probe)'`，以及 `just cargo nextest run -p uc-engine --features dev-tools --lib --profile ci --locked -E 'test(receive_retry::batch_outcomes)'`。


## 通知容量与本地接收隔离验证

新增主机案例 `real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup` 沿用原双 Engine、实际网络、SQLite 和文件能力：原独立批次已有两成功、一失败；新批次发送 67 份互不相同的内容，实际 `BlobReadStorePort::protect` 的未命中结果证明没有用缓存重试替代新下载。既有 wire 屏障阻止全部反向终态写线，原容量仍为每端 64、全局 256，不缩小参数制造超额。

在前 65 项已提交并逐份读回后，第 66 项已完成真实获取与文件写出、仍由原导出步骤持有完成权，第 67 项尚未获取。原取消动作先进入 cancelling，等待输出归还完成权后清理第 66 项，保留新批次 65 项及原独立批次两项成功的文件和路径。新批次共 66 次实际冷缓存获取、66 次导出，没有提前获取末项。解除反向屏障后，恰好最后 64 项（索引 3..66）取得原生精确 ACK，证明前三项已由原终态账本实际淘汰而未被观察循环重新领取；源端完整核对随后从同一接收尝试补齐 65 成功、2 取消，获取、导出及缓存计数不增加。没有生产行为、协议、持久格式或 Engine 稳定接口改动。

Linux 原资源监督同时记录实际 Blob 句柄从初始 `[0, 0]` 到累计创建／仍活跃 `[2, 0]`，关闭后 FD、线程、运行任务及活跃句柄均回到基线。完整案例直接执行通过 32.80 秒，其中资源空闲收尾 24.091 秒；Nextest ci profile 同场景与资源计数通过 32.893 秒。仅该精确用例的 default／ci 外层设置为 120 秒，容纳实际接收及原最长 45 秒资源收尾，原内层期限和其余配置保持。

大批次测试与发送并行消费最早接收提示。最初先等发送返回再读事件的测试在实际新获取／导出已到 66 项后收到 `ConsumerLagged`，错过原有界事件中的 Pending；这是测试消费顺序问题，没有扩大原 256 项事件通道或修改业务事件语义，临时诊断已移除。本例实际触发每端 64 项淘汰；全局 256 项账本、中间值隔离和原交换槽位仍由 Application 进度组分别验证，不冒充多 Engine 全局满载。此主机验证不替代移动设备、物理挂起或固定旧新端完整网络矩阵。


[固定资源入口](../../../scripts/testing/run-blob-lifecycle-resources.sh)现在保留原 8 项并强制加入容量场景，共 9 项；实际运行全部通过，含编译总 362.621 秒，新容量场景通过 33.194 秒。严格校验 7 个生命周期场景及 8 份报告（删除仍按两种调度各一份），SDK store 关闭前临时标签、原生句柄、FD／线程／任务、连续两份 1GiB 的精确读回及 ACK、原下载并发 4／2 和等待 64／16 均通过，原所有资源断言保持。[报告校验](../../../scripts/testing/verify-blob-lifecycle-resources.py)仅增加必需场景身份；[7 项反例检查](../../../scripts/testing/verify-blob-lifecycle-resources.test.py)包含缺少容量场景、缺少第二次删除、等量重复替换、未知身份、额外重复，以及逐报告逐资源字段的失败拒绝。新增缺场景检查在原校验器先失败，修正后通过；本次实际原 8 项产物也被新校验器明确拒绝，不能沿用旧覆盖冒充新入口通过。

当前候选完整 Engine 接收范围共 53 项通过：其余 46 项、固定入口中原 6 项生命周期及新容量 1 项，重复执行不叠加计数；Application 进度 78 项通过，Collector 专属项仍跳过。可复现入口为 `just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib --locked -E 'test(receive_retry::)'`、`just cargo test -p uc-application --lib transfer::progress:: --locked` 和上述固定资源脚本。移动设备与物理挂起等未执行项目仍保持原限制。


## 查询快照与最终准入竞争验证

核对只提供已观察的事实；重发仍由原 `ResendEntry` 执行，实际接收、重试 CAS 和清理由原接收／History 负责人决定。本项仅在测试组合中装饰既有完整 `ProgressSnapshotProviderPort`：原 owner 生成真实页后暂缓返回，不修改内容；必要时暂缓新尝试的通知登记，使旧快照真正进入重发路径，而非提前在发送侧返回 Superseded。没有新远端下载／取消动作、公开阶段入口、协议字段、持久格式或预算变化。

| 真实双 Engine 交错 | 原负责人给出的结果与证据 |
| --- | --- |
| 取得旧快照后，接收端先接受新重试，再恢复旧快照触发显式重发 | 发送端确实采用旧 Settled 快照并请求投递；当前接收持有原内容身份锁，重发记录保持 Pending。释放真实输出后，等待第二次同内容入站完成及最终 ACK 落入 Delivered／Duplicate；接收仍为该新尝试，既有两成功路径保持，总获取／导出均仅四次，最终三项成功。 |
| 取得旧快照后，接收端删除完成，再返回旧快照 | 默认再次发送只核对，旧结果仍可作为真实历史事实显示；旧本机重试输入返回 SourceUnavailable，条目 NotFound、接收状态 None，后续网络核对 NotRetained，没有重新获取、导出或复活已删条目。 |
| 实际远端查询已取快照但尚未返回时，发送端删除源条目，再完成显式重发等待 | 原意图已经退休，返回 Superseded、current None；明确确认也不发起投递。源条目、查询及列表不复活，后续重发返回 EntryNotFound；远端已有两份文件及接收尝试保持。 |

三例在 Nextest ci profile 分别通过 8.030／4.025／3.014 秒。第一例必须从持久 Pending 走到真实后续入站和 ACK，不能仅凭前台等待超时证明接收端最终检查。第二例不禁止删除完成后用户明确创建的全新传输；它验证旧查询和旧重试输入不构成恢复权限。

原 Application 手动重试组 30 项通过，覆盖准备后尝试／成员／身份变化、旧动作未结算、晚到清理及关闭围栏；这些受控 port 测试与真实主机交错分开陈述。原 SQLite 重试组 8 项通过，实际验证旧／活动尝试不插入投影、事务失败回滚旧尝试及全部行、重开后结果保持，以及既有内容／目录信封。设备与物理挂起等既有未执行项继续保持原范围限制。


当前源码的完整 Engine 接收范围共 56 项通过：新三例、其余 46 例，以及固定九项资源入口中的原七例接收场景，重复执行不叠加计数；Collector 专属项仍跳过。Application 进度组另 78 项通过。固定九项原生资源入口全部通过，含编译总 418.426 秒，严格核对 7 场景／8 报告、33 个 SDK 关闭前零临时标签样本，以及原生句柄、FD／线程／任务、连续两份 1GiB 的实际完成／ACK 和原下载 4／2、等待 64／16 配额。测试默认不启用新暂停信号，没有改变原资源入口、断言或期限。

可复现三例：`just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib --locked -E 'test(receive_retry::snapshot_cas::)'`。分层检查为 `just cargo test -p uc-application --lib apply_inbound::tests::manual_retry:: --locked` 与 `just cargo test -p uc-infra-storage --lib file_transfer_repo::retry::tests:: --locked`，完整接收、进度与固定资源入口沿用前节命令。


## 同资料重开与真实进程重启验证

原 Engine 同资料重开案例曾明确期望没有进度卡。新增真实三文件案例先得到红例：条目、目标投递归属及原文件清单仍持久存在，Query 却返回 None。修复将保守读取放回 Application 的完整查询负责人，Storage 复用原文件清单 adapter，以同一只读事务取得本机来源、既有目标归属、加密清单行及表示类型／长度，归还连接后认证这份固定清单。没有新增 schema、明文持久字段、通知 outbox 或永久原始计划映射；不能证明完整旧逻辑范围时不伪造项清单。规则和边界见[重启后的保守发送视图](../../design-docs/transfer-progress-protocol.md#重启后的保守发送视图)。

三个实际 Engine 案例在 Nextest ci 配置下通过：同 profile 关闭重开（3.477秒）；独立 sender 子进程读取同一 profile（6.521秒）；receiver 在三份互不相同的冷内容已经完成 SQLite／文件交付、终态通知尚被原 wire 屏障持有时遭 kill／wait，再启动独立新进程（15.111秒）。前两例均显示三项未确认，attempt、轮次、字节和总量缺失，没有新导出；旧消息头投递成功不解释为远端文件完成。第三例逐份精确读回实际文件，新进程没有再次 protect／导出，原 Source 一次真实状态核对得到 NotRetained，保持0完成、3未确认且没有运行项。子进程测试安全存储只通过匿名 stdin 管道传递；IPC 连接及 child 均在资源收尾前回收，没有把凭据写入普通文件。

receiver 两次新进程固定同一 UDP 端口，以单独核对进程重启。最初改变端口的测试在 Source 核对时得到真实 connection_timeout／1417，没有将传输错误冒充 NotRetained；固定端口后一次核对通过，不增加查询重试、不修改生产预算或失败分类。地址移动仍由后续网络矩阵验收。最初错误关闭发送开关引发的1253也作为测试准备失败保留，已经移除该设置；它不构成生产缺陷修复。

Application 新增历史 get/list 与删除／成员退休交错的四种组合，证明读取不能越过原世代围栏；存储错误保留原 PermissionDenied 来源且不创建运行期 slot。另覆盖纯空目录的单一完整单元、图片位置依据、阈值与混合范围不得只展示子集。完整 Application 当前1399项通过，2个独立驱动／独占日志项按原条件跳过；SQLite 文件清单12项通过，包含认证后清单、来源与目标过滤、删除及全局256／每端64上限。

完整 Engine 接收范围当前59项通过：普通接收入口52项，加原固定资源入口内7项接收场景，重复执行不叠加。固定九项资源全部通过，含编译480.178秒；严格校验7个生命周期场景、8份报告、SDK关闭前零临时标签、原生句柄及FD／线程／任务回归、连续两份1GiB精确读回与ACK，以及原下载4／2、等待64／16额度。资源脚本、断言及期限均保持。

复现入口为 `just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib --locked -E 'test(receive_retry::process_restart::)'`、原完整 Engine 接收入口、`just cargo test -p uc-application --lib --locked`、`just cargo test -p uc-infra-storage --lib entry_file_set_repo:: --locked` 及原固定九项资源脚本。物理挂起、设备／产品宿主及 Collector 等尚未执行部分继续保持未完成，不以独立PID或普通等待代替。


## 生产新旧端网络矩阵验证

[完整入口](../../../scripts/testing/run-progress-connectivity.sh)复用原独立进程宿主与隔离网络驱动，不复制 Engine 业务。新宿主只通过稳定 Query／List／Reconcile 及本地完整接收查询读事实；独立 `transfer-progress` feature 只加入新端默认能力，冻结构建器沿用的 `current-engine` 集合保持。固定 main `5cbca8fc24a278fa7b01ae611bf4b925914bef13` 和 a07 `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6` 均以原库实际构建，没有补丁旧 Engine。先保留两种旧端各自 Direct／Relay 的四格旧旧，再执行新新两格与两种旧端×新端两个角色×Direct／Relay八格；本机首次完整运行14格全部通过，无跳过。

每格均逐字节读回双向文件，混合格额外从独立原始副本升级一侧，校验原身份、传输身份、成员、内容及旧文件继续可读。原旧进度 codec SHA-256仍为`4f8130692441496a436be085f09691cd60236b9c3cc509803a56c504c6d61d0b`。新阶段产生20份方向进度：4份实际v2，发送侧单项完成并明确核对Settled；16份实际v1，新发送端面对旧接收端保持Unsupported／未确认、没有旧attempt／字节／轮次伪造，旧发送端面对新接收端则验证实际本地保存及没有凭入站条目虚构出站意图，不要求旧端实现新版查询或ACK。原API返回的源条目和实际读回的接收条目直接驱动检查，不注入任务身份。

原实际握手观测曾将进度归为`other`，不足以证明版本。本次在既有封闭协议词表加入进度v1／v2，并由同一`ConnectionLifetimeObservation`保留握手时已观察的选路分类；不输出原始ALPN、地址或业务身份，不增加业务root或Engine内部接口。缺少选路字段的旧记录仍可读取，但不算新路径证明。20份进度握手分别实测Direct十份、Relay十份，强制Relay同时核对两端IPv4／IPv6真实丢弃计数；不以presence在线通道或版本号推断进度连接。

[严格汇总器](../../../scripts/testing/verify-progress-connectivity.mjs)要求全部14格、固定版本及二进制摘要、精确文件、原资料保留、原始握手／路径字段、完整方向、正确新旧角色、实际本地结果、保守旧版投影、进程／网络清理及明文探针。证据门禁与原发送报告检查共12项通过，包含缺格组成／缺方向、重复场景、缺原始记录、错误路径、stale host、旧端假成功／字节、伪造出站、清理失败及Relay任一协议族未命中的拒绝；它们不冒充实际网络结果。协议全集77项、观测合同全集84项通过；旧无路径记录及任意地址／ALPN拒绝仍有合同检查。

原 Blob 矩阵继续使用它原先的 main `a461b89444efd60081c7ef63f927946f9dfc072d` 与a07，不替换成更易通过的版本；本次原十格也全量通过。新14格运行后到原十格结束，原矩阵记录的3062个源输入摘要逐项保持一致。此前旧旧Relay的历史`apply_failed`未在本次全量复现，不能因此登记历史根因已修复。CI原网络job现在先执行新14格（内含原旧旧4格），再执行原Blob10格，保留原入口和期限，并上传原始JSON／日志／构建来源。当前候选远端CI、物理挂起、设备／产品宿主、三端source/provider分离与Collector仍按后续未完成项验收。

复现命令：`UC_NETWORK_UNPRIVILEGED=1 bash scripts/testing/run-progress-connectivity.sh`；具备既有sudo网络权限的CI沿用原方式。当前本机工件分别位于`target/progress-connectivity-evidence/runs/local-20261027-1/`与`target/blob-connectivity-evidence/runs/progress-regression-20261027-1/`，两次完整入口均保留首次运行和源码／锁文件／二进制及驱动摘要；全部节点与隔离拓扑实际清理通过。


## 三端消息来源与提供端、双目标进度归属验证

新增 `crates/uc-engine/src/runtime/blob_source_authorization/ownership.rs`，复用三个真实 ProductionRuntime 的既有完整 Application facade、独立 profile、Endpoint、当前成员授权和实际 SQLite／文件输出。三端位于同一测试进程；该证明不冒充三个独立 PID 或平台剪贴板捕获。源身份取自实际发布结果，原 raw 发送入口建立临时所有者，不向进度投影注入身份或成果；不扩大 Engine 稳定入口。

| 实际场景 | 观察结果 |
| --- | --- |
| C 实际发布内容，B 发送引用 C 票据的消息，A 在真实导出边界暂停 | A 实际从 C 获得内容，B→A 保持 Processing／0完成。C 同时收到广播，其自指票据在缓存读取前被原对端准入拒绝为 AccessDenied；B→C 的真实失败不覆盖 A。释放 A 后，B→A 才以自己的 attempt 完成。 |
| B 实际发布新内容，同一消息发给 A／C，A 输出暂停 | A／C 各观察到该 digest 的真实冷缓存未命中。C 先完成并取得自己的源端完成视图；A 仍保持 Processing／0完成，attempt 与 C 不同。释放 A 后，A 的原 attempt 完成，C 的 attempt 与逐项结果保持。 |

每份成功成果都通过原 `GetHistoryEntryResource`／`ReadBlob` 读取最终文件 URI，确认仍在该接收端受管目录中并逐字节读回；没有把 export 中间临时文件当作最终成果。提供端 C 查询 B→A 的出站意图仍得到 None，不能凭提供字节取得消息作者的流程所有权。发送前经原 `QueryPeerConnections` 确认两个真实接收目标可达。

首次用例错误地预期 C 自指缓存可成功，实际得到 AccessDenied。追溯确认 `MembershipLedger::admits_inbound_peer` 只放行对端，原 Blob 路径在读取缓存前执行同一准入；本项因此纠正测试预期，没有放宽生产授权。另一次在第三端加入后的成员前置等待超时（Active2／期望3），原始失败保留，不声称成员传播根因已修复。修正后的两种完整场景在普通测试中通过23.75秒；增加最终反向隔离断言后，原 Nextest ci 独占／60秒／零重试配置通过21.541秒。

本次原 Native outbound 13项全绿（1.14秒），包含真实专属 profile 的旧登记／旧 attempt／迟到精确 ACK 拒绝，以及同绑定中旧 round／sequence 乱序、终态冲突不回退；Application progress 82项通过（16.91秒）。完整接收其余52项通过（475.857秒），原第三方提供端撤销后缓存仍拒绝的真实用例通过（11.762秒）。原固定九项资源全绿（364.918秒，含编译475.028秒），严格核验7个生命周期场景／8份报告／33个SDK关闭前零临时标签样本、双1GiB精确文件与ACK、FD／线程／任务／原生句柄，以及原4／2、64／16下载与排队额度。没有调整阈值、预算或启用重试。

以上完成阶段5的三端归属与原乱序组合验收；跨设备真实读流／ACK阻塞隔离另按下一项验证，不能用输出边界暂停代替。物理挂起、设备／产品宿主与 Collector 继续保持未执行。


## 不同接收设备的真实读流与ACK等待隔离

`crates/uc-engine/src/runtime/blob_source_authorization/peer_isolation.rs` 使用三个真实 ProductionRuntime、独立 profile／Endpoint 和原完整 Application facade，由 B 实际发布同一份256KiB文件，A／C 都经原网络冷获取。为只验证 B 的同一原意图，两接收端通过既有 `UpdateMemberSyncPreferences` 关闭向其他成员发起的新剪贴板发送；B→A／C 的原文件投递、当前成员授权和反向进度／精确ACK保留，不降低并发、排队或时间预算。

- 真实SDK读取场景：A 已校验至少64KiB且尚未读完时暂停；C 实际保存完整文件并收到精确ACK。B 的 A 视图保持原attempt／Transferring／0完成，C 是另一attempt的完成。原本机 CancelEntryReceive 返回后，A 的 SDK 已 stopped／finished，原attempt最终取消且只发生一次原SDK接收；C 的attempt／逐项结果和精确文件继续保持。
- 实际ACK等待场景：在既有测试组合根装饰完整 `ProgressReceiverPort`，原owner真正采用 A 终态后才等待，原 Infra adapter 此时尚未编码ACK。C 在同一发送端的另一连接继续完成并取得自己的精确ACK；A已保存成果和发送侧已采用事实保持，但实际ACK观察数为0。释放屏障后，A 获得同一attempt的原生精确ACK，两端结果不变且不新增下载／导出。屏障按真实peer选择，不伪造身份、结果或确认；Drop封口并唤醒全部等待，不持跨peer锁。

两项在原 Nextest ci 的三端独占／60秒外层／零重试配置下通过，分别10.870秒与7.031秒。没有为测试增加稳定 Engine 入口或生产阶段查询；新入站装饰器仅位于 `cfg(test, dev-tools)` 的既有完整协议工厂。

首次读取场景在原取消成功后仍看到 receiving。临时只读 SQLite 对照证明实际已有另一个attempt，原有日志进一步定位为 C 完成后的活跃剪贴板广播／A拉取动作；它在原取消前到达，之后独立升级部分条目。问题是场景混入了另一条同步动作，未更改生产取消或活跃剪贴板语义来凑结果。改用上述原成员偏好隔离新内容发送后，原取消／同attempt／单次SDK接收断言通过；临时 Python 和诊断启用代码已删除，原始失败输出保留。

同时扩展原 `real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup`：真实67项超过每端64项终态保留容量，原实际下载／保存／取消继续。原系统时钟跨过60秒无联系期限后，源视图保持原attempt，62完成／2取消／3未确认，失败／运行／等待均为0，已知64项逐项事实不变；原完整核对随后恢复65完成／2取消，不新增protect／导出／缓存未命中。该扩展在原120秒外层／零重试配置通过69.739秒，未改产品期限或资源阈值。

本轮完整 Application progress 82项通过（16.50秒）；原Native progress选择86项通过（62.814秒），包含实际16peer保留及61.025秒工作／空闲回收；原接收／三端归属／来源授权54项通过（501.616秒）。原固定九项资源全绿（含编译472.524秒），严格校验7个生命周期场景／8份报告／33个SDK关闭前零临时标签样本，原双1GiB内容／ACK、内存／FD／线程／任务／原生句柄及4／2、64／16额度保持。增强后的超容量项在该完整资源配置下通过92.426秒，仍在原120秒外层内。

以上完成本阶段跨设备读流／ACK隔离与超载未知结果验收；连续通知／完整查询／重连无持续增长仍按下一资源项独立验证，不因原资源9通过就自动勾选。主机结果不冒充物理挂起、设备／产品宿主或 Collector 验收。


## 大文件与连续进度核对及重连的资源验证

在原 `crates/uc-engine/src/testing/host_adapter_contract/blob_resources.rs` 的两轮实际1GiB流式文件交换中补充进度验收，保留原文件生成、128KiB最大块、精确内容读回、完整入站／剪贴板ACK完成边界、50ms进程采样、关闭与空闲回收。没有用小文件替代该原负载，也没有改变内存／FD／线程／任务或原4／2、64／16下载与排队阈值。

每轮完整交换后，通过原稳定操作确认发送侧 Completed 与实际接收attempt，并观察原反向终态的精确ACK；随后执行3批、每批4次 QueryOutboundProgress 与完整 ReconcileOutboundProgress，在批次之间关闭原受管连接，再以原核对动作触发真实新拨号。两轮共24次查询、28次完整核对、4次实际关闭／重连，始终保留原attempt与逐项成果，不增加源文件读取、接收缓存未命中或导出。测试只在既有受管backend装饰器中持有 PeerConnection 弱引用，不新增原生池、业务阶段查询或稳定Engine断连入口。

原Iroh wrapper的 `test-util` 计数同时观察实际进度协议的 WeakConnectionHandle，仅记录技术计数，不返回连接／身份／内容，不添加任务或延长连接生命。每批完整回复结束后，按原20秒收尾窗口等待资源返回同一原预算并连续两次观测稳定，记录瞬时峰值和实际收尾毫秒；不是清计数、降低缓存或把尚未收尾的工作纳入新基线。原 Resources.include 补齐 runtime_tasks 的真实峰值，其他原指标和阈值不变。

首次小文件纵向立即采样时任务数超出首批+4；该红测保留，没有放宽上限。按上述原收尾边界观测后，原64KiB索引采样边界用例通过32.271秒，各批实际收尾52–206毫秒，未越过尚未返回的原索引能力。该小文件只验证完整采样接缝，不冒充后续1GiB证据。

原双1GiB用例通过62.315秒，实际结果如下：

| 原完整文件轮次 | 累计实际进度Update | 查询／完整核对／实际重连 | 每批原生活跃进度句柄 | 每批FD | 每批运行期任务 |
| --- | --- | --- | --- | --- | --- |
| 第一轮1GiB | 80 | 12／14／2 | 4／4／4 | 56／56／56 | 148／147／147 |
| 第二轮1GiB | 162 | 12／14／2 | 4／4／4 | 56／56／56 | 133／132／132 |

原生进度端侧句柄累计4→6→8→10→12，关闭后12个已观察句柄全部失效、活跃数回0；每批实际收尾102–307毫秒。峰值RSS相对原准备基线增加3824KiB，仍按原半个1GiB上限判断；原跨轮任务+4、FD+4、线程+2、RSS+64MiB和关闭／空闲回初值断言全部保持。

原固定九项资源完整通过（391.548秒），严格7个生命周期场景／8份报告／33个SDK关闭前零临时标签样本及原下载预算保持。原校验器增加两轮进度与跨轮资源证据要求；17项正向／负向检查拒绝缺失通知／精确ACK、假重连、原生计数重置、跨轮增长、关闭泄漏和原负载缩小，不依赖汇总绿标志替代原始计数。

当前冻结Rust源码上的 Application progress82项通过16.08秒，启用test-util的原Native进度86项通过62.735秒，Engine接收／来源授权／三端与读流ACK隔离56项通过519.558秒。共享夹具的原无文件SDK关闭入口另通过32.763秒；最终测试二进制310409160字节，运行前后SHA-256保持一致。原小文件采样边界、双1GiB真实负载和无文件关闭三个入口均验证，没有为补证据重跑已经通过的整组资源。

以上完成本阶段原负载与连续通知／完整核对／重连资源验收。原文件部分失败／目录发布回滚／重试与恢复按下一专项逐条核对，不以本项通过自动代替；主机资源证据不代替物理挂起、设备／产品宿主或Collector验收。


## 原接收语义与目录回滚重启恢复专项验证

完整接收、保存、失败结算及重启责任继续归 Application 原 `transfer/receive` 与 `apply_inbound`；进度只采用已实际结算的成果。逐条核对如下，不能仅以测试名称或批次计数代替文件／SQLite 断言：

| 原约束 | 实际断言与证据范围 |
| --- | --- |
| 独立文件部分失败不撤销成功项 | `receive_retry/save_failure.rs` 的首／中／末保存故障均精确读回其余两份文件；实际 SQLite 保留网络 completed 与本地 storage_unavailable，不将已成功的传输改写为失败。 |
| 单项重试只获取所选失败项 | 原 `receive_retry.rs` 及 `batch_outcomes/retry.rs` 核对原成功路径不变、protect／export 共3→4且第四次只对应失败来源；重复请求 AlreadyReceiving、旧尝试 Superseded，源端按新 attempt 采用而不覆盖未选成果。 |
| 授权与删除沿用原完整动作 | 撤销来源后重试在缓存复用前返回 AccessDenied、获取／导出计数仍3、原成功文件保留；放弃删除等待者后仍等实际导出退出，再删除文件及接收事实。不是让取消隐式删除先前成果。 |
| 目录只交付完整单元 | 原三个真实目录案例覆盖双成员、最终提交失败和无 blob 空目录；成功精确读回目录及成员内容，失败不能把成功子文件算成交付，原成员终态修复期间父动作不能结束。 |
| 已发布根的撤回失败仍有恢复责任 | 新真实 Engine 场景通过原 ObserveClipboardChange 发送三个根：前两根走实际 FsAtomicPublisher，第三根在原 publisher 外注入固定 IO 失败；第二根实际撤回一次，首根撤回注入 IO 失败。实际 SQLite 为 publishing／可见根1／landed=0，无保存条目，原查询为 failing，不能假称结算完成。 |
| 重启只处理原输出 | 上述接收子进程真实 SIGKILL／wait（信号9），此前已改变当前保存目录；用户复用已撤回第二根的目标名称。原 Engine 启动 readiness 在旧保存目录撤回首根、清理全部 staging，保留用户新文件与无关文件；原 attempt 变 failed、可见根归零，无 pending 产物、无保存条目。partial_publication 表示已记录检查点，归零后仍为 true，不能误当未清理。 |
| 文件成果及未确认进度的进程重启 | 原 `receive_retry/process_restart.rs` 分别启动独立 sender／receiver 子进程；精确读回已保存文件、无额外 protect／export、无旧通知恢复，源端完整核对保留未确认。范围及原始结果见[进程重启验证](#同资料重开与真实进程重启验证)。 |

新场景复用原隔离子进程及完整生产对象图，安全存储材料只经原匿名 stdin 管道移交；没有新增 Engine 阶段接口、协议／持久化版本、通知 outbox 或下载预算。真实移动由原文件系统执行，故障由测试装饰器注入，不宣称模拟了物理磁盘损坏。初两轮候选失败保留原输出：子进程测试捕获隐藏了 panic，打开 nocapture 后确认是预期 receiving 而实际为 failing 的错误断言；按既有契约修正，不改生产状态来适配测试。

当前候选针对新增及共享目录夹具重新执行：Engine 目录／原文件强杀恢复7项通过（64.822秒），Application 目录及原 reconciliation 86项通过（7.233秒），真实 SQLite 目录提交／加密发布日志／产物日志8项通过（0.356秒），原文件系统发布9项通过。Application 的受控 port 测试用于证明流程分支，不冒充完整原生链路；SQLite 合同证明原子提交、缺日志回滚、密文关联与锁定拒绝；完整 Engine 场景另覆盖生产 V3 资料与实际文件。

复现入口：

```bash
just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib --locked --no-tests fail -E 'test(offline_lifecycle::crash::) | test(batch_outcomes::directory::) | test(save_failure::directory_settlement::)'
just cargo nextest run --profile ci -p uc-application --lib --locked --no-tests fail -E 'test(directory) | test(transfer::receive::reconciliation::)'
just cargo nextest run --profile ci -p uc-infra-profile --test directory_receive_commit_contract --test directory_publish_log_contract --test receive_artifact_log_contract --locked --no-tests fail
just cargo test -p uc-infra-local --lib fs::atomic_publish::tests:: --locked
```

其余原文件／授权／删除／单项重试及进程进度证据继承上一层 #321 已通过的56项接收回归；生产 Rust、依赖、原固定九项资源入口和门限均未修改，不冒充本轮重跑这些整组。原两轮1GiB、下载4／2、排队64／16及严格7场景／8报告／33个SDK关闭前样本仍见[连续资源验证](#大文件与连续进度核对及重连的资源验证)。本项闭合阶段5原接收专项；workspace／成员／升级全集、物理挂起、设备／产品宿主及 Collector 的其他验收仍按原未完成项执行，AC11整体不因此自动完成。


## 用户完整核对的业务观测与保留连接结算

完整业务仍归 Application 原接收、手动重试及进度核对负责人。Engine 只装饰原 `ReconcileOutboundProgress` 的完整 facade 调用，按其原返回结果生成独立 `clipboard.reconcile_progress` 业务记录；不读取内部阶段、身份或状态对象，不增加 port、持久字段、协议版本或通知 outbox。原返回值及错误 source chain 原样返回。普通 Query／List 不建业务根，内部再次发送的核对仍归原 resend 动作，heartbeat／ACK／补发不延长已经结束的接收或重试记录。

最小真实 Engine 用例先红：完整核对已经正确返回，但原始 OTLP 中没有对应业务根。修复后通过两个实际 Engine、原生连接、SQLite 和真实文件取得七种结果：运行、两次结算、旧尝试、未保留、非法输入和成员拒绝。三文件原接收两项成功、一项失败，原失败项手动重试后 exports 从3到4且原成功文件保持；终态通知被原 wire 屏障实际持有时，接收 partial 与重试 ok 记录已经结束并可导出，核对计时不回挂旧动作。另六次普通读视图不产生业务根。

首次最小绿例的原始记录包含49个 span、51条日志；七条完整核对各有唯一同 Trace／Span 的完成日志，使用固定结果名和分类。实际文件名、源／接收条目及目标设备标识的哨兵均不在原始 protobuf 中。观测合同68项、完整 runtime101项及 Collector 隐私5项通过；runtime 原30秒普查测试1项按原配置忽略，记为跳过。另一真实 HTTP 编码用例逐一覆盖全部12种固定结果，并拒收私有／伪造名称和错误角色。固定词表及计时结束规则只在[观测标准](../../design-docs/observability.md)维护；这些记录器证据不等于真实 Collector／Jaeger 或浏览器验收。

原完整固定九项资源首次出现8通过、1失败，不能用隔离重跑成功覆盖。两轮1GiB内容交换已完成，但连续完整核对的强制重连报 Engine1417。保留原日志及报告后，以原64KiB完整采样入口复现，错误链定位为连接名额 Busy：原受管保留连接已报告失活，新 flight 先取方向名额，而原关闭监督及本地工作尚未共同退出。新最小 manager 合同0.005秒失败，分别控制真实关闭结束与本地 work 释放，证明不能提前拨号。

修复由原 ConnectivityManager 独占：只保存受管保留出站连接原 lifetime 的完成通知接收端，不保活旧 record／lifetime，不另造许可权威。原 flight 在同一等待额度、取消与绝对期限内等实际结算，然后取额一次；完成通知已在首次取额失败后到达时也直接处理，避免返回过期 Busy。未保留连接仍沿原 Busy。首版等待修复触发原 Native 显式退休回归：调用者仍持 work 时应立即 Busy。进一步以同一 watch channel 精准撤下显式退休的通知，旧 lease 不能撤下新连接通知；原 Native 测试未改。三个 Application 合同覆盖关闭／本地工作两顺序、原截止／取消／会话封口和显式退休，方向、排队、下载及资源阈值均保持。


最终候选重新执行 Application 连接及 Space 连接126项（0.420秒）、原生进度86项（62.869秒）、Application 进度82项（16.49秒）、Engine 接收57项（517.607秒）全部通过；不使用被中断的42／57日志作完整通过证据。真实观测用例在该完整 Engine 组中通过4.157秒，原始52个 span／54条日志，七条核对及原接收 partial／重试 ok 的断言保持，终态仍实际持有、exports4。总节点数受实际诊断活动影响，不作为业务完成判据。

原固定资源九项最终整组通过424.642秒，严格校验7个生命周期场景、8份报告、33个SDK关闭前零临时标签样本。两轮真实1GiB用例93.004秒通过：精确文件与ACK、累计198次Update、24次查询、28次完整核对、4次真实重连，每批原生进度活跃句柄保持4，关闭后已观察14个句柄全部失效、活跃归零。每批实际收尾51–204毫秒，原内存／FD／线程／任务及4／2下载、64／16排队阈值保持。原67项超容量场景93.127秒通过，仍在原120秒外层预算内。64KiB原完整采样边界另通过32.563秒，不能用它代替上述原1GiB证据。

复现新增观测及原资源入口：

```bash
just cargo nextest run --profile ci -p uc-engine --features dev-tools --lib --locked --no-tests fail -E 'test(real_engine_reconciliation_exports_its_result_without_extending_receive_or_retry)'
just cargo test -p uc-observability-runtime --test progress_reconciliation --locked
node tests/observability/collector/collector-privacy.test.mjs
bash scripts/testing/run-blob-lifecycle-resources.sh
```

本项完成原完整接收／重试与明确用户核对的观测边界，以及验收中发现的保留连接关闭结算兼容修复。真实 Collector／Jaeger 列表和详情、物理挂起、跨平台候选CI与产品宿主仍按各自未完成项执行，不因原始OTLP或主机资源通过自动记为通过。


## 真实 Collector 与 Jaeger 的业务验收

本机 Linux arm64 使用已校验官方 SHA-256 的 Collector Contrib0.160.0 和 Jaeger2.20.0 原生二进制；Docker socket 不可用时没有修改系统权限或服务。Collector 沿用仓库严格模板，只将接收与 Jaeger 导出地址替换为 loopback；Jaeger 使用隔离内存存储。两份配置均由实际二进制 validate 通过，不删过滤条件、不扩展字段、角色或错误词表，没有实际投递 PostHog。

原正常目录用例首次虽通过实际文件／SQLite 断言，却未安装和刷新 OTLP，不能当作观测通过。现在仅在该原测试显式设置 OTLP 环境时安装既有运行时，并在原完整流程退出后刷新；保留原文件、计数、接收、核对和关闭断言。正常目录、部分失败／成功重试／取消、七次完整核对三个真实 Engine 场景分别通过8.304、11.595和4.462秒。前两者直接经过原始字节记录器转发到严格 Collector；七次核对沿原测试记录真实 protobuf，再逐份原样投递同一 Collector，不以 Mock 记录器冒充后端。Jaeger 按确切 Trace／Span ID 逐条保留七次核对的固定名称、实际结果及唯一完成日志。

Chromium 使用独立临时 profile，通过真实列表链接进入详情，展开 Tags 与 Process。以下八类具体样本均检查列表筛选、完整父子树、同节点同 operation 的唯一完成日志及实际可见分类；所选样本的 span 时长与日志毫秒值偏差均小于1.3毫秒。节点数量只描述样本，不作业务成功判据。

| 具体样本 | 详情中的固定结果 | 本次可解释的事实 |
| --- | --- | --- |
| 正常目录接收 | `clipboard_receive`／`ok` | 实际目录作为一个完整单元交付，两个成员逐字节读回。源端复制动作仍可为 deferred，不能用接收成功改写原异步发送结果。 |
| 文件部分失败 | `clipboard_receive`／`partial` | 两份成功文件保留，不能用消息接受或末项成功推断全部交付。 |
| 实际保存失败子节点 | `clipboard.export_file`／`error`／`storage` | 在同一发送树内展开真实失败，不把它升为新的业务 root。 |
| 用户取消重试 | `clipboard.retry_receive`／`cancelled` | 等实际受监督输出退出，保留原成功文件；不是停止等待就假称取消完成。 |
| 所选失败项重试成功 | `clipboard.retry_receive`／`ok` | exports3→4，仅重取所选项，原成功路径保持。 |
| 未保留的远端事实 | `clipboard.reconcile_progress.not_retained`／`deferred` | 保持未知，不改写为文件失败。 |
| 完整核对已结算 | `clipboard.reconcile_progress.settled`／`ok` | 表示已取得完整事实，可以包含文件失败，不表示全部文件成功。 |
| 原任务仍运行 | `clipboard.reconcile_progress.running`／`deferred` | 继续原任务，不发起新下载。 |

原始输出与设备测试均排除实际文件名及源／接收条目、目标身份哨兵。真实 Collector 另验证五种合成反例：私有展示名称、错误角色、额外内容字段、非法环境和日志正文。HTTP200不算导出成功；逐个反例 ID、哨兵及后端 trace 集合证明全部被过滤。JSON 正向对照重送同一真实 span 时，Jaeger memory 会保存重复项；该探针 epoch 单独保留，最终业务验收从全新内存实例重新执行，不对结果去重后冒充唯一记录。

最终 test 查询有92条 trace／395个 span，production 查询为0；浏览器的默认 test 业务入口及 production 空入口另实际检查，保留18张列表／详情截图。这是隔离测试实例中的筛选证明，不是实际产品、其他平台或 PostHog 的验收。

### 远端丢弃的实际边界

不能把上述八类已逐条核对的具体样本扩大为所有记录完整。采样期间另有两条正常接收 span 缺少远端完成日志，汇总保留 `all_observed_samples_complete=false`，没有将它们记为完整样本。原用例重复执行用于定位丢失点，完整样本不覆盖首次缺失。设备原始 protobuf 已无对应日志，排除 Collector 删除；启用 Nextest 成功输出后，同一正常动作的本地成功完成记录与 `uc.observability.log_dropped`／`queue_contended` 健康记录同时可见。

这是[既有非阻塞容量门合同](../../design-docs/observability.md#分层责任)允许的发送前丢弃：争用立即丢弃并计数，不能把远端记录当耐久交付依据。本项不将容量门改成阻塞、不补通知 outbox、不重放业务来伪造缺失记录。查询和文件成果仍由原 Application 权威决定；缺日志样本不能仅凭剩余节点判定完整。实际丢弃和选定完整样本分别保留，未宣称全量零丢弃或系统日志可靠交付。

复跑入口、原始核对的投递步骤及浏览器筛选见[本地观测入口](../../../tests/observability/collector/README.md#文件进度场景复跑)。原始 protobuf、系统及 Collector 输出、JSON报告与截图按仓库规则保存于本地忽略目录；正式结论不依赖临时 trace 地址长期存活。全部本任务后端、记录器及隔离浏览器已退出，端口回收，下载工具与临时浏览器 profile 已清理；共享 Cargo target 与缓存保持。物理设备、真实产品宿主及跨平台候选CI仍按各自未完成项执行。


## 当前候选跨层定向合同核验

本轮固定候选为 `fc7578e5f8582f4605a8aa26f0044b48d8ca3718`（#325），工作区干净。3412份源码／依赖及文档文件与 #325 交付快照一致，整个测试期间指纹保持 `39e294355d44cb76841e6945a7912ceda610a66868a60e55597e0e5d70049102`，没有修改生产代码、测试断言、阈值、重试、feature实现或持久格式。

协议／Core／Application lib共1844项通过（73.263秒）；原独占维护诊断另精确运行1项通过。原 `run-progress-registration-proof.mjs` 驱动当前 Application worker 与真实 Iroh，8项通过0.72秒，分别记录两个当前二进制SHA、clean来源、1024项实际准入计划、16页／每页64项、65536字节帧上限。状态payload明确为synthetic，不能冒充1024份真实文件传输或物理资源证明。

其余13个包统一构建后先生成 Nextest 实际清单，再执行1736项（812.456秒），全部通过。每条实际PASS与清单的binary／test完整身份及JUnit逐一比对，相同1736项，没有只以汇总绿标志代替覆盖。合计3589个测试案例通过：

| 验证面 | 通过项 | 本轮实际范围 |
| --- | --- | --- |
| uc-sync-protocol | 42 | 消息头／进度帧与固定协议合同 |
| uc-core | 400 | 状态转换、身份、逐项结果与聚合规则 |
| uc-application | 1402＋1 | 完整流程lib及独占维护诊断；注册worker另由原组合驱动 |
| 实际登记权威组合 | 8 | 真实Application CAS＋Iroh、1024项／16页与旧关联退休 |
| uc-engine | 64 | 接收／重试／核对、来源授权、稳定DTO及事件 |
| uc-engine-uniffi | 86 | lib、公开合同、生命周期及平台边界的Linux主机测试 |
| uc-infra-content | 80 | 内容处理及密文接收提交 |
| uc-infra-crypto | 41 | 加密能力原合同 |
| uc-infra-local | 84 | 实际文件发布／清理及经过时间主机测试 |
| uc-infra-p2p | 413 | 全部lib及两个入站诊断integration binary |
| uc-infra-profile | 301 | 全部lib及六个SQLite接收／产物／目录合同binary |
| uc-infra-security | 122 | 内容保护、密文及安全边界 |
| uc-infra-storage | 270 | SQLite、文件传输／重试、搜索和持久字段合同 |
| uc-mobile-probe-core | 39 | 宿主lib及host_boundary，不能作实机验收 |
| uc-observability-contract | 84 | 固定字段、名称和结果合同 |
| uc-observability-runtime | 101 | 真实编码／导出及有界运行时合同 |
| uc-ohos-napi | 51 | lib及公开、工程与workspace合同的Linux主机测试 |

六个SQLite integration binary为 `entry_receive_attempt_contract`、`inbound_receive_commit_contract`、`encrypting_inbound_receive_commit`、`receive_artifact_log_contract`、`directory_publish_log_contract` 和 `directory_receive_commit_contract`。Engine64项包含原接收／进度及来源授权和稳定合同；明确排除固定资源九项，不能把本轮定向结果替代下一项的原资源脚本、完整workspace、成员全集、升级smoke或网络矩阵。

首次调度的 `test(contract::)` 是子串，会误命中整个 `host_adapter_contract::`。该调度在构建阶段终止143，尚未执行任何测试，原日志保留；不是测试失败后缩小失败集。修正为 `test(/^contract::/)` 后，实际清单确认固定资源九项未被意外纳入，已完成的编译产物直接复用。以下为本轮13包统一定向命令，模式锚点不得去掉：

```bash
candidate_filter='(package(uc-infra-p2p) & (kind(lib) | binary(=clipboard_receive_diagnostic_file) | binary(=inbound_peer_rejection_diagnostics))) |
(package(uc-infra-profile) & (kind(lib) | binary(=entry_receive_attempt_contract) | binary(=inbound_receive_commit_contract) | binary(=encrypting_inbound_receive_commit) | binary(=receive_artifact_log_contract) | binary(=directory_publish_log_contract) | binary(=directory_receive_commit_contract))) |
((package(uc-infra-storage) | package(uc-infra-local) | package(uc-infra-content) | package(uc-infra-security) | package(uc-infra-crypto)) & kind(lib)) |
(package(uc-observability-contract) | package(uc-observability-runtime) | package(uc-engine-uniffi) | package(uc-ohos-napi) | package(uc-mobile-probe-core)) |
(package(uc-engine) & kind(lib) & ((test(receive_retry::) & not(test(real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup)) & not(test(/real_engine_(lock_waits|revocation_during|space_reset_seals|confirmed_local_removal|abandoned_deletion|shutdown_waits_for_failed_file_settlement_repair)/))) | (test(blob_source_authorization::) & not(test(quota::))) | test(outbound_progress) | test(transfer_progress) | test(/^contract::/)))'
just cargo nextest run --profile ci --locked --lib --tests \
  --features uc-engine/dev-tools,uc-infra-p2p/test-util,uc-infra-profile/test-util,uc-application/test-support,uc-infra-storage/test-util \
  -p uc-infra-p2p -p uc-infra-profile -p uc-infra-storage \
  -p uc-infra-local -p uc-infra-content -p uc-infra-security \
  -p uc-infra-crypto -p uc-observability-contract -p uc-observability-runtime \
  -p uc-engine -p uc-engine-uniffi -p uc-ohos-napi \
  -p uc-mobile-probe-core \
  --no-tests fail -E "$candidate_filter"
```

此前1844项和两个显式驱动入口分别为：

```bash
just cargo nextest run --profile ci --locked -p uc-sync-protocol -p uc-core -p uc-application --lib --no-tests fail
just cargo nextest run --profile ci --locked -p uc-application --lib --run-ignored only --no-tests fail -E 'test(=space::membership::maintenance::diagnostic_tests::blocked_updates_explain_queue_wait_and_coalesced_wakes_without_changing_order)'
node scripts/testing/run-progress-registration-proof.mjs
```

实际清单另保留27个原ignore记录和293个表达式排除项，不把它们当作已执行。八项登记证明已由上述driver显式运行；fixture子进程只由所属完整场景驱动。物理挂起、外部／受保护relay、外部旧资料探针、性能／元数据审查工具及30秒启动普查等未独立执行，继续如实跳过。需要真实Collector的独占用例沿用 #325 的相同Rust源码及实际后端证据，不冒充本轮重新启动后端。

绑定与移动宿主结果仅为当前Linux arm64主机上的Rust合同，不代表iOS／Android／HarmonyOS设备、跨平台候选CI或产品采用。本项只闭合阶段6定向合同；完整回归、原必需网络／资源及实机未完成项保持原状态。


## 当前候选完整回归与首次失败保留

2026-10-27完成阶段6第二项的七个原入口。基线为#326的`4c73cf1db28084c90e3d8863e23e2062d0a97bf6`：workspace在干净树执行；随后唯一代码差异为成员验收`assert_snapshot`失败快照，增加问题类型、候选标识、是否当前组、名单完整性和成员数，不输出设备名或用户内容。生产代码、依赖、协议、断言与等待预算均未改。该差异在后续入口中如实标记为modified，逐组校验同一补丁摘要；3412文件快照在测试前后保持`a8478fcaf7ded53e6f10e147a58f4a9c579ce4ad7f50a36815cf831e8cd9d001`。

| 原入口 | 实际结果 | 入口耗时，含构建 |
| --- | --- | --- |
| workspace | 131个binary，4927通过，42原跳过；4927条唯一PASS与JUnit4927项无失败核对 | 1673.514秒 |
| membership-e2e全集 | 56通过，14原跳过；完整重跑PASS与JUnit56项核对 | 1157.023秒 |
| upgrade-matrix --smoke | a13到当前源码5个维度符合原登记：3通过、1明确拒绝、1已知不兼容；其余64项未执行，JUnit5项无失败 | 877.781秒 |
| 原Blob网络矩阵 | 10格全部通过；双向文件精确读回、实际握手、清理与明文检查完整 | 292.980秒 |
| 进度网络矩阵 | 新新／混合10格＋冻结旧旧4格，共14格，无跳过；精确文件、进度ALPN／路径与旧端保守视图通过 | 375.060秒 |
| 原生命周期资源入口 | 固定9项通过，PASS与JUnit9项核对；原严格报告通过 | 1036.787秒 |
| 原可达性网络矩阵 | 11类全部通过，包含20次新新Relay、固定main双向三轮恢复、a07双向恢复及旧版拒绝；逐份JSON无失败且清理／明文检查通过 | 1204.961秒 |

资源入口继续使用两个1GiB文件，双文件场景92.291秒、终态容量溢出92.328秒。原采样与校验确认7类生命周期、8份报告、33次SDK关闭前观测，以及FD／线程／运行时任务、下载配额、SDK标签与原生连接数；没有以workspace中的同名场景代替独立资源入口。冻结Blob main仍为`a461b89444efd60081c7ef63f927946f9dfc072d`，进度main为`5cbca8fc24a278fa7b01ae611bf4b925914bef13`，可达性main为`b6fa0ce6b2efd1c1764e76c0a76537bdeead4e1b`，历史a07仍为`f6f305d9689e4e79e7ab6d0e4921061f9416e4a6`；不替换旧版本制造通过。a13为`5e8d3d4322ca4eb83113f342068d4cb3297ea5eb`，升级smoke的预期拒绝和已知不兼容不改写为功能互通。

### 原完整失败及复现边界

- 首次workspace为4926通过／1失败，真实执行1616.058秒：地址读取诊断默认要求captured，但Linux两个类别实际unresolved。原Linux CI已经使用`UC_EXPECT_DIAGNOSTIC_STACK=available`，macOS另执行captured严格验收。用同一个二进制对照，captured失败、available通过；实际7类仍验证分类、三层source chain与隐私，5类captured，locked／unknown为unresolved空栈。没有改白名单、测试断言或生产采集逻辑。对齐既有Linux CI参数后重新执行原完整4927项，原失败日志和JUnit另存，不用单项绿覆盖全组。
- 首次成员全集为55通过／1失败，611.054秒：`topology::f2_concurrent_leaf_removals_resolve_to_selected_branch`在heal之后、选择分支之前，节点C为active=4、issues=2、revision=36，预期issues=1。原工件分类product_timeout。精确单项一次及原stress-count五轮均通过；仅增强测试失败快照后，原56项全集重跑全部通过，其中F2为40.632秒。**间歇失败根因尚未确定，不能宣称已修复，也不能把六次孤立绿当作原全组结果。**原失败、孤立诊断和完整重跑分别保留；后续再次出现时用候选事实定位，不放宽数量或超时断言。

复跑沿用计划中的七个原命令。Linux按既有CI为测试组设置`UC_EXPECT_DIAGNOSTIC_STACK=available`，并用`UC_TEST_BUILD_SCOPE=workspace`保持统一构建图；隔离网络本轮使用原`UC_NETWORK_UNPRIVILEGED=1`路径，实际user／mount／net namespace预检通过，没有修改主机权限。构建串行复用共享target和MBX，固定旧源码临时目录已自动回收，未新增worktree或临时target。原始日志／JUnit／JSON和摘要仅保存在本地忽略的验收工件中，不提交原始输出。

### CI与未执行项

只读核对#326已结束的[CI run](https://github.com/UniClipboard/Engine/actions/runs/40442754299)：iOS、Android、HarmonyOS绑定、原网络矩阵、连接恢复、升级smoke和coverage通过；macOS仓库检查在workspace all-targets编译失败，Engine tests在资源步骤被取消，不能登记整条CI通过。macOS原错误为`peer_isolation`测试引用仅Linux启用的`testing::receive_streams`，条件边界修复及同一交付head的CI核对进入后续任务。本轮不以旧head或本地Linux结果替代跨平台验收。

当前仅闭合完整回归入口，未登记整片完成。物理挂起、真机切网／前后台、Desktop／移动产品采用继续未验收；当前主机没有adb／hdc／Xcode，iOS设备枚举也未成功，不推断设备已经不存在。原ignore、筛选排除项、升级非smoke维度和未运行设备均不记为通过。


## 本地根门禁与读流测试平台条件

2026-10-27闭合阶段6本地根门禁项，基于#327的`484331e5068b75669d11c0cfb8723579c59ce286`。原#326 macOS CI在`cargo check --workspace --all-targets --locked`报告E0432：`runtime/blob_source_authorization/peer_isolation.rs`引用`testing::receive_streams`，而该fixture及`assembly/sync_engine.rs`中的实际装饰接线仅在test＋dev-tools＋Linux启用。父模块已有test＋dev-tools条件，隔离测试子模块漏了Linux条件。

本次为`mod peer_isolation`增加相同Linux条件和中文原因说明，使两项真实读流隔离测试只在已接入屏障的平台编译运行。唯一代码变化位于测试模块声明；Core／Application流程、生产装配、协议、持久化和稳定入口均未变化。没有扩展fixture或更改原测试断言。

Linux arm64使用原workspace统一构建图，精确执行`runtime::blob_source_authorization::peer_isolation::`两项原用例，实际16.593秒、入口含构建546.019秒：

- `adopted_result_waiting_for_its_real_ack_does_not_block_another_receivers_confirmation`通过，5.672秒。
- `paused_native_read_and_local_cancel_do_not_stop_another_receivers_actual_completion`通过，10.910秒。

两项PASS与JUnit2项无失败核对，其他363项为原选择器排除，不算本轮执行。随后根六项metadata、workspace all-targets、fmt、Rust style、repository和diff全部通过；定向测试到根门禁之间3412文件快照均为`fbfc471debc147f4a89880d64e5b108cb7950aae7aced4b3ee02051d8f8d3745`，之后仅回写本节与计划。没有源码／依赖变化时复用前项完整回归，不重复运行4927项或重建固定旧宿主。

复跑定向入口：

```bash
UC_EXPECT_DIAGNOSTIC_STACK=available just cargo nextest run --profile ci --locked \
  --workspace --all-targets --no-tests fail \
  -E 'package(uc-engine) & kind(lib) & test(/^runtime::blob_source_authorization::peer_isolation::/)'
```

本地门禁证明Linux原隔离测试仍实际运行，macOS／Windows实际编译以同一交付head的原CI为准，当前未登记跨平台通过。Windows已有独立`windows-build.yml`，含windows-2025 x64和windows-11-arm原生检查；它的PR触发仅匹配main或migration目标，当前stack需要显式workflow_dispatch。后续使用原工作流核对，不另建重复Windows门禁。原成员F2间歇失败仍未定位，真机请求与未执行项保持原状态。


## 必需套件的整组CI预算修复

2026-10-27核对[原#326 Engine job](https://github.com/UniClipboard/Engine/actions/runs/40442754299/job/121986716097)的原始时间与check注释：03:11:19Z启动、04:11:49Z完成，结论cancelled，注释明确为超过1h0m0s的job上限。原workspace在04:05:35Z完成4927项全绿，耗时1770.029秒；04:05:41Z启动原资源九项，其中7项通过后在04:11:41Z被取消。该次资源入口及之后未执行步骤不能记为通过，也不是业务场景超时已修复的证据。

原`pr-check.yml`的Engine tests串行承担冷构建、完整workspace、登记权威、固定资源、testkit、成员smoke和必要网络步骤，60分钟外层预算已实际阻断完整执行。本次仅将该job上限调整到90分钟，保留所有原单项期限、测试负载、资源上限、选择器和执行顺序。复用现有统一构建与缓存，不拆成重复全量构建的另一套入口；其他job预算不变。

原CI构建／打包配置检查18项通过，根六项metadata、workspace all-targets、fmt、Rust style、repository与diff全部通过。Rust及依赖相对#328未变化，使用已有运行证据，不重跑原4927项或固定网络／资源负载。本项是允许必需CI套件完整运行的配置修复，**同一候选的跨平台CI验收仍未完成**；90分钟预算下的实际线上结果待原工作流验证，不提前勾阶段6对应项。


## 当前生产受管进度接线核对

2026-10-27按阶段2首个未勾父项逐责任核对当前#329候选`d3e4527045ff26beef771c79a2332a2b7df35f4d`。完整负责人是Application的进度运行时：调用方只组装传输能力并注册一个完整入站端点；版本交换、投影、收发监督与实际关闭由同一对象图完成。成功以实际采用和协议结果结算，失败保留原来源；当前会话退休及关闭共同等待已开始的实际工作，重启按原接收／意图事实恢复，不恢复临时连接绑定。

| 要求 | 当前生产责任链与实际证明 |
| --- | --- |
| 两种协议能力与专属profile | Application [ConnectionProtocol](../../../crates/uc-application/src/connectivity/backend.rs)区分V1／V2，Infra [ConnectivityProfile](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/backend.rs)为进度提供固定新旧协商集合；[原策略](../../../crates/uc-application/src/transfer/progress/connectivity.rs)集中定义额度、首流／入站预算及空闲保留 |
| 同一manager与租约 | [Application组装](../../../crates/uc-application/src/transfer/progress/runtime/assembly.rs)由同一manager创建producer、snapshot reader和inbound；[单向租约](../../../crates/uc-application/src/connectivity/unidirectional.rs)的open／accept持有原额度与work lease并检查当前准入，[双向StreamLease](../../../crates/uc-application/src/connectivity/stream.rs)同样持有实际工作使用权 |
| 完整入站负责人 | [TransferProgressTransport](../../../crates/uc-application/src/transfer/progress/inbound/owner.rs)固定首轮经过时间和连接世代，监督manager.accept及真实采用；[交换实现](../../../crates/uc-application/src/transfer/progress/inbound/exchange.rs)对旧单向／新双向使用同一lease及原期限，取消等待不取消已开始的本地结算 |
| 节点与Engine生产安装 | [Engine组装](../../../crates/uc-engine/src/assembly/sync_engine.rs)选择专属profile，把能力交给Application，仅注册其完整endpoint；[节点安装](../../../crates/uc-infra-p2p/src/network/iroh/node.rs)以一个handler注册两个ALPN；[Iroh handler](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/progress_handler.rs)只交接连接和完整shutdown，没有独立业务发送／采用流程 |
| Router真实版本选择 | [生产Router](../../../crates/uc-infra-p2p/src/network/iroh/protocol_router.rs)集中排序协议；`progress_negotiation_chooses_current_regardless_of_registration_or_offer_order`实际验证注册顺序及客户端offer顺序均不改变新版优先 |
| 生命周期实际接线 | [Application启动](../../../crates/uc-application/src/application.rs)启动并持有ProgressRuntime，[顶层关闭](../../../crates/uc-application/src/application/shutdown/owners.rs)实际调用其shutdown；[运行时](../../../crates/uc-application/src/transfer/progress/runtime/owner.rs)共同等待worker和入站关闭，[入站owner](../../../crates/uc-application/src/transfer/progress/inbound/owner.rs)重复shutdown等待同一实际完成结果 |
| 双版本实际收发 | [真实入站组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_inbound_transport_probe.rs)断言登记／终态ACK／查询、旧绑定退休、旧单向保守采用与关闭、原准入经过时间、取消shutdown等待及成员撤销；[实际profile组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_profile_probe.rs)断言受管连接复用；十四格生产网络另证明两个固定旧端与新端的双向Direct／Relay收发 |

上述真实入站六项、profile复用一项和Router偏好一项，在[原完整回归](#当前候选完整回归与首次失败保留)的workspace中均有唯一PASS与JUnit通过项，已经核对实际断言而非仅测试名称。该回归的进度矩阵还保留20份方向记录：新版4份、旧版16份，实际Direct10份、Relay10份，逐份具有真实握手和路径观测；严格汇总要求完整14格、固定旧端SHA、二进制摘要、精确文件、原资料保留、清理与明文检查。

对照该网络候选冻结的源码摘要，当前2134个Rust文件逐字节一致，唯一不同的Rust文件只补齐Linux测试模块条件，已有[两项原测试与根门禁证明](#本地根门禁与读流测试平台条件)。生产连接、Application流程、Engine装配、协议与依赖均未变化，复用已有完整证据；本轮不重新运行网络负载或以新模拟代替生产路径。早期[入站负责人](#阶段2完整入站连接负责人)仅到可安装能力的边界，现在已经由生产对象图、节点安装和实际生命周期接线闭合。

本项仅关闭阶段2的受管生产接线父项。降级触发、逐次授权和会话围栏仍按各自父项核对；AC01整体、物理系统挂起、设备／产品宿主及最终同一head CI没有因此自动完成。原成员F2间歇失败及全部跳过项继续保留，整片仍为active。


## 生产版本选择与旧协议保守语义核对

2026-10-27基于#330的`fbfaae24bc634af899b37e164e3e065488479aa1`，独立核对阶段2降级父项。当前Application／P2P／协议／Cargo清单及锁文件相对原登记组合候选`fc7578e5f8582f4605a8aa26f0044b48d8ca3718`没有差异；本轮只核对并更新文档，不调整协议、失败类别或测试条件。

| 要求 | 当前实现与原实跑证据 |
| --- | --- |
| 只由实际协议能力选择旧版 | [进度专属backend](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/backend.rs)在唯一`connect_with_opts`握手同时提供新旧ALPN，握手失败直接返回带source的Backend错误；[producer](../../../crates/uc-application/src/transfer/progress/producer.rs)只在成功连接的实际协议为V1时进入旧汇总分支，没有错误后改选V1或新建旧协议影子连接的分支。生产Router与十四格矩阵的实际新旧握手见[接线核对](#当前生产受管进度接线核对) |
| 坏新版帧不变成旧能力 | 原实际登记组合的`explicit_legacy_is_unsupported_but_corrupt_current_frame_keeps_exchange_failure`分别断言真实旧连接返回Unsupported、真实新版坏帧返回Malformed且保留source；它默认ignore，**由原driver显式执行，不计入普通workspace通过数** |
| 拒绝与错误关联不降级 | [真实入站组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_inbound_transport_probe.rs)断言成员拒绝后零采用且投影不变；[拒绝码组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_rejection_probe.rs)断言五种拒绝在各响应位置保留类别，错误关联／尾随字节不能作为有效拒绝采用。所有错误沿原结果返回，不改变连接已协商协议 |
| 完整旧批次事实 | [旧汇总](../../../crates/uc-application/src/transfer/progress/producer/legacy.rs)从原authority和snapshot分页取得同一descriptor的完整批次，逐页检查覆盖、当前授权与期限；原6项汇总组合分别断言最后成功但先前失败、Processing、兄弟活动、全部完成、Contact及Update均使用全量事实。原实际CAS组合另断言1024项真实失败事实形成一个Failed旧帧且完整读取16页 |
| FIN不作业务ACK | [旧wire发送](../../../crates/uc-infra-p2p/src/network/iroh/transfer_progress_v2/outbound.rs)等待FIN数据结算；producer的旧终态分支在完整发送后仍返回Unsupported。原实际CAS组合同时验证准确旧帧和没有业务ACK；不得把发送成功改写成对方采用 |
| 入站旧状态不能覆盖新版事实 | [旧入站](../../../crates/uc-infra-p2p/src/network/iroh/transfer_progress_v2/legacy.rs)仅接受实际V1，完整帧与FIN核验后交原owner；[无关联采用](../../../crates/uc-application/src/transfer/progress/receiver_unattributed.rs)仅标记现有原意图Unconfirmed。四类旧状态不产生attempt／单项终态，实际V2连接不能降为旧采用，另一peer同source不得采用，坏magic／status／截断／尾随字节在采用前拒绝 |
| 原期限与失败来源保持 | [旧来源组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_profile_legacy_source_probe.rs)对原期限、取消、旧会话、旧尝试和底层读取失败，断言实际读取被监督至结束且未打开新单向流，失败保留source；[旧入站组合](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/stream_profile_legacy_inbound_probe.rs)另覆盖FIN期间期限／取消／退休及删除边界 |

原完整workspace中`legacy_summary_probe`的21项逐条PASS与JUnit核对：10项旧入站、6项完整汇总、5项来源结算。上述新版坏帧和1024项旧汇总属于[原登记权威driver](../../../scripts/testing/run-progress-registration-proof.mjs)显式运行的8项组合，原日志分别为ok、总计8通过／0忽略，报告保留来源clean与两个实际二进制SHA；不能只凭required_cases清单或默认ignore的测试名称当作执行。该组合的容量状态payload明确为synthetic，实际CAS和Iroh能力仍照实记录，不当作1GiB或独立产品宿主负载。

十四格生产网络另有16份真实V1方向：新发送端对旧接收端维持Unsupported／未确认，未伪造旧attempt、字节或轮次；旧发送端对新接收端只验证实际本地成果，不凭入站内容虚构出站意图。原旧旧4格与新版4份V2方向保持各自边界。原字节、旧端来源、实际文件和资料保留沿用同一严格矩阵与冻结摘要，未以其他版本替换。

本项关闭阶段2“真实能力才使用旧协议”父项，不自动关闭下一项逐次授权／会话围栏或AC01整体验收。原F2间歇失败、物理挂起、设备／产品宿主和最终候选CI继续按原未完成状态维护。


## 当前授权与原会话围栏核对

2026-10-27基于#331的`d5f974e4c128f72c0bb4e13612fbae916156617b`，核对阶段2最后一个授权／会话父项。当前Application／P2P／协议／依赖相对原实际权威组合候选保持一致。本轮只更新验收文档，逐当前实现、原断言和实跑记录证明旧动作不能借新会话重新取得资格。

| 边界 | 生产责任与实际证明 |
| --- | --- |
| 排队及本地读取之前固定资格 | [manager.request／freeze_request](../../../crates/uc-application/src/connectivity/manager.rs)取得原session和admission attempt；[ConnectionRequest／ScopedAttempt](../../../crates/uc-application/src/connectivity/request.rs)在异步资格读取前后核对同一session／fence。[producer](../../../crates/uc-application/src/transfer/progress/producer.rs)在本地authority读取之前创建request，[snapshot reader](../../../crates/uc-application/src/transfer/progress/snapshot/reader.rs)在round future入队之前冻结request；refresh沿用原attempt、session及deadline |
| 当前成员与实际认证身份 | [Iroh准入](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/admission.rs)核对握手公钥对应身份与目标peer，成员读取前后都检查原RevocationEpoch；Grant保留原epoch，每次check_current读取当前成员，进度只接受实际V1／V2协议。[pending入站](../../../crates/uc-application/src/connectivity/pending.rs)在身份解析／grant绑定前后检查原session与入口revision fence |
| 网络阶段及采用前再次检查 | [ConnectionLease](../../../crates/uc-application/src/connectivity/connection.rs)在原request及grant实际读取前后检查stop；[新wire Exchange](../../../crates/uc-infra-p2p/src/network/iroh/transfer_progress_v2/io.rs)在read／write／FIN阶段授权，FIN等待结束后再次授权才进入采用。[旧wire](../../../crates/uc-infra-p2p/src/network/iroh/transfer_progress_v2/legacy.rs)同样在完整帧／FIN后重新核对；本地读取失败仍保留原source |
| 晚到拨号与关闭竞态 | manager在flight结果发布前检查manager／flight取消，失效结果不能成为新活动记录；原App用例断言拨号期间撤销后晚到实际连接关闭，并在64轮完成拨号／shutdown竞争中检查没有连接逃逸。旧排队请求、身份解析scope及其副本不能借新epoch或新缓存连接继续 |
| 旧本地读取与排队查询 | 原4项authority-session用例验证入站／出站资格读取跨session或shutdown后返回Stopped、失败source仍保留；第二backend的注册／Contact／Update／Terminal四项在读取已进入后退休session，均断言零新拨号。排队Query旧动作同样零拨号且不改投影，新的显式Query仍能完成65项两页查询 |
| 删除与新绑定不能被旧动作复活 | 第二backend分别在删除来源、移除peer、清空投影和关闭owner时暂停真实Challenge后的读取；重建同key后迟到登记／旧终态不能更新新投影或发出新事件。原实际Application权威子进程重开及Iroh新连接组合另断言新producer与新binding生效，旧挂起交换和旧终态不能覆盖新binding；这是子进程权威与真实Iroh证明，不冒充完整设备进程或物理挂起 |
| 实际成员撤销与来源归属 | 真实Iroh profile组合断言进度退休不影响同peer剪贴板连接，实际成员撤销同时关闭两者且错误协议不能借用进度grant；生产node用例先证明旧进度连接有效，再移除成员并等待实际关闭。原Engine三运行时证明消息来源、内容provider和两个接收目标的attempt／成果相互独立 |

本次从原完整workspace逐条提取22项，核对每条唯一PASS与JUnit无failure／error／skip，覆盖App 9项、P2P 12项和Engine 1项。权威子进程重开、当前Challenge与晚Bound等另来自原显式8项driver，按[版本核对](#生产版本选择与旧协议保守语义核对)保留其真实执行、来源及二进制摘要；默认ignore没有计作workspace通过。第二backend使用实际Application流程与codec，但身份／传输fixture受控，其零拨号／旧绑定隔离断言不作为密码握手、Direct／Relay或真机事实；上述真实Iroh、生产node、Engine与原十四格矩阵分别补齐对应层级。

当前[生产受管接线](#当前生产受管进度接线核对)、[版本选择](#生产版本选择与旧协议保守语义核对)和本节共同关闭阶段2生产收发父项。此处不代替物理系统挂起、前后台设备、产品采用或最终同一head CI；原经过时间必需物理项、原F2间歇失败及所有未执行项继续保持原状态，整片仍为active。

## AC01–AC13唯一证据映射

2026-10-27以`91a19e995f88c3fa4555a7f321c03a14e4d2dcc9`为本轮核对基线，逐条对照原spec，不改变验收范围。本表是验收状态的唯一证据索引；spec复选框只引用本表。阶段任务完成不等于整片完成。当前10项已验证，AC08、AC11、AC13仍未完成，必需实机项没有被主机测试替代。

证据复用同时核对来源：自真实Collector候选`fc7578e5f8582f4605a8aa26f0044b48d8ca3718`以来，crates／bindings／Cargo仅有两处测试改动，即成员失败候选快照与Linux读流测试条件；生产实现、依赖及两个绑定不变。原进度网络矩阵冻结的2134个Rust输入已逐项核对，唯一后续差异为该Linux条件，原两个隔离案例另已实跑通过。本轮将原完整workspace的4927条唯一PASS与JUnit逐条核对，均无failure／error／skip；另行执行的忽略项、资源驱动及网络矩阵分别保留自己的证据，不能混入该4927计数。

| 验收 | 状态 | 唯一证据入口及其证明范围 |
| --- | --- | --- |
| AC01 | 已验证 | [受管生产接线](#当前生产受管进度接线核对)、[实际版本选择](#生产版本选择与旧协议保守语义核对)及[生产网络矩阵](#生产新旧端网络矩阵验证)：同一Application连接负责人双版本入站／出站，实际ALPN单次协商；原旧进度codec摘要、blob与资料互通保持，失败不触发旧格式重发。新头`clipboard-applied/3`按已批准范围协商，旧头字节保持。 |
| AC02 | 已验证 | [身份与精确ACK](#阶段1身份与精确ack校验)、[当前权威及原会话](#当前授权与原会话围栏核对)、[重试围栏](#手动重试与旧尝试围栏的主机验证)及[三端归属](#三端消息来源与提供端双目标进度归属验证)：Core登记10项、实际权威driver8项、原会话22项及实际Engine尝试／双目标案例覆盖各自层级；旧轮次、旧描述、旧会话不能覆盖、续期或归还新实例的实际额度。 |
| AC03 | 已验证 | [真实成果与ACK](#真实文件成果与精确ack链路)、[实际结算](#真实本地结算边界与通知隔离)、[逐项批次](#逐项结果与批次汇总的主机验证)及[目录回滚与重启](#原接收语义与目录回滚重启恢复专项验证)：批次7项、真实保存／修复14项均在原workspace通过；包含慢输出、写失败、清理失败、部分成果、单项重试、完整目录和无blob纯空目录。网络完成不能代替成果提交。 |
| AC04 | 已验证 | [采用后ACK](#真实输出后的-application-采用与-ack)、[终态重放与丢事件](#终态丢失后的核对与同绑定重放验证)及[快照竞争](#查询快照与最终准入竞争验证)：实际Iroh丢ACK后精确重放不产生重复版本／事件，冲突与迟到活动不回退终态；实际Engine终态被阻塞后通过完整查询收敛且不重下。Application投影29项和快照CAS3项提供原子采用、事件失效后读取及旧快照围栏证据。 |
| AC05 | 已验证 | [资源策略冻结](#阶段0资源策略覆盖与冻结)、[实际查询监督](#共享查询与终态的实际监督)、[Contact边界](#contact与有效联系视图的边界)、[原生保留](#专属profile与真实连接保留)及[完整回归](#当前候选完整回归与首次失败保留)：下表逐项列出字面预算与实际边界，受控elapsed跳变只证明调度规则，不证明物理挂起。 |
| AC06 | 已验证 | [通知容量隔离](#通知容量与本地接收隔离验证)、[跨设备真实隔离](#不同接收设备的真实读流与ack等待隔离)、[实际结算](#用户完整核对的业务观测与保留连接结算)及[完整资源入口](#当前候选完整回归与首次失败保留)：通知阻塞／淘汰期间实际文件继续交付，取消等待已开始本地工作结算；另一接收设备继续。固定资源9项、7类8报告、33个SDK关闭前样本覆盖真实资源回收，不以调用方取消或已过期限假定工作返回。 |
| AC07 | 已验证 | [原再次发送](#原再次发送动作的主机验证)、[查询快照准入](#查询快照与最终准入竞争验证)及[稳定保守视图](#同资料重开与真实进程重启验证)：Application重发规则16项及真实Engine出站进度7项通过。七种核对结果分别测试确认与不确认，Running不重复启动；NotRetained／Unsupported保留`duplicate_files_possible`且仅显式确认重发，Unreachable即使确认也不重下；查询错误保留source而非改写为缺失。 |
| AC08 | 未完成 | [平台经过时间审计](#各平台经过时间审计与物理挂起入口)、[暂停恢复](#暂停恢复与同资料重开的主机验证)、[会话退休](#会话退休后的确认验证)及[实际进程重启](#同资料重开与真实进程重启验证)已证明各自主机／受控时钟范围。删除、成员撤销、离组、切Space及重启保守视图已有证据；真实系统挂起及移动后台继续计时／恢复先检查仍缺，不勾本项。 |
| AC09 | 已验证 | [生产会话与稳定查询](#生产会话与稳定查询的主机验证)、[生产接线](#当前生产受管进度接线核对)及[跨层合同](#当前候选跨层定向合同核验)：Application唯一持有投影、通知及恢复，Engine历史操作只调用完整facade并映射DTO，没有旧业务表或宿主步骤。两个绑定均为workspace版本，业务依赖只经`uc-engine`；原UniFFI86项、NAPI51项及其中两项出站映射实际通过。 |
| AC10 | 已验证 | [生产新旧端矩阵](#生产新旧端网络矩阵验证)、[三端角色归属](#三端消息来源与提供端双目标进度归属验证)及[完整回归来源](#当前候选完整回归与首次失败保留)：进度14格、原Blob10格和三运行时消息源／provider／双目标分别完成真实读回、角色及来源检查。冻结旧端SHA、当前二进制摘要与修改状态分别记录；三运行时同进程不冒充三台物理设备。 |
| AC11 | 待复核 | [原接收专项](#原接收语义与目录回滚重启恢复专项验证)、[完整七入口](#当前候选完整回归与首次失败保留)及[根门禁](#本地根门禁与读流测试平台条件)：原文件／取消／目录／重试／双1GiB／旧资料及workspace4927、成员56、升级smoke5维度、原网络矩阵与固定资源9均实际执行。首次workspace配置失败和成员F2间歇失败独立保留；原完整重跑通过不表示F2根因已修复。随后本轮只读核对发现#332旧旧Relay基线失败，见下述新证据；保留此前完整通过，但本项撤回完成状态，待失败定位及原入口复核。跨平台最终CI仍由AC13承担。 |
| AC12 | 已验证，保留退化 | [完整业务观测](#用户完整核对的业务观测与保留连接结算)及[真实Collector／Jaeger](#真实-collector-与-jaeger-的业务验收)：8类具体样本逐项对照真实业务结果、列表与详情展开，18截图及5个隐私反例；测试与产品列表隔离，7种核对结果不压成文件成功。非记录型`ObservationContext`只保存父身份，补发worker不持有原接收span；真实接收计时与完成日志对照。两条正常span缺远端完成日志、本地`queue_contended`健康记录和`all_observed_samples_complete=false`继续作为验收退化证据，不能声称全部远端记录完整。 |
| AC13 | 未完成 | [CI预算及原失败](#必需套件的整组ci预算修复)、[当前回归边界](#ci与未执行项)及计划阶段6。中间候选#329的macOS、三端绑定、Windows双架构已有成功结果；本轮最新只读核对时Engine tests仍运行，其余12个PR job成功。该候选不是最终文档交付head；随后核对#332还发现网络job失败，不能据此完成本项。真机切网／挂起／前后台及产品宿主仍未执行；稳定结论最终回写、入口更新与归档尚未进行，不关闭整个#34。 |

AC05的全部默认预算逐项核对如下；计数来自原4927条PASS／JUnit的一致提取，不是本轮重新运行次数。

| 预算／上限 | 实现与实际边界证据 |
| --- | --- |
| 中间值5Hz／任务 | `delivery/live.rs`的200ms间隔；实际worker在工作阻塞时合并序号2–100，工作返回后5199ms仍不发、5200ms发送最新100、5400ms才发送另一项，没有补发突发。 |
| Contact10秒／未知60秒 | `progress_freshness`4项及实际Contact worker：重复／旧联系不续期，producer切换不重置原意图期限；已结算事实不因后续时钟丢失改变。 |
| 终态10分钟／8次／整轮5秒 | Core delivery14项与Application delivery27项：原时点0／2／10／30／60／120／300／540秒、每轮5秒且600秒到期；实际恢复机会与错过时点合并，提前机会仍守前次启动后2秒，工作未返回不重叠，也不释放实际槽位。 |
| 实际工作4／等待64全局、16每端 | 终态与查询共用原exchange配额；真实worker混合占满4槽，排队超限Busy，无第二查询队列；同端终态优先不阻塞其他端轮转。 |
| 查询16观察者／总5秒 | 同查询共享一次读取与原期限，第17观察者Busy；取消一个不取消其余；旧观察者不能借后到观察者预算，实际读取结束前仍占槽。 |
| 投影、终态、中间值各256全局／64每端 | 原Core容量及实际Application溢出／重复／淘汰测试；节流／已见状态另守256／64个任务，每任务最多1024项，淘汰中间值不重置资格；旧实例完成不删除新值，重复不重占或刷新额度；真实Engine67项容量场景继续本地保存并经完整快照恢复。 |
| 登记4全局／1每端、无等待队列 | 实际持有登记读取，超额Busy；退休仅封口，原4／1工作返回前不释放准入，shutdown等待实际返回。 |
| 连接每端每方向1、每端出站1／每连接入站1；保留30秒／16条 | 独立进度profile及原生保留5项，未识别入站4、识别后每端1、已接受流5秒；实际work跨过30秒仍不被清理，工作结束后的真实空闲计时关闭；17个真实peer仅淘汰idle，全部active拒绝新增保留。 |
| 原计划1024／页64／帧65536字节 | 原权威driver实际1024项、16页；新协议快照与旧summary各有独立真实入站证明，帧限制不为最大输入扩大。 |
| 事件失效通道128 | Application广播保持128；事件只提示重新读取，不承载唯一事实。丢订阅／丢事件后完整快照恢复，终态重放不重复推进版本。 |

全部跳过和未闭合范围继续登记：workspace原42项忽略／排除、成员原14项排除、升级非smoke64维度均不计通过；原权威driver与独占诊断仅按单独真实执行计入。物理系统挂起、真机切网／后台、Desktop与移动产品宿主未执行，平台编译成功不代替采用或真机行为。当前没有adb／hdc／Xcode，iOS枚举失败只表示本机未取得可用设备证据，不推断设备不存在。上述未执行项和AC08／AC11／AC13继续阻止整片完成与归档。

### 映射期间发现的旧旧Relay失败

本轮只读核对[当前#332网络job](https://github.com/UniClipboard/Engine/actions/runs/40466433047/job/122062850454)，实际结论为failure。原工件ZIP SHA-256为`ea1fdb275cedd8ac102f4b9f259cb3433207abe97c49d9a0bba1f05079472cb0`，下载后按摘要校验，逐份读取四个旧旧格：冻结main Direct／Relay及a07 Direct通过；a07 Relay在B到A文件投递时`peer_rejected`、接收A为`apply_failed`，该格20.777秒失败，清理和明文检查仍通过。双方实际二进制摘要相同，为`59edcc71dd6a91f561460cd719df86b691f69a95112edffa2b4c356948d6399f`，双方均为冻结a07 `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6`，不是当前候选执行文件接收。新新／混合十格及后续原Blob十格尚未执行，不能算该次CI通过。

该表现与此前保留的旧旧Relay失败类别相同，但尚无证据证明根因相同，也不能因双方是旧端就忽略原必需基线。此前完整14格和Blob10格通过的原始结果继续保留，不覆盖这次失败；AC11因此保持待复核，AC13也未完成。下一任务定位原工件与既有冻结host／驱动的拒绝边界，不替换旧SHA、不放宽断言、不把直接重跑成功称作修复。

原拒绝反馈环的本机结果：复用同一冻结a07源码的现有arm64 host（SHA-256 `aaae467b8bb6e3cb80ed5bfeed50d26e7623aa849871187e4dc92ce63eba0ddc`）及真实Relay，使用原user／mount／net隔离、`--old-pair-only --repeat 1`，由外层逐轮启动12个独立进程；每轮独立配对、profile重开和双向精确文件读回。12轮全部通过，入口各26.853–28.182秒，实际Relay路径、清理和明文检查逐份JSON核对。原预配对入口自身不循环repeat，不能用一次`--repeat 12`虚报12轮。本地arm64不等于CI x64；未重现原拒绝，也未证明根因或修复，不用这些通过覆盖原CI失败。

## 平台、真机与产品宿主状态矩阵

2026-10-27分别登记编译、受控主机执行、物理设备和产品采用，不把任一列外推到其他列。本表完成阶段6的状态登记任务，不完成阶段0／4物理证明，也不放宽AC08／AC13。

平台编译统一以中间候选#329 `d3e4527045ff26beef771c79a2332a2b7df35f4d`为已取得证据的范围：[PR run40462540479](https://github.com/UniClipboard/Engine/actions/runs/40462540479)原13个job均实际success；[Windows run40462544289](https://github.com/UniClipboard/Engine/actions/runs/40462544289)的原生arm64与x64均success。最终交付head仍待独立核对，本表不能代替AC13。

| 平台 | 中间候选编译 | 本片当前运行证据 | 真机切网 | 物理系统挂起／前后台 | 产品宿主采用 |
| --- | --- | --- | --- | --- | --- |
| Linux | 通过，workspace全部目标及原Linux CI | 通过，实际Iroh／SQLite／文件／双1GiB／独立进程及受控暂停恢复，详见原证据映射；不包含物理睡眠 | 跳过，隔离namespace不是物理设备切网 | 跳过，未执行物理挂起；受控elapsed不算实机 | 跳过，测试host不是Desktop产品 |
| macOS | 通过，原生仓库all-targets及严格diagnostic | 跳过，本片完整运行场景未在该平台执行；编译／诊断不代替运行矩阵 | 跳过 | 跳过 | 跳过 |
| Windows | 通过，原生arm64和x64 | 跳过，未运行该平台进度完整场景 | 跳过 | 跳过 | 跳过 |
| iOS | 通过，原绑定编译 | 跳过，未取得连接设备或模拟器运行证据 | 跳过 | 跳过 | 跳过 |
| Android | 通过，原绑定编译 | 跳过，未取得连接设备或模拟器运行证据 | 跳过 | 跳过 | 跳过 |
| HarmonyOS | 通过，真实SDK编译原绑定 | 跳过，未取得连接设备或模拟器运行证据 | 跳过 | 跳过 | 跳过 |

本机只读预检：adb、hdc、xcrun、PowerShell／pwsh均不可用；idevice_id存在但枚举退出255，设备数量未知。该结果不证明外部没有设备；当前也没有产品宿主仓库／运行连接信息。此前已请求设备及产品宿主信息，未收到答复；不重复索取或自行修改系统电源／设备配置。

物理证明仍使用[平台审计](#各平台经过时间审计与物理挂起入口)中的既有`node scripts/testing/run-progress-suspend-proof.mjs --physical`及平台验收宿主，操作方法和5／60／600秒证明边界只在原平台审计维护。仅编译、普通等待、SIGSTOP或可控时钟前进都不能填为物理通过；未执行行必须继续标跳过。获得环境后从本表缺口逐项补证，真实挂起及移动前后台必需项仍阻止整片归档。

## iOS与Android探针的完整进度动作准备

继续核对设备验收准备时发现，共享`uc-mobile-probe-core`没有出站查询／列表／核对命令，三个合法生产结果会落入`unsupported_development_result`。原网络host的进度入口不能代替移动宿主。这是验收入口缺口，不是Application或两个生产绑定缺少相应能力。

新增四个接缝测试在原实现全部失败：真实JSON命令反序列化拒绝，合法缺失视图和核对结果被误判为开发结果。修复仍只调用既有Engine完整Operation：`query_outbound_progress`接收`entry_id`与`target_device_id`，`list_outbound_progress`无参数，`reconcile_outbound_progress`另可带`expected_attempt_id`。命令不创建任务、下载、投影或恢复循环；未启动及既有Engine失败仍走原固定错误结果。返回摘要保留未知／部分未知、各项计数、freshness与七种核对结果，revision以十进制字符串避免跨JSON宿主丢精度；不输出原条目／设备／attempt身份、内容、文件名或路径。iOS证据白名单同步保留这些安全字段；Android既有广播直接返回共同Rust摘要，不另建一套映射。

使用既有`tests/hosts/ios/probe-command-device.sh`或`tests/hosts/android/probe-command.sh`传入上述JSON命令即可调用；源条目与目标身份取自原发送动作，不由探针推断。测试命令必须在已经启动并完成原会话准备的宿主上执行，不把`not_started`或核对完成当成文件完成。

本项只准备iOS／Android共同探针。HarmonyOS使用独立NAPI验收宿主，仍按独立入口核对；不把此补齐登记为三平台或真机通过。当前本机无Swift／Java编译器、Xcode及移动设备工具，原生App编译和真实前后台／系统挂起未执行，继续跳过并阻止AC08和归档。Linux完整共享探针30项、既有宿主边界13项，共43项实际通过，其中4项新增用例由原失败转为通过；逐条PASS与原输出核对。零doc test另行记录，不算执行案例；不以这些测试代替设备结果。

后续核对发现原移动CI只构建`uc-engine-uniffi`，不能验证共同探针的Apple／Android条件编译。原`mobile-bindings`作业现各增加探针原生库构建：iOS设备arm64及两种模拟器目标，Android沿原arm64与固定cargo-ndk；均复用原SDK、共享target、MBX和dev／locked配置，不新增工作流，也不改生产绑定步骤或原60分钟预算。该配置补齐只建立后续候选的实际编译门禁；本地主机没有对应SDK，原生库构建结果须以该候选线上步骤为准，尚未取得时保持未验证。它不构建Swift／Java App，不运行设备，也不替代物理挂起证明。
