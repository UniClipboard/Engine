# 第八片阶段 0：受管流、线上合同与登记围栏证明

状态：**阶段 0 结论已形成，经过时间实机证明仍有缺口，整体尚未完成**（2026-10-25）。
需求见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)，推进见[执行计划](2026-10-24-transfer-progress-connectivity-plan.md)。
开工基线为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；本记录描述该基线上的候选变更，
各节分别说明主机探针与固定旧端独立进程基线的证据，不将它们当作完整生产迁移或真机验收。

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
