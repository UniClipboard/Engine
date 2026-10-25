# Issue #34 第八切片：反向进度与发送侧状态投影执行计划

状态：**阶段0实机前置仍待完成；按用户指示推进独立规则与协议实现，生产业务迁移尚未开始**（2026-10-25）。
唯一需求、预算和验收正文见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)。
前置代码已合入 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`（PR #230）；开工前冻结实际 main、依赖和旧端来源。
本计划中的未勾选项均未验收，文档检查通过不代表阶段 0、行为、平台或交付通过。

## 完整负责人、调用及恢复责任

- Application `transfer/progress` 拥有完整反向通知、发送侧投影、终态采用与 ACK、有限补发、失联显示、状态核对及其生命周期。
- 原 `transfer/receive` 继续独占实际接收、重试、输出、提交、取消及本地清理；progress 不产生第二套业务结算权威。
- 宿主继续提交原发送／接收／重试动作，查询完整进度视图；再次发起所需核对在完整动作内部，不让宿主编排核对、准入和下载步骤。
- 成功分别按实际本地结果、发送侧采用、核对已证实事实表达。未确认、明确失败、明确拒绝、资料缺失、旧版不支持不能混为同一返回。
- 网络补发只由 progress owner 按 spec §5.5 执行；存储／输出结算修复仍由原 owner 执行。本片队列、ACK 等待及关联不跨进程重启恢复。
- Engine 只装配完整能力、提供稳定契约并装饰既有完整 port；Infra 执行编解码、流和存储能力，两套绑定只映射同版本 Engine。

## 开工与证据规则

每阶段完成一个可独立验证的纵向结果。先列失败方式与基线，再改变对应生产行为；原行为已满足时记录真实基线，不制造假红测。
会话记录、命令原始输出、JUnit、资源采样、OTLP、截图和源码快照留在本地忽略目录或 CI Artifacts，正式文档只保留结论、验证入口和必要取舍。
每轮证据关联完整 source SHA／tree、lockfile、旧端 SHA、二进制指纹、测试身份与首次失败，不拿上一候选绿测替代交付 head。
禁止为获得绿色结果放宽第七片预算、文件内容断言、授权、资源阈值或必需兼容范围。

## 阶段 0：冻结基线，证明接缝与协议可行

目标：在生产迁移前解决 spec §11 四项技术证明，对应 AC01／AC02／AC05／AC08 的前置。

* [x] 核对最近 AGENTS、架构、协议、错误、安全及业务观测标准；确认源码工作区没有切片 7 撤销差异，冻结实际开工来源。
* [x] 盘点旧 reporter、通知邮箱、Engine 投影、Router、生命周期、逐项查询、绑定和网络 harness 的全部消费者。
* [x] 冻结旧 `/1` 全部状态及总量形式的18条编解码向量，以及开工前 main／既有受支持历史锚点；两种旧端各自旧旧 Direct／强制 Relay 四格实际资料重开与文件读回通过。精确 SHA、产物来源及复跑入口见[旧端基线](2026-10-24-transfer-progress-connectivity-feasibility.md#固定旧端旧旧文件与旧进度向量)，不以移动分支代替证据。
* [x] 定义新 `/2` 登记／更新／终态 ACK／核对的精确字节、限制、分页及 golden 向量；保留旧 `/1` 字节和 blob／V3 格式。见[线上合同](../../design-docs/transfer-progress-protocol.md)；仅 codec，不据此勾选真实登记或 ACK 采用证明。
* [x] 用真实双端及可控消息延迟证明源条目→接收逻辑项／attempt 的关联，新旧 producer／登记乱序不能回退；连接重建不重置修订，进程重启退休旧关联。普通文件／空目录、挑战前后替换、迟到 Bound、真实接收 fixture kill／relaunch 与新 QUIC 连接组合通过；受控存储 port 的边界见[组合证明](2026-10-24-transfer-progress-connectivity-feasibility.md#实际当前权威与真实登记的组合)，不冒充后续 Engine／SQLite 或设备矩阵。
* [x] 用真实 Uni 双向流完成一个输出后的终态采用与 ACK，证明 FIN 仅为传输事实，不是 Application 采用。实际 BAO 接收／文件导出、采用前暂停、精确重复／冲突、取消／期限到达及尾随字节证据见[输出后采用证明](2026-10-24-transfer-progress-connectivity-feasibility.md#真实输出后的-application-采用与-ack)；生产接收 attempt／SQLite 交接及完整阶段 3 尚未完成。
* [x] 用真实旧 `/1` 验证最小受管单向流接缝，两个交换共用连接，停止一项不打断另一项；记录活动许可、当前准入及关闭寿命。主机证明范围见[接缝记录](2026-10-24-transfer-progress-connectivity-feasibility.md)，生产 Router 迁移不据此记完成。
* [ ] 核对各目标平台经过时间能力，证明系统挂起消耗期限；逻辑模拟、主机睡眠与物理设备分别登记，不把未测平台记通过。源码审计、无socket唤醒期限回归及人工物理挂起入口见[平台审计](2026-10-24-transfer-progress-connectivity-feasibility.md#各平台经过时间审计与物理挂起入口)；进度空闲预算与原子复用已取得[主机证明](2026-10-24-transfer-progress-connectivity-feasibility.md#进度连接空闲期限证明)，物理测试仍未闭合，保持未完成。
* [x] 以最大原始清单、分页、终态超额及一个卡住的对端验证 spec §5.6 起始技术策略，冻结最终策略和调度算法。能力及证据边界见[资源覆盖表](2026-10-24-transfer-progress-connectivity-feasibility.md#阶段0资源策略覆盖与冻结)；最大CAS清单强制通过真实专属profile读取16页，主机连接保留含真实31秒工作／30秒回收。平台挂起、生产迁移和阶段5实机矩阵不据此记完成。
* [x] 输出阶段 0 结论，逐项为已证明／阻塞／未执行；见[阶段0结论](2026-10-24-transfer-progress-connectivity-feasibility.md#阶段-0-结论)。经过时间实机前置仍缺证据，阶段0整体保持未完成；结论交付不解锁生产迁移。当前没有要求更改blob／V3、扩大持久化或弱化登记隔离。

交付物：固定协议合同、最小原型证据、完整 owner／port 边界与阶段 0 结论。用户于2026-10-25明确要求不等待、继续下一任务，允许先推进可独立验证的阶段1纯规则与既有原型快照；阶段0仍按实际证据保留未完成，生产迁移继续保留前置门槛。

## 阶段 1：规则、投影及有界调度

目标：独立验证状态与时间算法，对应 AC02／AC04／AC05／AC06。

* [x] 在 Core 定义纯状态转换和逐项／目录汇总；复用既有单项与联系规则，新增批次汇总并接入Application同锁完整快照。6项汇总规则与完整快照采用／失联／真实结算测试通过，Core＋Application完整1812项通过；保留待完成的生产迁移及实机验收。
* [ ] 建立 Application progress 模块入口、模型、专属 port、policy、projection、delivery 和测试，避免堆入 mod.rs 或按公共技术目录拆散责任。
* [ ] 实现设备／源条目／逻辑项／attempt／round／sequence／producer 修订及精确结果 ACK 校验，登记和更新使用同一权威。
* [ ] 建立从完整发送意图开始的等待投影，不依赖首条进度；完整快照可以从事件丢失恢复。
* [ ] 用可注入经过时间实现 spec §5.5 串行补发，覆盖前次等待跨第 2 秒、恢复提前资格、多个错过时点、恰好过期、次数耗尽及重复网络机会。
* [ ] 实现逐任务最新中间值、终态优先、跨对端公平与独立终态保留；验证所有全局／每端额度及缓存淘汰策略。
* [ ] 实现低频 Contact 与失联显示；重复消息、旧轮次、ACK、本机拨号不刷新有效联系，Contact 不刷新下载有效数据预算。
* [x] 错误分类保留 source；登记前／挑战后／终态采用的具体拒绝通过既有Rejected回传并核对本轮关联。畸形帧保留WireError来源并停止补发，真实I/O沿用原预算；不将授权／存储错误变为离线或资料缺失。生产查询入站与Router迁移仍单独验收。

交付物：纯规则、实际调度与可控时间测试；具体数值只有 spec 一处事实来源。

## 阶段 2：新旧协议与受管连接完整收发

目标：可在真实网络交换完整消息，对应 AC01／AC04／AC06。

* [ ] uc-sync-protocol 实现阶段 0 冻结的 `/2` codec、固定向量和长度／截断／额外字节拒绝；协议层不承担拨号、准入、重试或业务采用。
* [ ] 扩展 ConnectionProtocol／ConnectivityProfile、入站 Router、manager／租约和最小单向流能力，旧版与新版均受管。
* [ ] 新版双向交换的登记响应、终态 ACK 及查询结果来自 Application owner；adapter 不自行 ACK 广播入队成功。
* [ ] 只有真实能力不支持新版才使用旧协议；网络超时、错误来源、损坏帧、拒绝准入不触发降级。
* [ ] 入站与出站逐次核对当前成员及会话；旧连接、撤销、晚到拨号或本地读取不能发送到新世代。
* [ ] 单项停止只结束本流；真实本地工作未结束时继续持有许可，空闲回收不误关活动交换。
* [ ] 删除旧 reporter 自建原生连接缓存与独立业务发送循环，保留旧 codec 和兼容适配；不以 feature 留生产旁路。
* [ ] 第二 backend 验证业务边界可替换；真实 Iroh 分别证明单向／双向流、两流隔离和实际授权。

交付物：唯一受管协议接线及两版本真实收发。第二 backend 不作为 Direct／Relay 事实证据。

## 阶段 3：接收交接、逐项成果与轮次

目标：真实业务结果驱动反向状态，对应 AC02／AC03／AC04／AC06。

* [ ] 从原接收 owner 交接不可变原始计划、当前 attempt 与必要语义，消息源设备和 blob provider 保持独立。
* [ ] 跑通真实文件接收→校验／解密适用路径→所需输出／提交→终态通知→发送侧采用→ACK 的最小完整动作。
* [ ] 受控暂停校验、输出、提交和清理，验证 Processing 及真实失败，无提前 Completed，无通知等待进入本地结算链。
* [ ] 逐项终态和批次汇总覆盖成功、失败、取消、未确认及尚在运行；目录按完整单元，未知不计失败，末项成功不覆盖前项失败。
* [ ] 手动重试只更新选中项并保留成功／未选结果；旧 attempt 的登记、状态、终态和 ACK 均不能污染新尝试。
* [ ] 传递真实自动重试轮次；百分比允许本轮真实回落，保持原高水位、停滞期限和恢复预算不变，重复下载字节不冒充新进展。
* [ ] 注入终态／ACK 丢失、重复、冲突及乱序，验证 Application 采用和事件发布幂等、未知可收敛而真实终态不回退。
* [ ] 通知永久阻塞、终态容量不足及对端离线时，实际下载、取消、输出、既有成功项与原资源监督不受影响。

交付物：真实 Engine／SQLite／文件输出的完整行为证据，旧邮箱发送责任由新 owner 统一替代。

## 阶段 4：完整状态核对、稳定契约及生命周期

目标：宿主只读取完整业务事实并提交原动作，对应 AC07／AC08／AC09。

* [ ] 完整快照和状态核对由 Application facade 提供；查询关联绑定认证设备及当前 Space，不暴露任意本地 entry 枚举。
* [ ] 实现 Running／Settled／Superseded／NotRetained／Unsupported／Unreachable，区分 AccessDenied、存储读取失败和损坏。
* [ ] 将再次发起的核对接入原完整动作；运行中继续原任务、真实结果返回、缺失或旧版说明后显式新尝试、离线不重复下载。
* [ ] 查询快照后重试／删除竞争仍由原 owner 最终 CAS 和清理围栏决定；不新增远端强制下载或取消操作。
* [ ] 发送侧事件和完整快照可区分设备、尝试及逻辑项；Rust、UniFFI、N-API 同版本映射，不让宿主汇总或推断协议阶段。
* [ ] 删除 Engine OutboundProgressRuntime 中的业务投影／取消推断，保留必要的组装及完整能力 decorator。
* [ ] 暂停／恢复／后台／系统挂起按原期限检查，普通重连不重置预算；离组／撤销／切 Space 先封口再监督停止。
* [ ] 删除清理临时投影，迟到帧不复活条目或进度卡；原文件资源清理和取消语义保持。
* [ ] 同资料关闭重开及真实进程重启后，无持久依据的发送侧旧任务显示未知，不恢复通知队列、不伪造旧终态；本地接收已有事实保持。
* [ ] 全部内部调用方迁移，删除无意义旧转发和测试默认空依赖；不新增 schema、通知 outbox 或敏感明文字段。

交付物：完整产品查询／动作与绑定合同、唯一 Application owner 和受控生命周期。

## 阶段 5：真实网络、资源及业务观测验收

目标：不借用单元测试证明网络或 UI 事实，对应 AC10／AC11／AC12。

* [ ] 扩展现有 `tests/hosts/connectivity` 和网络 harness，新新与两种固定旧端 × 新端两角色 × Direct／强制 Relay，保留旧旧基线及全部必需结果。
* [ ] 网络矩阵精确读回实际文件、核对真实 selected path／ALPN；旧端验证文件互通和新版保守投影，不强求旧端实现 ACK／查询。
* [ ] 真实三端源设备／provider 分离，以及同条目向双设备并发传输、旧登记／尝试／轮次乱序均通过。
* [ ] 真实阻塞一端读流／ACK 时，另一端仍前进且取消独立；队满、缓存淘汰和超大批次未知项结果准确。
* [ ] 至少回归原 1GiB 负载，观察通知任务、连接、内存、FD、许可及清理前后；连续通知／查询／重连无持续资源增长。
* [ ] 回归原文件部分失败、成功项保留、目录发布／回滚、单项重试、授权／删除及重启恢复，保持原阈值与下载预算。
* [ ] 观测沿用原完整接收／手动重试动作，用户核对有完整触发和结果；heartbeat、ACK 和补发不各建业务根，不延长已结束动作计时。
* [ ] 真实 Collector／Jaeger 验证正常、部分失败、取消、未知和核对的列表／详情、原始输出与固定分类；敏感哨兵及测试／产品隔离通过。

交付物：固定源码和二进制的可复跑网络／资源／观测证据，首次失败与修复证据均保留。

## 阶段 6：回归、当前候选 CI 与文档交付

目标：完成 AC01–AC13 的逐项审计，再归档。

* [ ] 当前候选定向协议、Core、Application、Infra、Engine、SQLite 和绑定检查全部通过。
* [ ] workspace、成员全集、升级 smoke、前六片与第七片必需网络及资源回归通过；不以隔离重跑成功覆盖原全量失败。
* [ ] 本地根门禁全部通过，源码／依赖未变化后不无理由重复构建；单负责人、共享 target、保留编译缓存。
* [ ] 将本片必需网络／资源用例接入原 CI；对同一交付 head 核对 Linux／macOS／Windows、iOS／Android／HarmonyOS 必需编译与线上检查。
* [ ] 真机切网、睡眠／前后台和 Desktop／移动产品宿主采用分别标明通过／失败／跳过，主机／模拟器结果不得冒充真机。
* [ ] 按 spec AC01–AC13 核对唯一证据映射和全部跳过项，存在任何必需未执行项不能登记整片完成。
* [ ] 稳定结论回写所属 Design Doc／必要 ADR，更新入口；实际完成后将 spec／plan 一起移入 completed，不关闭整个 #34。

## 验证入口

所有 Rust 命令从仓库根目录执行，复用原共享 target 与 MBX 缓存，由一个负责人串行验证。
新增具体选择器和网络入口在阶段 0 冻结后登记到现有 harness；当前未实现的测试不能冒充已有命令。

根门禁：

```bash
just cargo metadata --locked --format-version 1
just cargo check --workspace --all-targets --locked
just cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

