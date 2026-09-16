# 本地观测入口

## 默认入口

- [本地测试业务动作（默认）](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [本地测试运行诊断](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22diagnostic%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [本地测试全部失败（含业务内的失败）](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.outcome%22%3A%22error%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [生产业务动作](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22production%22%7D)

这是项目约定的默认查询入口，不是修改 Jaeger 通用 Search 的默认行为。裸地址仍可以查询全部记录；旧记录不回写类别，必要时用原始搜索查看。
链接使用现有 Jaeger 筛选能力，不修改 service.name，也不需要重启会丢失内存数据的 Jaeger。真实产品与测试按既有 environment 资源字段分开。
Jaeger 的查询与界面能力参考[官方界面配置说明](https://www.jaegertracing.io/docs/2.20/deployment/frontend-ui/)。

## Deep Dependency Graph 的使用边界

当前 Jaeger 2.20.0 的依赖图只作为跨端调用关系概览，不作为完整业务流程图或成功判据。
它沿真实父子路径保留起点，以及服务名称发生变化或 `span.kind=server` 的节点；同一服务的其他内部/client 节点会被折叠。
因此即使按动作显示，也不会展开全部工作。依据：[官方能力说明](https://www.jaegertracing.io/docs/2.20/features/#deep-dependency-graph)、
[对应版本的路径转换实现](https://github.com/jaegertracing/jaeger-ui/blob/v2.20.0/packages/jaeger-ui/src/model/ddg/transformTracesToPaths.ts)。

本项目两端都使用 `uc-engine`，但真实接收节点仍可出现在图中；不需要把设备或 Application/Infra/Engine 伪装成不同服务。
不要把保存、系统写入或普通函数改成 server 来强迫它们出现在图中，也不要为了图的形状新增业务步骤或采集字段。

### 分场景入口与操作

- [配对记录](http://127.0.0.1:16686/search?service=uc-engine&operation=pairing.lifecycle&lookback=2d&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [复制同步记录](http://127.0.0.1:16686/search?service=uc-engine&operation=clipboard.copy_and_sync&lookback=2d&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [设备上线后的成员恢复](http://127.0.0.1:16686/search?service=uc-engine&operation=membership.recover.peer_online&lookback=2d&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22test%22%7D)

1. 从对应搜索入口开始，核对 Operation、Tags、时间范围和结果数量。以上入口固定 test 环境、最近两天、最多 100 条；无结果时调整时间范围，不能认为业务没有执行过。启动恢复或重试另选对应的 `membership.recover.startup` / `membership.recover.retry`。
2. 等结果加载完，再点击 **Deep Dependency Graph**。确认图中显示动作名称，而不只是服务名；图布局和折叠不改变原始记录。
3. 用图判断是否存在对应的接收关系，不用节点数、连线数量或图形相同推断完成情况、设备数量、请求轮数或先后耗时。
4. 点击顶部 **Trace Results** 返回同一批结果，再点击具体记录，展开节点 Tags，检查 `uc.outcome`、`error.type` 与时间线。当前路径已实际验证，保留搜索筛选条件。
5. 部分完成可在原业务入口附加 `uc.outcome=partial`。检查系统写入失败时使用上面的“全部失败”入口，并选择 `clipboard.write_system`；不要保留 `uc.record.kind=business` 去筛选内部失败节点，否则可能遗漏它们。

本版本点击图中 **Set focus** 生成的新查询会移除原 Tags。使用它后必须重新核对并补回环境与其他适用条件；
接收节点不是业务 root，不要机械补回 `uc.record.kind=business`。为保持原样本范围，优先使用顶部 Trace Results。
不把带固定起止时间的临时图地址当作长期默认入口；本版本从相对时间搜索直接附加 `view=ddg`，首次查询时可能回到列表，按按钮进入更可靠。

### 本机核验记录（2026-09-06）

复查既有 Engine 测试产生的记录，没有重新运行产品设备流程，没有重启或清空 Jaeger，也没有修改采集配置。
三类搜索当时分别返回配对 8 条、复制 5 条、上线恢复 15 条；这些数量只描述本次样本，不是产品统计。
浏览器已进入三类依赖图，并从列表进入详情；正常、部分完成、写入失败另按各自时间窗口隔离为一条记录比较。

| 样本 | 依赖图实际展示 | 详细记录核验 |
| --- | --- | --- |
| 配对 `6542ee687a67f4e574f9f727b156949b` | pairing.lifecycle → pairing.receive_request | 17 个原始节点，确认轮次及处理动作仍在详情中；图不是配对步骤清单。 |
| 正常复制 `ac2d9d1a0cb81beb8dc19d99a7b6c64a` | clipboard.copy_and_sync → clipboard_receive | 8 个节点，派送、两端保存及系统写入均为 ok。 |
| 部分送达 `aad40b26a344bdf91a8a46548d2d5ab3` | 与正常复制相同 | 顶部为 partial；不能因在线目标的接收路径完整，就把所有目标都算作完成。 |
| 系统写入失败 `fa609b2cff74553beed1f57ce36f021a` | 与正常复制相同 | 保存和接收成功，write_system 为 error/unavailable；失败节点及 Tags 可在详情中查看。 |
| 上线恢复 `e57828d78781c219012a478350e849de` | membership.recover.peer_online → membership.compare_summary.handle_and_reply | 3 个节点，包含图中省略的 exchange，恢复结果为 ok。 |
| 上线恢复等待 `9900adc3a19da88f96c1451fe0deb324` | There are no dependencies | 原记录仍有恢复与失败交换两个节点，顶部为 deferred；没有 server 节点不等于记录丢失或没有失败。 |

复制的三个对照样本已同时核对原始记录：各自只有一个根，所有父节点都存在。这里图中省略节点是展示规则，不是补日志的理由。
上述浏览器验证使用页面内容与实际点击结果；截图接口超时，未取得截图。样本来自本机测试，真实产品宿主、其他平台和 PostHog 本轮未验收。
Jaeger 使用内存保存，实例重启或数据淘汰后样本可能失效，不能把这些临时记录当作长期可用链接。

## 记录规则

- 业务类别只由合法的完整动作派生，完整调用树不会被拆成两份；子节点保留在父业务记录内。
- 没有业务父节点的网络交换及运行期管理归运行诊断；业务内部的失败仍可通过“全部失败”找到，不能为了分类丢掉上下文。
- 正常任务关闭与无需工作的检查不导出独立节点，但必要诊断日志保留；超时和任务异常仍保留节点及对应错误日志。
- 新代码不允许直接声明 uc.record.kind 来冒充业务动作；字段由编码端派生，Collector 再次校验组合。
- 本地保留全部已准入 trace；生产模板仍按既有策略采样，日志不采样。未执行真实 PostHog 投递不能称为上线完成。

## 验证

运行 `node tests/observability/collector/collector-privacy.test.mjs` 校验两份配置的字段合同。
查看默认入口时应能找到配对、复制/发送、实际升级与恢复，不应看到正常任务清理；查看运行诊断和失败入口时异常不能消失。
测试数据须保持 test 环境，不为验证筛选而伪装成真实生产数据。
