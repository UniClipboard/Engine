# 运行期观测

## 目标

运行诊断必须同时满足两件事：能够把一次跨设备动作从发送端串到接收端，也不能让观测需求改变业务接口、流程顺序或隐私边界。
产品分析是另一套合同，不共享身份、流程号或发送许可。

## 业务记录组织标准

本节是已确认的设计与验收约束，不表示现有记录已经全部完成迁移。下文的字段合同、传播方式和输出配置描述当前实现；
不能因为记录已经成功导出，就认定它符合本节的业务可解释性要求。

### 独立记录的准入条件

一条独立 trace 必须说明：为什么开始、要完成什么、最后怎样。函数调用、创建任务、建立连接或停止任务本身不是建立独立业务
记录的理由。装饰既有完整能力，不等于让每个能力调用自动成为业务 root；不得按公开操作清单批量生成无业务意义的入口。

| 独立业务动作 | 开始条件 | 必须表达的结果 |
| --- | --- | --- |
| 配对设备 | 发起一次加入尝试 | 加入成功、拒绝、取消、失败或等待恢复；不把旧成员尚未确认写成全部完成 |
| 复制并同步 | 接受一次本机复制通知 | 本机保存及各目标接收、保存结果；对方系统剪贴板写入独立结算 |
| 手动发送或重新发送 | 用户明确发起发送 | 各目标成功、失败或等待重试；文件块与单个网络包不是独立业务动作 |
| 修改成员关系 | 添加、移除成员或选择设备组 | 本机变更结果与其他成员确认情况分别表达，未确认不能算全部完成 |
| 恢复成员一致性 | 上线、重启恢复或重试触发实际恢复工作 | 已一致、发现冲突、仍待确认或恢复失败 |
| 恢复连接或会话 | 明确开始一次恢复尝试 | 恢复可用、失败或延期 |
| 实际执行资料升级 | 确认需要升级并开始执行 | 完成、失败或需要用户处理；无需升级的检查不是一次升级 |

普通查询、检查后发现无需处理、正常内部清理，不单独占据业务主列表。新增独立动作必须先确定完整负责人、触发条件、完成条件、
成功与失败含义以及重试责任；不能仅给底层调用换一个更像业务的名称。

### 关联与结束边界

- 同一次动作直接引起、连续执行的工作归入同一 trace。例如当次配对直接执行的身份验证、加入处理和成员通知应沿该动作查看，
  网络往返只是其中的工作，不默认成为新的业务入口。
- Application 完整流程负责人持有不透明关联上下文；Engine 只装饰既有完整能力，不能新增业务步骤查询、读取内部状态或编排流程。
  Core 不增加观测字段。运行期恢复和关闭由其现有生命周期负责人承担，不把技术生命周期伪装成业务流程。
- 离线等待、延期或进程重启结束当前在线执行；后续重试或恢复建立新 trace，不让配对或同步无限挂起。跨尝试关联只能使用当前合同
  已批准的 owner 材料，不能临时增加持久观测标识，也不能用内容摘要、设备身份或时间相近来拼接记录。
- 后台系统剪贴板写入可以是原复制的异步后续节点，但不得延长已结束的网络操作。发送方收到保存确认不等于对方已能粘贴，写入失败
  不得反写已经完成的保存结果；不能为了汇总观测结果改变业务回执或等待顺序。
- 接收端缺少可靠关联时明确作为独立接收诊断，不伪造发送方。异常导出、过滤或队列丢弃造成的父节点缺失必须纳入验收，不以剩余节点
  名称清楚为通过依据。

### 业务动作与运行诊断分开

业务动作是默认查询入口，运行诊断是独立的排障类别；两者不应在默认业务列表混排。此要求不意味着每类记录都要新增一个 service，
也不允许绕过当前隐私合同添加任意标签。

- 正常停止后台任务、释放资源、空检查，只保留必要的固定分类日志或统计；作为业务动作内有诊断价值的子工作时可以保留子节点，
  不额外制造一串独立的“生命周期成功”。
- 关闭超时、任务异常退出等进入运行诊断，必须说明具体动作和失败类别，不能只写 `session_lifecycle`。
- 真正执行的后台恢复可以成为独立动作，但要说明上线、重启、重试等固定触发原因，以及实际恢复结果。
- 隐藏例行噪声不能丢失业务动作中的失败证据，不能单独过滤必要父节点、导致完整记录变成孤儿片段。
- 测试记录与实际产品记录明确隔离。测试可以显式导出整批诊断用于验收，但不能让这些记录默认混入实际产品的业务视图。

### 业务可解释性验收

每条独立业务记录必须能回答：

