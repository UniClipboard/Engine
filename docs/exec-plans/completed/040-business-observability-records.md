# 规格 040：以完整业务动作组织观测记录

- 状态：已完成；S1–S6 已按本机验证矩阵实施并验收，实际产品宿主与真实 PostHog 投递未执行。
- 日期：2026-09-06。
- 依据：[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)。
- 基线：`a95287f7` 加当前工作树中已验证、尚未提交的成员观测修复与标准文档。不得覆盖其他存储升级、构建目录或观测运行时整理工作。
- 完整负责人：各 Application 流程拥有业务意图、关联、稳定结果与恢复；Engine 仅装饰既有完整能力。运行期负责人拥有技术关闭与恢复。
- 唯一调用：产品继续使用原有 Engine 操作；不新增业务 facade、port、结果字段或步骤查询。
- 重试责任：保持现有持久恢复与在线任务负责人不变；观测不能触发重试或延长业务等待。

# 1. Overview

当前一次配对和复制测试产生多个独立运行期、资料升级和成员交换记录。名称与父子关系正确并不足以说明业务结果：
复制整体观测只按 Result 是否为 Ok 分类，可能把全部离线、全部发送失败或未发送显示为成功；资料升级将 UpToDate、Pending、Busy
等结果统一记为成功，且完成日志发生在 span 作用域之外。手动发送缺少完整入口；成员维护只有一次次网络交换，缺少恢复意图与总结果。

本规格修复观测语义与组织，不改变业务协议、授权、持久化格式、发送回执和恢复行为。

# 2. Goals

- 复制和手动发送的顶部结果能区分完成、部分完成、等待、未发送和失败。
- 无需升级不是一次升级；真实升级结果与完成日志关联到同一记录。
- 用户发送和实际成员恢复有完整入口，直接引起的连续工作沿该入口查看。
- 正常内部清理不形成独立业务记录，异常关闭仍有明确诊断。
- 测试验证完整业务结果和列表，而不只验证字段与节点数量；测试与产品视图隔离。

# 3. Non-Goals

- 不调整配对耗时预算（由 038 负责），不整理所有历史普通日志（由 039 负责）。
- 不修改 Core 业务结果或持久字段，不新增阶段查询，不复制业务判断或解析私有状态给 Engine。
- 不因观测改变异步系统写入、网络确认、取消、重启和重试顺序。
- 不新增观测平台或设备身份字段，不改写已导出的旧记录，不宣称真实 PostHog 项目已投递。
- 不按公开操作清单批量生成独立 trace，不为跨日恢复无限持有原 trace。

# 4. Current Architecture Context

```text
Component: 本机剪贴板完整动作
Path: crates/uc-application/src/clipboard/local.rs
Responsibility: 捕获、保存、登记活动内容、索引和发送；返回既有 LocalClipboardOutcome。
Relationship: 自动复制与手动发送共用；派送结果已有成功、重复、离线、失败与等待数量。

Component: 剪贴板观测装配与入口
Path: crates/uc-engine/src/assembly/observability/clipboard.rs
Responsibility: 包装完整复制调用、保存和系统写入；目前顶部只判断 Result::is_ok。
Relationship: runtime/host_clipboard.rs 已接入；runtime/host_operations.rs 的 send_snapshot 尚未接入完整动作记录。

Component: 资料升级装配
Path: crates/uc-engine/src/assembly/wire/mod.rs
Responsibility: 调用 ensure_v3 并处理升级结果。
Relationship: assembly/observability/storage_upgrade.rs 未区分多种正常返回，且完成日志在原 span 外。

Component: 成员一致性恢复
Path: crates/uc-application/src/space/membership/synchronize_history/target_use_case.rs
Responsibility: 选择实际目标、执行恢复、汇总结果与保留恢复责任。
Relationship: Engine 的 assembly/observability/membership.rs 只记录各次完整网络交换。

Component: 运行诊断与输出
Path: crates/uc-engine/src/runtime/task_shutdown.rs
Responsibility: 关闭后台任务并记录结果；当前正常关闭也建立独立 session_lifecycle。
Relationship: session_supervisor.rs 同名记录会话切换；共同合同与 Collector 控制输出字段。
```

