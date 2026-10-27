# Issue #34 第八切片：反向进度与发送侧状态投影执行计划

状态：**生产实现与主机验证已完成；用户2026-10-27明确跳过物理挂起／设备后台验证，继续最终候选CI及文档交付，逐任务创建未合并stack PR**。
唯一需求、预算和验收正文见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)。
前置代码已合入 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`（PR #230）；开工前冻结实际 main、依赖和旧端来源。
复选框表示任务已处理；按用户要求跳过的验证必须明示“跳过”，不计为通过。当前跳过范围及交付影响见[用户跳过决定](2026-10-24-transfer-progress-connectivity-feasibility.md#用户批准跳过物理挂起与设备后台验证)。文档检查通过不代表行为、平台或交付通过。

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
* [x] 核对各目标平台经过时间能力，证明系统挂起消耗期限；逻辑模拟、主机睡眠与物理设备分别登记，不把未测平台记通过。源码审计、无socket唤醒期限回归及人工物理挂起入口见[平台审计](2026-10-24-transfer-progress-connectivity-feasibility.md#各平台经过时间审计与物理挂起入口)；进度空闲预算与原子复用已取得[主机证明](2026-10-24-transfer-progress-connectivity-feasibility.md#进度连接空闲期限证明)，物理测试按用户2026-10-27要求跳过，不计通过；本任务按主机证明与明确跳过结论完成处置，不再作为后续交付阻塞。
* [x] 以最大原始清单、分页、终态超额及一个卡住的对端验证 spec §5.6 起始技术策略，冻结最终策略和调度算法。能力及证据边界见[资源覆盖表](2026-10-24-transfer-progress-connectivity-feasibility.md#阶段0资源策略覆盖与冻结)；最大CAS清单强制通过真实专属profile读取16页，主机连接保留含真实31秒工作／30秒回收。平台挂起、生产迁移和阶段5实机矩阵不据此记完成。
* [x] 输出阶段 0 结论，逐项为已证明／阻塞／未执行；见[阶段0结论](2026-10-24-transfer-progress-connectivity-feasibility.md#阶段-0-结论)。经过时间实机证据仍缺，按用户2026-10-27决定登记跳过；阶段0按已证明范围及明确跳过结论完成处置，不代表实机验收通过。当前没有要求更改blob／V3、扩大持久化或弱化登记隔离。

交付物：固定协议合同、最小原型证据、完整 owner／port 边界与阶段 0 结论。用户于2026-10-25要求先推进独立阶段1规则；2026-10-26再次明确“不需要等待直接开始下一个任务”，允许按未合并stack PR继续后续生产实现。用户2026-10-27进一步明确跳过物理挂起／设备后台验证并继续后续任务；该部分不再阻塞交付，仍不算实机通过。其余既定网络／资源／兼容矩阵及最终候选CI继续验收；未授权自动合并或发布。

## 阶段 1：规则、投影及有界调度

目标：独立验证状态与时间算法，对应 AC02／AC04／AC05／AC06。

* [x] 在 Core 定义纯状态转换和逐项／目录汇总；复用既有单项与联系规则，新增批次汇总并接入Application同锁完整快照。6项汇总规则与完整快照采用／失联／真实结算测试通过，Core＋Application完整1812项通过；保留待完成的生产迁移及实机验收。
* [x] 建立 Application progress 模块入口、模型、专属 port、policy、projection、delivery 和测试，避免堆入 mod.rs 或按公共技术目录拆散责任。已逐文件核对定义、完整动作、单轮／单页能力、原接收事实与统一调度的所有权；纯策略在Core，Application只装配专属连接策略，不另造重复policy。导航与调用边界见[模块所有权](../../design-docs/transfer-progress-protocol.md#模块所有权与调用边界)；生产接线不据此记完成。
* [x] 实现设备／源条目／逻辑项／attempt／round／sequence／producer 修订及精确结果 ACK 校验，登记和更新使用同一权威。逐字段规则与Application拒绝／无副作用、专属profile真实Iroh的21个回执字段／边界及14种完整结果、原实际接收权威8项共同通过；完整Core／Application／P2P2164项通过。[身份与精确ACK证据](2026-10-24-transfer-progress-connectivity-feasibility.md#阶段1身份与精确ack校验)只完成本阶段规则和主机原型，不提前完成生产接线。
* [x] 建立从完整发送意图开始的等待投影，不依赖首条进度；原意图等待／过期与已登记、首次未登记的完整快照恢复均已实现。真实双端1024项／16页核对覆盖两种情况，查询不授予通知binding，后续同范围登记保留事实；生产发送hook仍按阶段3迁移。
* [x] 用可注入经过时间实现 spec §5.5 串行补发，覆盖前次等待跨第 2 秒、恢复提前资格、多个错过时点、恰好过期、次数耗尽及重复网络机会。Core纯规则与原Application worker均已验证；组合用例让取消后的能力继续等待实际收尾，确认原许可和生成时刻保留、恢复机会合并、跳过时点不突发补齐，以及保留期限精确到达后停止。本项为逻辑时间与worker证明，不替代阶段0物理挂起或阶段3生产hook迁移。
* [x] 实现逐任务最新中间值、终态优先、跨对端公平与独立终态保留；验证所有全局／每端额度及缓存淘汰策略。Core公平队列／终态保留、Application最大清单／实际额度／旧值隔离与真实多端阻塞证明通过；同一监督器双缓存饱和、全部中间值淘汰及重复合并不改变终态结果或生成时刻，实际四端优先终态，关闭仍等真实工作结束。生产接线和阶段5完整资源矩阵继续单独验收。
* [x] 实现低频 Contact 与失联显示；重复消息、旧轮次、ACK、本机拨号不刷新有效联系，Contact 不刷新下载有效数据预算。Core、原worker与真实Iroh联系／重复序号证明已通过；完整FetchBlobUseCase在持续有效Contact保持Live时仍按原首数据期限停止并监督实际身份读取，原ReceiveTiming已验证数据后也不因Contact或重复字节报告延长停滞期限。生产hook和设备矩阵仍在后续阶段。
* [x] 错误分类保留 source；登记前／挑战后／终态采用的具体拒绝通过既有Rejected回传并核对本轮关联。畸形帧保留WireError来源并停止补发，真实I/O沿用原预算；不将授权／存储错误变为离线或资料缺失。生产查询入站与Router迁移仍单独验收。

完整快照核对与原子采用由Application统一负责，覆盖首次未登记发现，详见[协议设计](../../design-docs/transfer-progress-protocol.md#完整快照的原子采用)。稳定宿主查询与完整生产接线仍在后续阶段，不据此提前勾选这些验收。

交付物：纯规则、实际调度与可控时间测试；具体数值只有 spec 一处事实来源。

## 阶段 2：新旧协议与受管连接完整收发

目标：可在真实网络交换完整消息，对应 AC01／AC04／AC06。

* [x] uc-sync-protocol 实现阶段 0 冻结的 `/2` codec、固定向量和长度／截断／额外字节拒绝；协议层不承担拨号、准入、重试或业务采用。完整协议crate tests的76项全部通过，包含旧/1全部18状态／总量向量、新/2全部36基础及带载体固定字节和拒收边界；与阶段0最终来源核对11个wire／依赖／fixture文件无差异，逐源码确认分配前校验长度及错误来源保留。不据此完成真实Router或新旧网络矩阵。
* [x] 扩展 ConnectionProtocol／ConnectivityProfile、入站 Router、manager／租约和最小单向流能力，旧版与新版均受管。 已逐当前生产源码核对同一Application对象图、manager／单向及双向work lease、双ALPN唯一handler、Engine端点安装和实际生命周期关闭；六项真实入站、profile复用及Router偏好共八项原PASS／JUnit和十四格网络证据一致。详见[生产受管接线核对](2026-10-24-transfer-progress-connectivity-feasibility.md#当前生产受管进度接线核对)，独立降级／授权父项及物理／最终CI保持各自状态。
* [x] 新版双向交换的登记响应、终态 ACK 及查询结果来自 Application owner；adapter 不自行 ACK 广播入队成功。查询已由原接收目录／保存事实提供，7项强制真实连接证明包含实际失败清理后1024项／16页Query；生产Router和宿主结果采用仍按后续任务迁移。
* [x] 只有真实能力不支持新版才使用旧协议；网络超时、错误来源、损坏帧、拒绝准入不触发降级。 已核对唯一握手的真实协议选择、错误原类别、完整旧批次汇总、FIN非业务ACK与旧入站保守采用；原21项旧语义PASS／JUnit、显式登记组合8项及十四格生产矩阵一致，默认ignore没有冒充普通workspace执行，见[版本选择与旧语义核对](2026-10-24-transfer-progress-connectivity-feasibility.md#生产版本选择与旧协议保守语义核对)。
* [x] 入站与出站逐次核对当前成员及会话；旧连接、撤销、晚到拨号或本地读取不能发送到新世代。 已核对原request／session／epoch、grant当前成员、每段wire授权与晚拨结果发布围栏；原22项PASS／JUnit及实际权威子进程／Iroh组合证明旧读取、旧Query与旧binding不能借新资格，见[授权与原会话核对](2026-10-24-transfer-progress-connectivity-feasibility.md#当前授权与原会话围栏核对)。第二backend与真实Iroh／Engine证据分层，物理与最终CI另行验收。
* [x] 单项停止只结束本流；真实本地工作未结束时继续持有许可，空闲回收不误关活动交换。真实旧/1两条流中停止一项后另一项按原字节读回，实际work未释放时下一项准入仍等待；新/2真实输出后的Application采用跨期限仍被监督。实际profile仅持work的31秒内连接不关闭，释放work后原空闲期限回收；Application另验证本机work／实际关闭两种顺序及晚到拨号收尾。生产Router与物理挂起仍单独验收。
* [x] 删除旧 reporter 自建原生连接缓存与独立业务发送循环，保留旧 codec 和兼容适配；不以 feature 留生产旁路。 原Native adapter、Core reporter port、Application旧邮箱和Engine译送循环均已删除；生产节点只安装Application双版本完整端点，旧34字节codec保留。workspace全部targets及真实成员撤销4项通过。
* [x] 第二 backend 验证业务边界可替换；真实 Iroh 分别证明单向／双向流、两流隔离和实际授权。独立内存字节流backend使用原manager／profile、真实/2 codec与Application完整流程，完成登记、Contact／Update、精确终态ACK、65项两页核对采用；授权撤销不能复用旧连接或改变投影，停止单流后同连接下一交换仍可用。固定测试身份不冒充密码握手，内存通道不作为Direct／Relay证据；实际Iroh与设备矩阵按各自证据维护。

交付物：唯一受管协议接线及两版本真实收发。第二 backend 不作为 Direct／Relay 事实证据。

## 阶段 3：接收交接、逐项成果与轮次

目标：真实业务结果驱动反向状态，对应 AC02／AC03／AC04／AC06。

* [x] 从原接收 owner 交接不可变原始计划、当前 attempt 与必要语义，消息源设备和 blob provider 保持独立。 原BeginReceive结果为Begun后才交接，准备计划不授予通知资格；有/无源提示×接受/拒绝四组合核对原ticket顺序、实际attempt、本机条目及消息源/provider查询隔离，接收目录21项覆盖计划跨attempt及旧动作围栏。受控存储/材料化port的范围见[交接验收](2026-10-24-transfer-progress-connectivity-feasibility.md#原接收owner交接验收)，不作为实际三端网络或SQLite成果链的替代。
* [x] 跑通真实文件接收→校验／解密适用路径→所需输出／提交→终态通知→发送侧采用→ACK 的最小完整动作。真实双Engine、SQLite、文件读回及精确ACK验证输出屏障前后的逐项结果；证据和范围见[成果链验证](2026-10-24-transfer-progress-connectivity-feasibility.md#真实文件成果与精确ack链路)。
* [x] 受控暂停校验、输出、提交和清理，验证 Processing 及真实失败，无提前 Completed，无通知等待进入本地结算链。 真实AEAD前、输出返回、SQLite终态提交及失败输出清理均由原能力形成屏障，完整源端核对Processing且无提前ACK；离线通知永久等待时本地重试仍提交。证据及范围见[结算边界](2026-10-24-transfer-progress-connectivity-feasibility.md#真实本地结算边界与通知隔离)。
* [x] 逐项终态和批次汇总覆盖成功、失败、取消、未确认及尚在运行；目录按完整单元，未知不计失败，末项成功不覆盖前项失败。 原双Engine、SQLite、实际文件和精确ACK验证首／中／末失败、重试运行／取消、锁定拒绝与退休后未确认，以及目录成功／整体回滚／纯空根；见[批次汇总验证](2026-10-24-transfer-progress-connectivity-feasibility.md#逐项结果与批次汇总的主机验证)。
* [x] 手动重试只更新选中项并保留成功／未选结果；旧 attempt 的登记、状态、终态和 ACK 均不能污染新尝试。 双Engine／SQLite／文件验证原选中失败项实际重试、源端新attempt完整采用及原成功路径保持；专属原生连接验证旧挑战描述、状态／终态拒绝、精确ACK返回后的当前权威复核，未选取消及接收交接由原完整负责人测试覆盖。证据和范围见[重试围栏验证](2026-10-24-transfer-progress-connectivity-feasibility.md#手动重试与旧尝试围栏的主机验证)。
* [x] 传递真实自动重试轮次；百分比允许本轮真实回落，保持原高水位、停滞期限和恢复预算不变，重复下载字节不冒充新进展。 原下载负责人真实恢复分支交接轮次/Retrying，SDK重放20不超过原80高水位时仍按原300秒停滞到期；表示按实际压缩blob大小保留到保存结算。188项传输测试通过，fixture网络能力不冒充实际Iroh故障恢复。
* [x] 注入终态／ACK 丢失、重复、冲突及乱序，验证 Application 采用和事件发布幂等、未知可收敛而真实终态不回退。 原双Engine按实际60秒期限从2成功1未知完整核对为3成功，不增加获取／导出；真实专属连接验证同binding丢ACK、重复精确ACK、五类冲突和迟到活动，以及无订阅者时事实仍保持。见[终态交付验证](2026-10-24-transfer-progress-connectivity-feasibility.md#终态丢失后的核对与同绑定重放验证)。
* [x] 通知永久阻塞、终态容量不足及对端离线时，实际下载、取消、输出、既有成功项与原资源监督不受影响。 真实67项冷缓存接收在每端64项容量溢出时完成65项保存并取消／清理余项，最后64项原生ACK及完整核对证明真实淘汰与补齐；原离线阻塞保存／取消及完整接收回归保持。固定资源入口保留原8项并加入容量场景，共9项，严格7场景／8报告及原资源断言通过。见[通知隔离验证](2026-10-24-transfer-progress-connectivity-feasibility.md#通知容量与本地接收隔离验证)。

交付物：真实 Engine／SQLite／文件输出的完整行为证据，旧邮箱发送责任由新 owner 统一替代。

## 阶段 4：完整状态核对、稳定契约及生命周期

目标：宿主只读取完整业务事实并提交原动作，对应 AC07／AC08／AC09。

* [x] 完整快照和状态核对由 Application facade 提供；查询关联绑定认证设备及当前 Space，不暴露任意本地 entry 枚举。 AppFacade仅提供完整get/list/reconcile，使用原意图目录与当前成员范围；真实双Engine/SQLite/文件证明读取源端原条目和接收尝试，来源与本机接收条目不同，核对不增加导出。
* [x] 实现 Running／Settled／Superseded／NotRetained／Unsupported／Unreachable，区分 AccessDenied、存储读取失败和损坏。 统一App分类保持完整source chain；明确授权拒绝不冒充Unavailable/离线，expected_attempt已被替换返回Superseded。规则/查询全覆盖及真实Settled/Superseded核对通过，Busy按实际调度返回。
* [x] 将再次发起的核对接入原完整动作；运行中继续原任务、真实结果返回、缺失或旧版说明后显式新尝试、离线不重复下载。 原ResendEntry统一核对及明确确认，逐目标返回完整事实与重复风险；消息头Delivered不跳过文件核对。App完整1387通过/2跳过，真实Engine运行/结算/明确失败项重投及离线无重复下载通过，UniFFI47、NAPI21、移动验收宿主26项通过；完整固定旧端/Direct/Relay及设备矩阵仍由阶段5验收。
* [x] 查询快照后重试／删除竞争仍由原 owner 最终 CAS 和清理围栏决定；不新增远端强制下载或取消操作。 真实双Engine在已取旧页后交错本机重试、接收端删除及源端删除；实际显式重发保持新attempt／四次获取导出，迟到快照不授旧重试权限，源删除将旧核对退休为Superseded且远端文件保持。App最终检查及SQLite事务CAS分层验证通过，见[快照竞争验证](2026-10-24-transfer-progress-connectivity-feasibility.md#查询快照与最终准入竞争验证)。
* [x] 发送侧事件和完整快照可区分设备、尝试及逻辑项；Rust、UniFFI、N-API 同版本映射，不让宿主汇总或推断协议阶段。 三套稳定入口已接同一完整App能力；OutboundProgressChanged只提示重读，真实双Engine事件/完整视图通过。两绑定all-targets编译通过；NAPI大整数/未知总量映射测试通过。移动设备实机仍未执行。
* [x] 删除 Engine OutboundProgressRuntime 中的业务投影／取消推断，保留必要的组装及完整能力 decorator。 Engine旧模块和空SyncSessionAssembly已删除；投影、核对、通知和实际关闭统一归Application，Engine只提供网络能力及稳定DTO映射。真实本地取消/关闭不被阻塞进度网络拖住。
* [x] 暂停／恢复／后台／系统挂起按原期限检查，普通重连不重置预算；离组／撤销／切 Space 先封口再监督停止。主机规则、暂停恢复及会话退休证据见[AC08映射](2026-10-24-transfer-progress-connectivity-feasibility.md#ac01ac13唯一证据映射)；物理挂起及设备后台部分按用户要求跳过，不计通过，任务按此范围完成处置。
* [x] 删除清理临时投影，迟到帧不复活条目或进度卡；原文件资源清理和取消语义保持。 真实源删除红→绿覆盖已结算及输出暂停/迟到终态两种Engine/SQLite/文件场景；原发送在首次异步等待前冻结本机owner世代，History完整删除统一封口旧/在途意图，源UUID与本机owner分离。另一本机条目不受影响，接收端文件保持；Application1381通过/2跳过、真实进度stream82通过/8跳过，另强制原权威8项通过。成员/会话及再次发起仍按未完成项实施。
* [x] 同资料关闭重开及真实进程重启后，无持久依据的发送侧旧任务显示未知，不恢复通知队列、不伪造旧终态；本地接收已有事实保持。 Application只读已有本机条目／目标归属与完整清单，三文件重开及独立sender进程均无attempt／字节恢复；receiver在实际保存但终态未ACK时被kill，同profile新进程保留三份精确文件，原Source一次真实核对NotRetained并保持三项未确认。读取沿用原删除／成员围栏，无新schema/outbox，见[重启验证](2026-10-24-transfer-progress-connectivity-feasibility.md#同资料重开与真实进程重启验证)。
* [x] 全部内部调用方迁移，删除无意义旧转发和测试默认空依赖；不新增 schema、通知 outbox 或敏感明文字段。 旧reporter所有生产/测试消费者及反向路由字段已删除，新端点能力显式装配；workspace全部targets、Root门禁通过。没有schema/outbox或新明文持久字段。

交付物：完整产品查询／动作与绑定合同、唯一 Application owner 和受控生命周期。

## 阶段 5：真实网络、资源及业务观测验收

目标：不借用单元测试证明网络或 UI 事实，对应 AC10／AC11／AC12。

* [x] 扩展现有 `tests/hosts/connectivity` 和网络 harness，新新与两种固定旧端 × 新端两角色 × Direct／强制 Relay，保留旧旧基线及全部必需结果。 独立宿主能力不扩大冻结current-engine集合；main5cbca8与a07原库不打补丁，新新2／混合8／旧旧4共14格全量通过并接入原CI，原Blob10格继续完整执行，见[生产矩阵验证](2026-10-24-transfer-progress-connectivity-feasibility.md#生产新旧端网络矩阵验证)。
* [x] 网络矩阵精确读回实际文件、核对真实 selected path／ALPN；旧端验证文件互通和新版保守投影，不强求旧端实现 ACK／查询。 20份方向进度由稳定完整操作与同连接实际握手的协议／路径证明；4份v2逐项完成及Settled，16份v1保持旧版保守语义，读取实际文件与升级前原件；严格门禁拒绝缺方向、缺原始记录、假终态、旧二进制及未命中双栈隔离。
* [x] 真实三端源设备／provider 分离，以及同条目向双设备并发传输、旧登记／尝试／轮次乱序均通过。 三个真实ProductionRuntime验证C提供／B发消息／A保存和B提供时A／C双冷下载；另一目标的失败或完成不能替代A，最终文件精确读回，peer／attempt独立。原Native登记／attempt／round／迟到ACK和完整接收及固定9资源回归通过，见[三端归属验证](2026-10-24-transfer-progress-connectivity-feasibility.md#三端消息来源与提供端双目标进度归属验证)。
* [x] 真实阻塞一端读流／ACK 时，另一端仍前进且取消独立；队满、缓存淘汰和超大批次未知项结果准确。 两项真实三Runtime证明SDK读取／本机取消与实际采用后ACK等待按peer隔离；原67项容量场景实际跨过60秒后保持62完成／2取消／3未确认，完整核对不重下。App队列／全局及每端淘汰、Native86及原固定9资源通过，见[跨设备隔离验证](2026-10-24-transfer-progress-connectivity-feasibility.md#不同接收设备的真实读流与ack等待隔离)。
* [x] 至少回归原 1GiB 负载，观察通知任务、连接、内存、FD、许可及清理前后；连续通知／查询／重连无持续资源增长。 原双1GiB及完整采样屏障保持，两轮实际162次Update／24查询／28完整核对／4次原受管重连，原生进度活跃句柄维持4、关闭归零，原内存／FD／线程／任务和配额阈值通过；严格原资源9与17项校验反例通过，见[连续资源验证](2026-10-24-transfer-progress-connectivity-feasibility.md#大文件与连续进度核对及重连的资源验证)。
* [x] 回归原文件部分失败、成功项保留、目录发布／回滚、单项重试、授权／删除及重启恢复，保持原阈值与下载预算。 新增三根真实发布／撤回失败后的SIGKILL恢复，实际SQLite保留残留责任，重启清旧输出且保留用户复用的已撤回名称；Engine7／App86／SQLite8／Fs9通过，其余未改动接收与固定资源证据继承#321，见[原接收专项验证](2026-10-24-transfer-progress-connectivity-feasibility.md#原接收语义与目录回滚重启恢复专项验证)。
* [x] 观测沿用原完整接收／手动重试动作，用户核对有完整触发和结果；heartbeat、ACK 和补发不各建业务根，不延长已结束动作计时。 真实Engine七次完整核对／六次普通读视图、原接收partial及重试ok在通知仍持有时结束；固定12种结果、敏感哨兵与唯一完成日志通过。完整资源验收发现的普通保留连接关闭Busy由原manager按实际结算修复，显式退休原契约保持；App连接126／进度82、Native86、Engine57及原固定资源9通过，见[业务核对观测与结算验证](2026-10-24-transfer-progress-connectivity-feasibility.md#用户完整核对的业务观测与保留连接结算)。真实Collector按下一项独立验证。
* [x] 真实 Collector／Jaeger 验证正常、部分失败、取消、未知和核对的列表／详情、原始输出与固定分类；敏感哨兵及测试／产品隔离通过。 严格原模板、真实Engine三场景与七次核对、八类具体样本的列表／展开详情及18张截图完成；五种实际反例拒收，test／production筛选隔离。另两条正常完成日志远端丢弃按原queue_contended健康记录单独保留，不能宣称所有记录完整或由完整样本覆盖缺失，见[真实后端验收及丢弃边界](2026-10-24-transfer-progress-connectivity-feasibility.md#真实-collector-与-jaeger-的业务验收)。产品宿主／PostHog未验收。

交付物：固定源码和二进制的可复跑网络／资源／观测证据，首次失败与修复证据均保留。

## 阶段 6：回归、当前候选 CI 与文档交付

目标：完成 AC01–AC13 的逐项审计，再归档。

* [x] 当前候选定向协议、Core、Application、Infra、Engine、SQLite 和绑定检查全部通过。 固定#325干净源码，协议／Core／App1844、独占诊断1、实际登记权威8及13包1736项全部通过；清单与每条PASS／JUnit精确一致，共3589项。六个SQLite合同与Engine64／UniFFI86／NAPI51／宿主39均实际执行，见[当前候选定向核验](2026-10-24-transfer-progress-connectivity-feasibility.md#当前候选跨层定向合同核验)。原ignore及物理／跨平台未测项如实保留，不替代下一项全量回归。
* [x] workspace、成员全集、升级 smoke、前六片与第七片必需网络及资源回归通过；不以隔离重跑成功覆盖原全量失败。 原完整workspace4927、成员56、升级smoke5维、Blob10格、进度14格、资源9项和可达性11类均完成；首次workspace平台配置失败与成员F2间歇失败分别保留，后者根因未定、不称已修复，见[完整回归与首次失败保留](2026-10-24-transfer-progress-connectivity-feasibility.md#当前候选完整回归与首次失败保留)。
* [x] 本地根门禁全部通过，源码／依赖未变化后不无理由重复构建；单负责人、共享 target、保留编译缓存。 对齐原读流隔离测试与Linux屏障装配条件，两项原隔离用例及根六项全部通过；不以本地结果宣称macOS／Windows候选已绿，见[平台条件与本地门禁](2026-10-24-transfer-progress-connectivity-feasibility.md#本地根门禁与读流测试平台条件)。
* [ ] 将本片必需网络／资源用例接入原 CI；对同一交付 head 核对 Linux／macOS／Windows、iOS／Android／HarmonyOS 必需编译与线上检查。
* [x] 真机切网、睡眠／前后台和 Desktop／移动产品宿主采用分别标明通过／失败／跳过，主机／模拟器结果不得冒充真机。见[平台与设备状态矩阵](2026-10-24-transfer-progress-connectivity-feasibility.md#平台真机与产品宿主状态矩阵)；此项只完成逐项登记；物理挂起／后台按用户要求跳过，产品采用仍未执行，不据此声称任何实机或产品通过。
* [x] 按 spec AC01–AC13 核对唯一证据映射和全部跳过项，存在未经用户明确允许跳过的必需未执行项时，不能登记整片完成。见[唯一证据映射](2026-10-24-transfer-progress-connectivity-feasibility.md#ac01ac13唯一证据映射)，当前10项已验证、AC08实机部分按用户要求跳过，AC11／AC13保持未完成。
* [ ] 稳定结论回写所属 Design Doc／必要 ADR，更新入口；实际完成后将 spec／plan 一起移入 completed，不关闭整个 #34。已在[稳定协议合同](../../design-docs/transfer-progress-protocol.md#模块所有权与调用边界)纠正早期待接线描述，明确当前运行期负责人及关闭监督；物理验收已按用户要求跳过；AC11失败复核和最终候选CI仍未闭合，本项继续未完成，不提前归档。

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
bash scripts/testing/run-progress-connectivity.sh
bash scripts/testing/run-blob-lifecycle-resources.sh
bash scripts/testing/run-peer-reachability-connectivity.sh
```