1. 是用户发起，还是上线、重启或重试触发？
2. 本次要完成什么？
3. 哪些工作已经完成？
4. 哪里失败，或还有什么尚未完成？
5. 接下来由系统自动恢复，还是需要用户处理？

这些含义只能用经批准的固定分类和既有稳定结果表达，不记录内容、设备名、路径、原始业务标识或原始错误正文。确需扩展诊断字段时，
必须同步收紧设备编码、Collector 与隐私测试，不能为了满足上述问题扩大业务 facade、port 或结果接口。

验收必须从真实使用流程进入，并检查可视化列表与详情：

- 一次配对和一次复制能够找到两个对应的业务入口，展开后能读懂结果与未完成部分，而不只是名称、节点数量或关联号正确。
- 正常关闭不额外产生一串独立业务记录；后台恢复单独可查且有触发原因。
- 保存成功、系统写入失败、成员尚未确认等部分完成状态不被压成“整体成功”。
- 两端调用树完整，节点结果与日志一致，关键失败直接可查，不要求到后台日志中猜测。
- 测试与产品数据隔离；过滤正常噪声不破坏失败证据；未执行的平台或流程不得记为通过。

## 分层责任

| 层 | 唯一责任 | 禁止事项 |
| --- | --- | --- |
| Core | 只返回纯业务或生命周期结果 | 不依赖 tracing，不携带 trace context、计时或观测步骤 |
| Application | 负责完整业务流程和恢复 | 不为观测增加 port/facade/result，不向 Engine 暴露内部阶段，不做跨层持续计时 |
| Infra | 实现网络、存储和协议；在已认证边界注入或提取 W3C context | 不把 OpenTelemetry 类型或 wire metadata 暴露给上层，不在认证前信任 remote parent |
| Engine | 在既有完整 capability seam 装饰 port，记录稳定结果和总耗时 | 不为观测查询 Application 状态或内部阶段，不解析持久业务字符串，不编排业务步骤 |
| Host/Binding | 安装进程唯一运行时，拥有远程诊断许可和生命周期 | 不让单个 Engine 实例关闭进程 provider，不安装第二 subscriber |
| Collector | 第二次字段收窄、批处理和后端路由 | 不作为设备侧脱敏的替代品，不要求客户端依赖具体厂商 |

接收端是一个容易误判的边界。remote context 只有 Infra 读完并认证协议头后才存在，因此 server span 由该协议的完整 endpoint
handler 创建。强行交给 Engine 会迫使 Core/Application message 增加 context 字段或跨步骤 registry，明确禁止。

## 进程运行时

`uc-observability-runtime` 是唯一进程 owner，统一构造：

- 共享 Resource；
- trace provider 与 OTLP/HTTP exporter；
- log provider 与 tracing logs bridge；
- Apple OSLog、Android Logcat 与其他平台的通用 JSON 系统输出 fallback；
- 有界 JSONL；
- batch、flush、shutdown 和健康结果。

宿主先安装运行时，再创建任意 Engine。相同配置重复安装返回复用结果，不同配置明确失败。远程构造或发送失败只使远程能力降级，
不改变 Engine 启动和业务结果。业务线程只尝试把记录放入有界容量门和官方 batch processor；容量门发生争用或队列满时立即丢弃
并计数，不等待锁或网络。关闭线程串行封口后再等待后台发送，因此关闭后不会接受新记录。
容量门与 exporter wrapper 只统计发送前丢弃总数和最终发送失败，不复制官方批处理、线程或刷新逻辑。发送前丢弃包括格式拒绝、
锁争用、队列已满和运行时已关闭，首个原因使用不同固定分类记录；累计字段不冒充单独的队列满计数。`health()` 还返回失败批次数
和本地文件丢弃数；每类首次故障写一条无正文的本地健康记录，且不递归进入远程 exporter。
直接 Rust、Apple/Android UniFFI 与 HarmonyOS N-API 都公开同一份当前累计健康查询；初始安装结果只表示安装时状态，不能代替
运行一段时间后的查询。

移动 `suspend` 在 Engine 暂停成功后做有界 flush，`resume` 继续使用同一 provider。单个 Engine shutdown 只 flush；只有宿主确认
进程最终退出时才 shutdown provider。flush 与 shutdown 通过同一生命周期门串行；调用方截止时间到达后旧操作可在后台收尾，
但不会与后续刷新或最终关闭重叠。最终关闭先拒绝普通记录，排空期间仍允许固定健康记录，排空完成后再关闭所有输出。只有关闭
线程未启动或本地清理仍未结束时才能重试；已完成但失败的结果必须原样保留，不能被第二次调用改写成成功。shutdown 后不在同一
进程复活。