# 5. Proposed Design

## Components

- 剪贴板领域观测只读取既有完整结果，生成固定观测结果并原样返回；不读取条目、摘要、内容或内部步骤。
- 升级观测使用既有完整升级结果分类；调用作用域覆盖完成记录。只有实际升级/初始化/恢复属于相应完整动作，无需升级检查降为诊断。
- 成员完整恢复 owner 持有不透明在线关联，实际工作而非定时空检查建立入口。各轮既有网络能力自然成为子工作；Engine 不获得触发步骤对象。
- 运行诊断与业务动作采用明确类别与默认查询入口。正常清理日志化；异常诊断不伪装成业务失败。保留同一进程资源信息，不按类别伪造 service。

## Data Model

不新增业务数据。诊断合同扩展固定 outcome：`partial`（部分目标已完成）、`skipped`（未执行发送）；保留 `ok/error/deferred/rejected/cancelled`。
`partial/skipped/deferred` 不伪装为错误，节点状态为 Unset；`error` 仍必须携带固定 error.type。全部派送失败采用固定 `delivery_failed`，具体原因仍在目标子记录。
新增值同时更新设备元数据/编码校验、日志校验、Collector 两份配置及合同测试，不接受自由字符串。

S1 分类矩阵（依次匹配；只汇总当前返回的派送结果）：

| 既有结果 | 观测结果 |
| --- | --- |
| 捕获或派送调用返回 Err | error；保留原错误与来源，不改变业务返回 |
| Empty、未请求派送、Skipped 或目标数全为零 | skipped |
| 至少一个 accepted/duplicate，且存在 offline/pending/errored | partial |
| 至少一个 accepted/duplicate，其他数量为零 | ok |
| 没有完成目标，存在 offline/pending（可同时含 errored） | deferred |
| 没有完成或等待目标，存在 errored | error / delivery_failed |
| 调用被取消 | cancelled，恰好一条完成日志 |

部分完成不表示待处理部分已经完成。异步系统写入结果仍独立显示，不得汇总为发送方已获知的粘贴结果。索引为既有 best-effort 能力，本片不把索引失败改写成派送失败。

## API / Interface

- S1 将 Engine 私有 `observe_local_copy` 从任意 Result 改为读取现有 LocalClipboardOutcome；错误仍原样透传。仅内部包装器具体化，不扩大业务接口。
- S3 将该私有包装器复用于已有手动发送入口，以固定完整动作名区分自动复制与显式发送；重发使用既有完整返回结果。
- S4 触发原因与恢复总结果只由 Application 完整 owner 提供给不透明观测作用域；不从 Engine 查询 peer、账本或阶段。
- S5 的类别和日志事件使用共同合同的固定词表；保留安全字段白名单，不为“本地日志”放行正文。

## Workflow

1. 完整负责人接受用户动作或实际恢复触发；无需工作时不生成业务 root。
2. 在该负责人已有调用作用域中延续关联，各完整能力按原顺序执行。
3. 既有业务结果决定完成、部分、等待、拒绝或失败，观测不能反向控制流程。
4. 当前在线执行结束；异步尾部独立结算，不延长网络耗时。后续恢复建立新在线记录。
5. 业务列表呈现完整入口，运行诊断单独可查；异常与必要父子节点不能因降噪被删除。

# 6. Implementation Plan

各片均先建立失败测试，再实现、验证并回写状态。S6 的适用断言必须随前五片加入，不留到最后才补。
默认串行执行，不启动并行 Agent；共享合同、Collector、架构文档与 Cargo 验证不能由多片同时修改。