网络矩阵需要 Linux 隔离网络权限；使用现有网络和 relay host，不另建只用合成地址的替代框架。
文档交付单独核对相对链接、索引、11 节结构、行为覆盖和 diff 格式；不等于上述行为回归已运行。

## 当前状态与下一个动作

- 当前正式任务55/59，余4；spec验收10/13，AC08／AC11／AC13未完成。生产双版本进度、兼容源身份头、完整接收成果／逐项通知／采用与ACK、稳定查询／绑定、原再次发起核对、删除／成员退休和保守重启视图已经接线；新新及两个冻结旧端的14格网络矩阵、原Blob10格，以及本项三端归属验证已取得主机证据。各项证据以对应复选框和[可行性记录](2026-10-24-transfer-progress-connectivity-feasibility.md)为准。
- 下一个实施动作：定位最新#332旧旧Relay基线的peer_rejected／apply_failed，保留原失败；本机原独立12轮均未复现，不能称修复；设备／产品宿主状态已逐项登记，继续核对同一交付head CI及物理时间缺口。原必需网络／资源均已接入原工作流；Engine job实测触及60分钟外层上限，已调整为90分钟，所有单项期限／负载／阈值不变。显式触发stack未自动运行的原Windows门禁，分别核对Linux／macOS／Windows与三端绑定。保留成员F2间歇失败未定位状态；旧head CI与本地主机绿不替代最终交付head的检查。
- 阶段0实机挂起及设备／产品宿主、当前候选跨平台CI和最终文档交付仍待完成；AC唯一映射已经完成。Collector已完成正常与退化证据核对，真实日志丢弃边界保留。主机和普通计时结果不替代物理挂起，未执行项不记为通过；本片继续active，不合入或发布当前stack。
- 按用户指示，每个完整任务通过本地交付检查后创建未合并stack PR，触发一次CI并直接继续下一项，不等待前层CI。


