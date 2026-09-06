# 本地观测入口

## 默认入口

- [本地测试业务动作（默认）](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [本地测试运行诊断](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22diagnostic%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [本地测试全部失败（含业务内的失败）](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.outcome%22%3A%22error%22%2C%22deployment.environment.name%22%3A%22test%22%7D)
- [生产业务动作](http://127.0.0.1:16686/search?service=uc-engine&lookback=1h&limit=100&tags=%7B%22uc.record.kind%22%3A%22business%22%2C%22deployment.environment.name%22%3A%22production%22%7D)

这是项目约定的默认查询入口，不是修改 Jaeger 通用 Search 的默认行为。裸地址仍可以查询全部记录；旧记录不回写类别，必要时用原始搜索查看。
链接使用现有 Jaeger 筛选能力，不修改 service.name，也不需要重启会丢失内存数据的 Jaeger。真实产品与测试按既有 environment 资源字段分开。
Jaeger 的查询与界面能力参考[官方界面配置说明](https://www.jaegertracing.io/docs/2.20/deployment/frontend-ui/)。

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