| 分片 | 修改位置与内容 | 风险 | 通过条件 | 状态 |
| --- | --- | --- | --- | --- |
| S1 复制顶部结果 | Engine clipboard 观测；共同 diagnostics；runtime 校验；Collector；现有剪贴板观测测试 | 部分结果误判；新值被过滤；影响旧取消记录 | 分类矩阵逐项通过，结果原样返回，节点与日志一致；复制正常链路无回归 | 已完成 |
| S2 升级结果 | wire/mod.rs 与 storage_upgrade.rs，真实升级测试 | Busy/Pending 被误报完成；升级作用域断开 | 无需升级不冒称执行；进行中/等待/完成/失败可区分，同一日志关联；不改变升级循环 | 已完成 |
| S3 手动发送入口 | runtime/host_operations.rs、既有重发入口、clipboard 观测、双设备测试 | 重复建 root；丢失目标分支 | 自动复制、显式发送、重发可区分；多目标归同一动作；失败/等待与真实结果一致 | 已完成 |
| S4 成员恢复入口 | Application synchronize_history 完整 owner 及触发者；membership 观测与双设备恢复测试 | 空轮询建记录；长期占用；跨步骤泄露给 Engine | 上线/重启/重试触发可区分；完整结果含已一致/待确认/冲突/失败；原网络父子关系保留 | 已完成 |
| S5 运行期降噪 | task_shutdown.rs、session_supervisor.rs、共同日志合同与 Collector 查询/输出配置 | 丢失异常或必要父节点 | 正常关闭不增加独立业务记录；异常关闭可查；业务/诊断及测试/产品默认分开 | 已完成 |
| S6 整体验收 | space_membership_auto_pairing_e2e.rs、OTLP receiver 测试、Collector 测试 | 用节点数替代业务语义；只测试合成宿主 | 列表入口、部分完成、空工作、恢复触发、异常与隐私同时验证；明确平台跳过项 | 已完成 |

每片交付至少运行定向测试、完整 locked metadata、workspace all-targets check、fmt、架构/隐私检查与 diff check。跨设备或输出合同变化还需真实 OTLP 解码与 Jaeger 实看。

## S1 执行证据

- 红灯：生产包装器收到全离线的真实结果类型时，期望 deferred，实际导出 ok。
- 绿灯：Engine 剪贴板观测 4 项测试通过，分类矩阵覆盖 11 种计数组合以及空结果、未派送和 Skipped；错误来源与业务返回保持不变，取消仍恰好结算一次。
- 观测合同 50 项、运行时 43 项通过；新增真实 HTTP 导出测试同时验证 partial/skipped/deferred/error 的节点、日志、关联和状态。
- 正常复制的双 Engine E2E 1 项通过。部分完成分类本片由结果矩阵与真实导出覆盖，不冒称已经执行多目标部分失败的实机测试。
- Collector 5 项合同测试与实际配置校验通过；新结果已送到本机 Jaeger，展开 Tags 实看 partial。该可视化样本为合成诊断记录，不是性能基准。
- 完整 metadata、workspace all-targets check、fmt、架构/隐私检查与 diff check 通过；仅保留既有无关警告。
- 其他分片未实施；本轮未提交或推送。临时浏览器资料在检查结束后回收。

# 7. Edge Cases

S2 证据：Pending 的旧记录预期 partial、实际 ok 的测试先失败；分类及完整包装 3 项测试通过。
真实双 Engine 首次准备显示 storage.initialize_profile，重启检查只产生无关联的 skipped 日志，不导出空升级节点；完成日志准确挂在原节点。
共同运行时与 Collector 只在合法字段校验后有意省略 profile_storage_upgrade/skipped，不计为丢失故障。
双 Engine 复制/重启验收、观测合同/运行时、全目标编译、metadata、格式及架构/隐私检查通过。