## 记录合同

所有远程 span/event 使用 `uc.telemetry` target，并通过类型化合同产生。span 只允许以下业务字段：

- `uc.flow.id`
- `uc.domain`
- `uc.operation`
- `uc.role`

完成的 span 同时携带固定 `uc.outcome`；失败时增加固定 `error.type`，两者与该节点的完成日志一致。
编码端还会为完整业务动作派生 `uc.record.kind=business`，为没有业务父节点的其他操作派生 diagnostic；普通子节点不另设类别。
该字段不由 Application/Engine 自报，不改变服务身份或传播业务字段；两类查询仍返回原来的完整调用树。正常任务清理的成功节点有意省略，异常保留。
项目默认业务、运行诊断与失败入口见[本地观测入口](../../tests/observability/collector/README.md)，测试与产品按既有环境字段分开。
复制结果按既有派送汇总分类：全部目标已保存为 `ok`，部分目标完成为 `partial`，无目标完成且仍有离线/等待目标为 `deferred`，
空内容、未派送或没有目标为 `skipped`；全部目标失败为 `error/delivery_failed`。`partial/skipped` 使用非错误状态，不等于整体成功。
这些结果仅描述当前已知派送状态，不能代表对端异步系统写入完成，也不改变既有重试和回执。
这让只有 trace 后端的本地 Jaeger 也能解释失败，不依赖另一个日志查询入口。错误正文、自由文本 status message 和 span event 仍禁止。

log 允许 `uc.domain`、`uc.operation`、`uc.role`，并只额外允许：

- `event.name`
- `uc.outcome`
- `error.type`
- `duration_ms`

span 还可使用 OpenTelemetry 自身的 name/kind/status 控制字段，名称只来自固定能力与动作词表。日志事件名取 `event.name`，正文
固定为空；源码位置、线程和 busy/idle 元数据在设备编码前关闭。错误只记录固定类别，不记录 source 正文。缺失值直接省略，不写
空字符串、占位身份或 `unknown-id`。设备编码前不仅检查字段名，还逐项检查 operation、domain、role、outcome、error type、flow 格式
及其组合；准入和成员历史交换的显示名独立校验固定能力、角色与动作组合，其他 span name 等于 operation，禁止任意文本。附加 body、event、link、tracestate、scope 属性或任意合法字段名下的自由字符串都会整条丢弃。

`TraceId` 表示一次未中断的在线因果执行。Space 准入在 Application 的完整 owner 中创建一个 `local/internal` 生命周期 root；同一次
连续配对的建链、Joiner client、Sponsor server 与完整 endpoint 都挂在该 root 下。`uc.flow.id` 只用于聚合同一业务 owner 已经拥有的
随机、持久 attempt；schema v1 只允许该生命周期 root 和 Space 准入 Joiner client span 携带 flow，server、endpoint、log 和其他
领域必须省略。延期、拒绝、取消、升级阻塞或 Engine 关闭会结束当前 trace；重试或重启建立新 TraceId，但同一 attempt 继续得到同一
flow。Application 不能读取或返回 flow；Engine、Infra、Core 和公开接口均没有 flow 构造入口。禁止从内容摘要、设备、密文、路径或
TraceId 伪造。产品 analytics 不携带两者。

## 跨设备传播

完整成员同步由 Application 的 SynchronizeMembershipHistoryUseCase 持有不透明恢复作用域，固定入口为
membership.recover.{startup|resume|peer_online|retry|state_changed|requested}。现有维护触发者提供原因，不向 Engine 暴露目标身份或内部步骤。
已完成、部分完成、等待、失败与损坏分别按现有完整报告分类；只有已经记录的分歧证据/关系才标为 conflict，不把普通传输失败猜成冲突。
没有目标和实际工作时不导出该业务节点，也不生成指向被省略节点的日志。该作用域只覆盖本次在线执行，不跨重启持有。

资料升级完整包装器在同一作用域记录结果：Upgraded/LegacyReady 为 ok，FreshReady 用固定显示名 storage.initialize_profile 表达准备新资料，
Pending 为 partial，Busy 为 deferred。UpToDate 只产生无关联的 skipped 诊断日志；编码端在验证合法性后省略其空检查节点，Collector 同样拒绝将它呈现为升级业务记录。
不提前查询内部阶段，不改变 ensure_v3 的调用、返回、等待或重试行为。

