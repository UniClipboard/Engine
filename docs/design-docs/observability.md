# 运行期观测

## 目标

运行诊断必须同时满足两件事：能够把一次跨设备动作从发送端串到接收端，也不能让观测需求改变业务接口、流程顺序或隐私边界。
产品分析是另一套合同，不共享身份、流程号或发送许可。

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

log 允许 `uc.domain`、`uc.operation`、`uc.role`，并只额外允许：

- `event.name`
- `uc.outcome`
- `error.type`
- `duration_ms`

span 还可使用 OpenTelemetry 自身的 name/kind/status 控制字段，名称只来自固定 operation 枚举。日志事件名取 `event.name`，正文
固定为空；源码位置、线程和 busy/idle 元数据在设备编码前关闭。错误只记录固定类别，不记录 source 正文。缺失值直接省略，不写
空字符串、占位身份或 `unknown-id`。设备编码前不仅检查字段名，还逐项检查 operation、domain、role、outcome、error type、flow 格式
及其组合；span name 必须等于 operation，附加 body、event、link、tracestate、scope 属性或任意合法字段名下的自由字符串都会整条丢弃。

`TraceId` 表示一次在线因果执行。`uc.flow.id` 只用于聚合同一业务 owner 已经拥有的随机、持久 attempt，可跨重试生成新的 trace；
schema v1 只允许 Space 准入的 Joiner client span 携带 flow，server、endpoint、log 和其他领域必须省略。Application 的完整准入恢复
owner 只能用 32-byte attempt 材料开启异步不透明作用域，不能读取或返回 flow；Engine、Infra、Core 和公开接口均没有 flow 构造
入口。禁止从内容摘要、设备、密文、路径或 TraceId 伪造。产品 analytics 不携带两者。

## 跨设备传播

Application 的完整准入恢复 owner 在调用现有 transport 前开启不透明 flow 作用域，不增加 facade、port、result 或 Core 字段。
Infra 当前只传播 W3C `traceparent`，不传播 `tracestate` 或 baggage。发送时从当前 client span 注入；接收时先完成既有业务身份和
消息认证，再从空 Context 提取并在 span 第一次进入前设置 parent。缺失、损坏、超长或未认证 context 全部忽略，业务消息继续按
原规则处理。

context 由既有认证传输保护。Clipboard 使用端到端认证 QUIC request；已有独立消息 MAC 的协议把 context 纳入该 MAC。不得为了
观测修改 Application/Core 的内容加密 AAD，也不得让 context 参与授权、去重或业务摘要。

Space 准入的认证消息往返由 Infra 记录为通用 `network_transport` client/server span；Engine 只在既有认证消息 endpoint 上记录
一个完整 `space_admission` 子节点，原样转发输入输出，不读取消息、编号、状态或步骤。真实调用树固定为 client transport ->
server transport -> sponsor endpoint，不包含 JoinRequest、Prepared、Applied 等 Application 内部业务步骤。连接建立的完整
`space_admission` client span也归 Infra，不由 Engine 从业务编号构造。成员观测同样只保留完整网络交换；账本读取、提交和分支恢复
子步骤不进入 Engine 观测。

Space 的 OPAQUE 认证握手保持原布局；认证后的 Request/Reply 使用新的固定 frame kind，旧 kind 只映射为
`PeerUpgradeRequired`。当前普通协议错误使用独立关闭码，不能被误报成升级；新旧判断发生在密码证明之后。当前 Engine 尚未发布，
因此该 clean cutover 不增加 Engine 或协议版本号，也不保留双 reader。Sponsor 只有收到 Joiner 对 reply 的确认后才记录成功；认证、
业务处理、reply 和确认共用一个总截止时间，缺少确认、错误确认或超时都只记录一次明确失败。认证前没有可信父关系：失败只写一条
带真实耗时、无 TraceId/SpanId 的完成日志，不制造接近零耗时的 root span；认证成功后才建立三层 trace。每个三层节点恰好对应一条
完成日志，日志只通过 TraceId/SpanId 关联，不复制 flow。

## 输出与隐私

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

性能门禁必须同时报告端到端总时长和本机时长。现有同 TraceId 且 server parent 精确指向 client 的 `network_transport` 外壳只能
作为诊断粗估：发送端外壳仍含部分编码、认证与回包校验，初始连接又未完整覆盖，两种偏差方向相反，因此既不是严格上界也不是
下界，不能作为一秒通过依据。精确区分纯网络等待、本机工作和重复样本 p95 由规格 038 完成；server 内的存储、认证、加解密和
业务处理始终计入本机预算。