S3 证据：手动发送缺少 clipboard.send root 的双 Engine 测试先失败再通过；真实手动发送与定向重发分别形成独立 trace，后者到达目标。
Engine 剪贴板观测 5 项覆盖结果透传、部分完成、离线、拒绝及取消；同一完整包装作用域覆盖现有多目标 fan-out，实际三设备部分失败留给 S6。
手动发送 trace `c3eac5a92e238b0a71b1a440d6a7b8f2` 已在 Jaeger 展开；重发 trace `5254ccb026f9e25103f727027129d90b` 已核对发送与接收关联。
共享合同/运行时、Collector、全目标编译、metadata、格式、架构及隐私检查通过。

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 空内容、无目标、设置关闭发送 | 不称为已同步 | skipped；后续入口片决定是否不创建业务记录 |
| 全部离线与混合失败 | 等待恢复或全部失败，不误报成功 | 使用既有计数，失败与等待共存时保留 deferred |
| 部分送达 | 不把部分完成压成整体成功 | partial，并保留各目标子结果 |
| 并发复制/多目标 | 不混入另一动作 | 复用已存在的不透明任务关联，测试独立父关系 |
| 业务取消 | 原取消责任不变 | 一条 cancelled 完成记录，不伪造成功 |
| 损坏数据或升级占用 | 不误报升级完成 | 原稳定错误与升级结果分别映射，不输出原始正文 |
| 极大计数 | 不溢出、不增加高基数字段 | 分类使用非零判断，避免相加计算总数 |
| 旧版本/旧记录 | 不改写旧数据、不改变 wire | Collector 与设备准入同步更新；旧 trace 不会补回缺失字段 |
| 后台写入晚于保存或失败 | 网络确认不等待写入 | 保持既有异步写入节点和独立结果 |
| 恢复重启或长期离线 | 不无限持有旧 trace | 新在线执行；只使用已有批准的跨尝试关联 |

# 8. Testing Strategy

## Unit Test

- S1 输入全部成功/重复、部分完成、全离线、全失败、混合等待、Skipped、无派送、空结果、极大计数；调用生产包装器；断言结果分类、恰好一条日志和业务结果不变。
- S2 输入各 ProfileStorageUpgradeOutcome 与失败；断言真实分类及关联，不只断言返回 Ok。
- S5 输入正常关闭、超时、任务失败；断言业务记录数量与异常诊断分别符合约束。

## Integration Test

- 真实 exporter 到测试接收器：partial/skipped/delivery_failed 均进入节点与日志，非法组合、未知文本不进入任何输出。
- 双 Engine：自动复制、显式发送、多目标部分失败、重新上线恢复；断言父子完整性、结果与实际历史/剪贴板状态一致。
- 真正升级场景：无需升级、升级推进、占用等待、失败；检查没有独立“无需升级成功”业务记录。

## Regression Test

- 既有配对单 trace、剪贴板慢写入和写入失败、网络 gate 与协议解析测试保持通过。
- 新 outcome 不影响既有 ok/error/deferred/rejected/cancelled，JSONL 限额与远程不可达不阻塞业务保持通过。
- 可视化检查主列表和详情，不只检查编码通过或节点数。

# 9. Acceptance Criteria

* [x] S1 分类矩阵通过且业务返回不变。
* [x] S2 升级状态和结果关联正确。
* [x] S3 自动复制、手动发送与重发具有可区分的完整入口。
* [x] S4 成员恢复说明触发原因和实际结束状态。
* [x] S5 正常清理退出业务列表，异常证据保留。
* [x] S6 本机验收场景的列表与详情按业务意图和结果组织，父子完整、测试与产品隔离。
* [x] 所有新增诊断值通过设备、Collector 与原始输出隐私验证。
* [x] 实际平台与真实后端未执行的检查标为跳过，不能冒充通过。

# 10. Risks and Trade-offs

S4 证据：摘要交换没有恢复父节点的 E2E 先失败再通过；Application 同步 11 项回归、真实导出触发/结果/空检查测试通过。
复用现有维护触发枚举，稳定结果由原报告计数映射；已记录冲突证据或关系时才标记 conflict。没有实际目标的空报告不导出节点或关联日志。
设备上线恢复 `f55dc792830914b4f9c661bb3f51709b` 在 Jaeger 实看为恢复→交换→对端回复，启动恢复与等待结果也已实际导出。
共享合同/运行时 50 + 44 项、全目标编译、metadata、格式、架构和隐私检查通过。没有扩大业务接口。

- 新结果值要求设备和 Collector 一起更新；只改一端会丢记录，因此 S1 包含完整输出链验证。
- 不能通过等候所有后台工作来制造完美总结果；顶部只表述本次已知结果，后续工作保留独立结算。
- 不能以隐藏成功 trace 代替正确归属；先建立完整入口，再调整正常诊断展示，避免孤儿节点。
- 不引入新的启发式匹配或永久 registry。已有稳定结果足够分类，内容摘要与时间邻近不可用作因果证据。
- 保持业务接口不变比创建观测专用业务查询更重要；发现完整负责人缺口时在相应片内校正所有权，不能把步骤暴露给 Engine。