成员历史交换在协议边界用固定名称区分请求用途：`membership.{compare_summary|request_history|send_history|request_conflict_evidence|send_conflict_evidence|acknowledge|deliver_restricted_event|deliver_restricted_decision}.{exchange|handle_and_reply}`。
Engine 原样装饰既有完整能力，不解析消息。Infra 只根据已有协议消息提供用途；`exchange` 成功表示发起方收到并解码回复，
`handle_and_reply` 成功只表示对方处理并写出回复，不代表双方成员同步最终完成。每条记录仍是一次网络交换，不冒充完整成员收敛生命周期。
协议实现将地址缺失、连接失败、读超时、收发失败、格式/解码失败、编码失败和大小限制分别提供为固定类别；完整调用的私有诊断作用域
保留第一次失败，防止边界原有粗粒度业务错误覆盖它。业务错误、返回结果和重试责任不变，不增加 Core 字段或业务步骤接口。
本机暂停网络工作的请求使用 `network_paused`，不与远端不可用混淆。暂停分支在 gate 处命名，真正发出的请求只在协议处命名，避免重复字段被拒收。

Clipboard 的宿主复制在既有完整调用处创建 `clipboard.copy_and_sync`；Engine 在既有保存与系统剪贴板能力上记录
`clipboard.persist`、`clipboard.write_system`，不读取业务标识或编排内部步骤。发送端每个目标任务、接收广播交接和后台系统写入
都由 Application 延续不可读取的 `ObservationContext`。它只保存不可记录的在线父身份，不持有原 span，不持久化，不参与业务判断。
Application 专用的 `ClipboardReceiverPort` 从 Core 移回接收模块，仍只有原订阅方法；广播容量、串行消费、回执、超时和关闭语义不变。
`ClipboardDelivery` 是队列任务信封，Core `InboundClipboard` 原样放在其中，Core 仍无观测字段或依赖。

接收回执只表示保存结果，系统剪贴板写入仍是异步尾部；失败独立记录，不能反写已完成的保存结果。后台写入沿原接收父关系建子节点，
但不反复进入或延长已结束的网络 span。Infra 接收回复后的连接清理也不计入业务接收耗时。地址解析与建链各自有一条固定分类完成日志。
自动恢复重发不持久化旧上下文；它开始新的在线执行，不凭业务摘要重新拼接旧 trace。

Application assembly 为每个 Engine 实例创建一个中性 Space admission registry，并在 Space Session 重建时复用。完整准入 owner 用既有
32-byte attempt 材料创建或进入不可读取的生命周期 root，不增加 facade、port、result 或 Core 字段；registry 不跨 Engine 实例重启。
Infra 当前只传播 W3C `traceparent`，不传播 `tracestate` 或 baggage。发送时从当前 client span 注入；接收时先完成既有业务身份和
消息认证，再从空 Context 提取并在 span 第一次进入前设置 parent。缺失、损坏、超长或未认证 context 全部忽略，业务消息继续按
原规则处理。

context 由既有认证传输保护。Clipboard 使用端到端认证 QUIC request；已有独立消息 MAC 的协议把 context 纳入该 MAC。不得为了
观测修改 Application/Core 的内容加密 AAD，也不得让 context 参与授权、去重或业务摘要。

Space 准入的认证消息往返由 Infra 记录为通用 `network_transport` client/server span；Engine 只在既有认证消息 endpoint 上记录
一个完整 `space_admission` 子节点，原样转发输入输出，不读取消息、编号、状态或步骤。一次连续配对的真实调用树以
`space_admission local root` 为根；每轮连接建立是直接 client 子节点，每轮认证消息是 `client transport -> server transport ->
sponsor endpoint` 子树，不包含 JoinRequest、Prepared、Applied 等 Application 内部业务步骤。成员观测同样只保留完整网络交换；
账本读取、提交和分支恢复子步骤不进入 Engine 观测。

Space 的 OPAQUE 认证握手保持原布局；认证后的 Request/Reply 使用新的固定 frame kind，旧 kind 只映射为
`PeerUpgradeRequired`。当前普通协议错误使用独立关闭码，不能被误报成升级；新旧判断发生在密码证明之后。当前 Engine 尚未发布，
因此该 clean cutover 不增加 Engine 或协议版本号，也不保留双 reader。Sponsor 只有收到 Joiner 对 reply 的确认后才记录成功；认证、
业务处理、reply 和确认共用一个总截止时间，缺少确认、错误确认或超时都只记录一次明确失败。认证前没有可信父关系：失败只写一条
带真实耗时、无 TraceId/SpanId 的完成日志，不制造伪造的远端父关系；认证成功后才建立三层子树。每个三层节点和最终生命周期 root
各自恰好对应一条完成日志，日志只通过 TraceId/SpanId 关联，不复制 flow。

## 输出与隐私