### 2026-10-26：生产会话与稳定发送侧视图任务

生产节点的双进度版本已进入同一Application对象图，原接收CAS/保存事实驱动登记、逐项通知、采用及ACK。
用户已批准兼容消息头扩展；新clipboard-applied/3携带源UUID，旧2/1仍保留冻结字节，不能因失败重发旧格式。
当前完成范围及复跑入口见[主机验证](2026-10-24-transfer-progress-connectivity-feasibility.md#生产会话与稳定查询的主机验证)。
本任务创建未合并stack PR后继续原再次发送核对、删除/成员/会话退休接线；对应未完成复选框保持原状。
实机挂起、完整旧新Direct/Relay/三端/资源和产品宿主矩阵仍未闭合，本片不登记完成，也不合入或发布当前stack。


### 2026-10-26：成员发布与在途发送意图退休

完整成员事实仍由 `MembershipOwner` 提交；Application 在同一对象图中强制注入进度退休能力，
调用方继续只提交原移除／重置动作。Owner 在发布前按签名身份及数据库世代同步退休关联，
不依赖 watch 差集，不将无关成员修订或短暂离线当成离组。原提交失败不回滚成员事实，
派生缓存失去可信状态时封住旧意图；下次读取由原 Owner 重载，进度队列不持久恢复。

每次原发送单独围住已退休目标，防止首个异步等待之后重新建卡；同条目向其他设备的既有结果保留。
长期阻塞动作至多保存 256 个不同退休目标，超过后拒绝该旧动作继续建立投影，
保留其他已建投影及后续新动作。此界限沿用投影总容量，不影响原文件发送／结算结果。
本任务的成员清理不单独完成暂停／后台／系统挂起全项验收；正式进度仍为 29/59。

实际验证：Application 1384 通过／2 跳过，真实 Engine 两种完整动作通过；复跑入口和证明边界见
[成员退休主机验证](2026-10-24-transfer-progress-connectivity-feasibility.md#成员发布与在途意图退休的主机验证)。


### 2026-10-26：原再次发送动作内核对

原ResendEntry已接上同一进度owner的完整核对，增加明确新传输意图及逐目标完整结果；不要求宿主拼接查询与下载。
不依赖消息头送达推断文件成果，不要求为核对重建已丢失的原内容；原接收CAS和清理责任不变。
实际行为、复跑入口和证明边界见[再次发送验证](2026-10-24-transfer-progress-connectivity-feasibility.md#原再次发送动作的主机验证)。
本项功能验证后正式30/59；平台挂起、查询竞争完整矩阵、同资料重开及全部阶段5/6验收继续保持未完成。


### 2026-10-26：新版剪贴板的故障注入覆盖

前层CI的`failed_content_dial_preserves_peer_connection`在实际接受1/预期0处失败。
同一预构建真实Engine场景连续3次复现，原因是dev-tools内容拨号选择仍只包含旧版，漏过生产优先使用的源身份版本。
补齐同一协议常量后，原场景真实拒绝拨号、保持对端连接、失败内容不重放且后续用户发送正常。
此项修复故障注入的有效性，原断言、原预算和正式30/59进度保持；不将它作为实机或完整网络矩阵证明。


### 2026-10-26：会话退休后的活动确认

真实接收端锁定／解锁使运行期来源退休后，源端核对收到NotRetained却仍显示下载中的问题已复现。
修复由Application原进度负责人在完整核对内撤销当前活动确认及绑定，保留文件事实、原期限与修订围栏；
调用方继续只执行原锁定、完整核对或再次发送动作。重启不恢复无持久依据的绑定，不新增持久字段或协议版本。
完整终态、初始等待及过期回复的边界保持，后续完整核对可恢复确认但重复事实不能续期。
验证入口及实际结果见[会话确认验证](2026-10-24-transfer-progress-connectivity-feasibility.md#会话退休后的确认验证)。
此项独立修复不完成整个生命周期／物理挂起矩阵，正式进度仍为30/59。


### 2026-10-26：活动接收暂停恢复与源端资料重开

补齐真实双Engine四种会话动作，保留原文件结算、暂停等待和默认不重下责任；
测试fixture保留原安全存储实例供资料重开，不改变生产持久化或恢复策略。
实际动作与证明边界见[主机验证](2026-10-24-transfer-progress-connectivity-feasibility.md#暂停恢复与同资料重开的主机验证)。
后续仍须独立进程、普通重连预算与平台矩阵，正式30/59保持。


### 2026-10-26：普通恢复机会的生产接线与原预算

核对发现原补发服务的network_recovered仅在测试调用，生产恢复没有获得该机会。
Application在Space运行期启动前订阅原可达性事件，由原ProgressRuntime监督观察者；
只在线事件触发对应peer机会，丢失提示合并一次全局机会，原worker继续决定资格、次数和原截止。
关闭仍由原运行期统一完成，不新增Engine步骤、持久通知或第二个补发循环。
实际观察者／worker与真实连接重建证明见[验证记录](2026-10-24-transfer-progress-connectivity-feasibility.md#普通重连与原预算的主机验证)。
本项不完成整个物理挂起／生命周期矩阵，正式30/59保持。


### 2026-10-26：原接收计划与尝试的交接验收

阶段3首项完成：继续由原接收owner持有计划、CAS接受及实际attempt，进度仅采用同一原事实。
交接前后、源提示与旧格式、定位能力透传和错误设备查询隔离已由完整入站用例验证；
原计划跨尝试及旧动作围栏由接收目录回归验证。本项没有生产实现变化。
正式进度31/59；真实文件成果链、实际三端、物理挂起、独立进程和其他阶段3/5项仍按原范围验收。


### 2026-10-26：放弃删除等待的开始边界

前层CI真实删除用例20秒清理巡检超时。可控首次轮询延后使原测试3次稳定失败，
额外完整查询确认原来源仍保留、操作已在导出放行前报告取消，证实原测试缺少已开始业务的前提。
修正仅测试：先确认原来源存在，再通过完整核对观察删除将来源退休，随后放弃原50毫秒等待。
原20秒全部清理断言、实际输出屏障、取消完成事件及重复删除NotFound保持，正常/延后调度均验证。
证据及推断边界见[验证记录](2026-10-24-transfer-progress-connectivity-feasibility.md#放弃删除等待的调度边界验证)。
正式31/59保持，本项不修改生产删除或取消语义。


### 2026-10-26：真实成果与精确ACK链路

在已有真实Engine/SQLite/文件屏障上增加仅测试的真实wire ACK观察，核对第三项输出持有期间和放行后两时点。
首轮只取得一份ACK；独立查询红测进一步确认，已实际结算的失败项被父批次finished过粗地挡在Processing。
修复仍在原ReceiveBatch负责人内保存单项实际失败／取消结算事实，正在清理的项继续等待，目录仍保留整体围栏；
没有增加持久字段、阶段查询、第二套流程或通知预算。目录23项、最小真实链、完整Application1392项及真实Engine接收42项通过；Application2项和Engine1项按实际条件跳过。
阶段3第二项完成，正式32/59；根交付检查随当前候选执行，完整输出阶段矩阵、实际三端与设备验收保持未完成。


### 2026-10-26：启动重叠场景的完整预算

前层coverage连续两次在同一独立进程启动重叠场景20秒被nextest终止。
固定原二进制、原12轮及全部断言下，单CPU重放触发同一20秒终止；
相同CPU限制直接执行26.42秒完成全部12轮，各宿主正常退出且没有损坏误分类。
只为该精确测试配置120秒外层预算，保留每次操作期限和零重试；
单CPU下ci原四项全部通过（重叠项34.02秒），default精确项通过（25.99秒）；12轮工件均核对正常退出。
不把本机复现当作旧CI全部原因的证明，候选coverage结果仍须核对。
正式32/59保持；本项不改变生产启动／关闭、存储分类或第八片设备验收范围。


### 2026-10-26：真实结算边界与通知隔离

原双Engine、SDK、SQLite与磁盘流程覆盖AEAD校验前、输出返回前、提交拒绝和实际清理失败。
源端完整核对保持Running／Processing，实际100%或文件已可读都不授予终态ACK；
正常密文保存并读回，篡改副本由原AEAD拒绝且精确确认IntegrityCheckFailed。
来源离线且反向通知已实际进入永久等待时，原失败项缓存重试仍提交本地成果，成功文件与路径保持，关闭不等待通知放行。
完整接收45项通过／1项Collector跳过，新边界组nextest ci模式3项通过；仅精确表示双场景配置90秒外层预算，原操作期限及零重试保持。
阶段3第三项完成，正式33/59；本项只有测试装配与行为验收变化，不扩大稳定接口或持久化。
其他逐项汇总、完整故障／容量／三端／设备／业务观测矩阵保持原范围，不提前勾选。

阶段3第四项完成，正式34/59。修复真实入站port丢弃源ID及既有空根`.`标记无法物化；没有新增消息或持久化版本。完整Application1395通过／2跳过，Engine接收50通过／1跳过。设备与完整兼容矩阵继续保留原验收责任。

阶段3第五项完成，正式35/59。新增真实手动重试源端视图及专属连接旧尝试围栏验收，不改生产流程、稳定接口、线上或持久化格式；当前设备及完整兼容矩阵继续保留。前层CI审计阻断和生命周期失败另行留证处理，不以本机通过抹去首次失败。