# 11. Open Questions

S5 证据：真实 OTLP 缺少分类字段的测试先失败再通过；编码端派生类别且拒绝调用方冒充。真实 TaskRegistry 的正常、超时、panic 三条路径已通过实际导出验证，正常清理只有日志，异常节点与日志保留。
原生 Jaeger 类别+环境筛选已通过 API 与浏览器实看：业务和诊断各自可查，生产筛选不会返回测试数据。旧记录不删除、不回填分类。
正常 host 启动/恢复运行属于运行诊断，明确的会话重建恢复单独作为业务动作。共享合同/运行时、session supervisor 5 项、全目标编译和仓库检查通过。

S6 新发现：派送 deadline 之后的后台结算原先使用 in_current_span 持有前台 span，会拉长已经返回的记录。生产 spawn_deferred_drain 的受控任务测试先失败，改为已有不透明 ObservationContext 延续关系；后台结果仍按原方式立即落盘。

S6 时间与关联检查进一步发现：Iroh Endpoint/Router 启动的长期任务继承了会话操作，导致前台完成后 span 仍存活；只在库初始化/启动后台循环时隔离 tracing dispatch，后续已认证协议观测保持正常。
无关联日志的 parent: None 也不足以阻止日志 SDK 从当前 OpenTelemetry 上下文补充关联。真实导出测试复现后，在同步记录期间显式进入空上下文；时间验收同时按 operation 匹配完成日志，避免误选嵌套清理日志。

全量 Infra 回归还发现接收关联测试按全局最新 span 查找，在并行测试中可能选到别的调用。定向运行通过；测试改为按本次 client 的确切 trace/span 查找两端，不放宽业务条件或超时。

- S5 查询入口已确定为带固定类别与环境筛选的原生 Jaeger 链接，见 Collector README；不篡改通用 Search，不重启或清空现有 Jaeger，不新增平台。
- S4 不同恢复触发者目前能提供的稳定原因与最终报告需在该片开始时逐一核实；缺失事实不能以“未知成功”补齐。
- 实际手机/桌面宿主及真实 PostHog 项目的测试条件未在本轮提供；不阻塞 S1 的本机输出与业务结果验证，但必须列为未验收。

业务分类来自既有结果，不引入 heuristic；传输、编码、过滤和后端路由属于 Infra/观测运行时。Application 的完整恢复知识不可泄露到 Engine。

## 最终交付证据

- Application 全部 740 项、Engine 全部 146 项测试通过；Infra 全量 793 项通过，4 项既有 ignored 未执行。
- 观测合同 50 项、运行时 44 项通过；Collector 5 项合同测试与实际配置校验通过。
- 最终正常复制、三设备部分完成、慢写入、系统写入失败、未中断配对及中途重启配对六项真实 Engine 场景分别精确运行通过；手动发送与真实定向重发亦通过。
- 真实 TaskRegistry 正常关闭、超时、panic 的实际导出测试通过；受控后台派送结算仍保存结果且不延长前台 span。
- 时间验收要求会话操作的 span 时长与同 operation 完成日志一致；Endpoint/Router 启动隔离后通过。无关联日志不再借用外层 OpenTelemetry 上下文。
- Jaeger 已实看初始化、显式发送、成员上线恢复、三设备部分完成详情，以及业务筛选列表。项目默认查询链接位于 Collector README；不改写通用 Search 或旧 trace。
- 最终三设备部分完成 trace 为 aad40b26a344bdf91a8a46548d2d5ab3；慢写入与失败写入分别为 cbc7bdca2f16cfbae92213860bda09b9、fa609b2cff74553beed1f57ce36f021a。以上是本机合成宿主验证，不是平台性能承诺。
- 完整 metadata、workspace all-targets check、fmt、架构/隐私与相对链接检查通过；保留既有无关编译警告。
- 真实 macOS/手机产品宿主、图片/文件专属跨设备观测与真实 PostHog 项目投递：跳过。未提交、未推送；不包含其他存储与构建目录工作的处理。