准入可读名称固定为 `pairing.lifecycle`、`pairing.authenticate`、`pairing.reconnect`、`pairing.receive_request`，以及
`pairing.{request_join|confirm_prepared|confirm_applied|settle|cancel}.{send|process}`。未知请求使用固定的 `pairing.send_request` 或
`pairing.process_request`。动作由 Application 在既有发送与认证请求处理位置提供，Engine 不解析消息或阶段。日志通过 TraceId/SpanId
关联到这些节点，分类字段仍保持原值。Infra 在调用 Iroh 建链时隔离底层 tracing dispatch，避免 noq ConnectionDriver 长期持有短期
认证 span；这段底层调用不输出库内调试日志，外层认证耗时、结果与后续认证业务交换均正常记录。

当前 tracing-opentelemetry 版本不更新已启动节点的名称；Sponsor 完整请求负责人通过固定的 `uc.display.name` 内部字段描述动作，
共同运行时在结束编码时转换为 name 并移除该字段，再执行严格名称与角色校验。Collector 不接受该内部字段；禁止借此传入任意文本。

设备侧系统日志、JSONL 和远程层均默认拒绝普通模块 target。历史 local debug 调用点保留在
`docs/generated/observability-inventory.md`，但不因此获得输出许可。

本地文件固定为 `engine.YYYY-MM-DD.jsonl`，保留 7 天，总量不超过十进制 100,000,000 bytes。owner 只枚举这一严格命名，启动和
跨日时按最旧优先清理；单条记录会使总量超限时整条丢弃。目录不可写时降级到其余输出，不影响业务。文件名解析只有诊断合同一份
事实来源；诊断导出先有界刷新当前文件队列，再识别该严格命名。

Resource 中 namespace、service name 和 schema version 固定；environment、OS 与 app channel 使用固定枚举。`service.version` 只接受
SemVer，预发布标记只允许 alpha、beta、rc，并可选再加一段数字；build metadata 不发送；`host.arch` 由运行时从固定架构集合取得，不接受宿主输入。
绑定配置的 Debug 输出整体隐藏。

Collector 再次按 target、scope、Resource、字段名和固定字段值收窄，并拒绝正文、事件、链接、tracestate、scope 属性和字段组合不一致
的记录。开发环境把 trace 送到 Jaeger、log 送到可解码 debug sink 并保留全量；生产 Collector 模板优先把两类信号送到
PostHog，错误 trace 全部保留，其他 trace 固定保留 10%，日志不采样。客户端没有 PostHog 专用代码。037 环境没有真实项目凭据，
完成的是模板与 Collector 合同验证，不能把它表述为已向真实 PostHog 项目投递。

Android 的 HTTPS client 使用系统证书校验。绑定从首次 JNI 启动入口同时初始化 NDK context 与证书校验器；AAR 构建按 Cargo
metadata 定位并打包维护库提供的 Java 组件，同时附带消费者混淆保留规则。其他平台由同一 HTTP client 使用对应系统校验实现。

任何输出都不得包含剪贴板内容、密码、密钥、完整令牌、邀请、设备名、地址、文件名、路径、profile/Space/member/device/entry/
transfer 原始 ID、摘要、原始错误正文或可恢复派生值。

## 验证

每次修改至少验证：

1. 类型化字段合同和 flow 单向派生；
2. 一条 tracing event 只成为一条 LogRecord，不重复进入 span events；
3. 日志 TraceId/SpanId 与当前 span 一致；
4. 两个真实本机 endpoint 的 client/server parent 正确；
5. 未认证、损坏和超限 context 不被接受且不改变业务；
6. Collector 不可达和队列满不阻塞业务；
7. JSONL 格式、7 天/100 MB、严格枚举和诊断导出；
8. 原始 OTLP、系统输出和 JSONL 的敏感哨兵扫描；
9. Apple、Android、HarmonyOS 与直接 Rust host 的构建和生命周期；
10. 本地 Jaeger 页面和 Collector 解码输出中的真实调用树与关联日志。
11. 未中断双设备配对只有一个 TraceId；延期或 Engine 重启后使用新 TraceId、同一 flow。

性能门禁必须同时报告端到端总时长和本机时长。现有同 TraceId 且 server parent 精确指向 client 的 `network_transport` 外壳只能
作为诊断粗估：发送端外壳仍含部分编码、认证与回包校验，初始连接又未完整覆盖，两种偏差方向相反，因此既不是严格上界也不是
下界，不能作为一秒通过依据。精确区分纯网络等待、本机工作和重复样本 p95 由规格 038 完成；server 内的存储、认证、加解密和
业务处理始终计入本机预算。