沿用的回归入口：

```bash
just mbx-exec bash scripts/testing/run-test-group.sh workspace --no-fail-fast
just mbx-exec bash scripts/testing/run-test-group.sh membership-e2e --no-fail-fast
just mbx-exec bash scripts/testing/run-test-group.sh upgrade-matrix --smoke --no-fail-fast
bash scripts/testing/run-blob-connectivity.sh
bash scripts/testing/run-blob-lifecycle-resources.sh
bash scripts/testing/run-peer-reachability-connectivity.sh
```

网络矩阵需要 Linux 隔离网络权限；使用现有网络和 relay host，不另建只用合成地址的替代框架。
文档交付单独核对相对链接、索引、11 节结构、行为覆盖和 diff 格式；不等于上述行为回归已运行。

## 当前状态与下一个动作

- 当前已完成受管单向流最小接缝及主机证明、新版有界字节合同与向量；旧 reporter／生产 Router 尚未迁移，新版登记围栏已取得真实双端原型证据；输出后 Application 采用／ACK 的最小主机链路已完成；接收 CAS 后权威交接及 Application 进程寿命已验证；当前权威与真实协议组合已完成，完整单轮终态通知与精确 ACK 已接入同一原预算；有界终态监督已接纯补发／账本／交换规则并取得实际多端阻塞隔离证据，仍未接生产 Router／结算 hook。单轮完整快照读取已具备同一原预算内的分页与覆盖核验能力，实际最大CAS清单／网络证明由强制入口执行；普通Update已具备同一监督器内的最新值合并、按任务5Hz及活动／终态采用；Contact已与普通Update共用监督器，并提供原发送意图起算的60秒联系视图；生产状态核对及其他阶段 0 项尚未完成。查询round已与终态监督共享实际4／1和等待64／16额度，仍无生产Query入站及结果采用接线。全局发送投影注册表已有原意图等待、256/64×1024上限、退休和变化提示能力；生产原发送hook仍待迁移。专属progress profile已提供单次原生拨号、双版本协商、1连接/方向和16保留/30秒策略及真实主机证明，生产Router/reporter接线与平台挂起矩阵仍待后续完成。
- 2026-10-24 文档交付：相对链接、结构与决定覆盖检查，以及 metadata、workspace 全目标 check、fmt、Rust style、repository 和 diff 根门禁通过；不据此勾选上述实施验收。
- 产品行为与默认预算已确认；技术前置状态按 spec §11 和阶段 0 记录维护，不把主机证明泛化为完整生产或平台矩阵。
- 下一个实施动作：继续完整查询入站及Application实际状态读取，不等待前层CI。密码兼容修复已完成固定旧端、资料兼容与审计验证，见[依赖记录](../../design-docs/opaque-dependency-compatibility.md)；CI缓存fixture隔离已另行修复。阶段0实机挂起证据仍待提供，不记为通过；完整生产迁移与设备矩阵继续单独验收。
