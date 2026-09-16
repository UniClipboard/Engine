# 进度

- 原 Jaeger 记录已证实：发起方失败但节点没有原因，后台只有 stream_failed；两端同名且结果含义不同。
- 真实 OTLP 测试先失败于缺少节点结果字段，修改合同及两道设备过滤后通过。字段只接受固定结果和固定错误类别。
- 成员协议名称区分用途及 exchange/handle_and_reply；Engine 不解析消息。协议第一次失败通过不透明诊断作用域传递，不改变原业务返回类型。
- 成员协议 9 项测试通过，其中真实解码函数面对非法回复时保留 decode_failed 而非被边界 stream_failed 覆盖。
- 观测合同/运行时 50 + 42 项、Collector 5 项及配置校验通过；架构/隐私检查通过。正在运行双 Engine 导出及 Jaeger 验收。
- 双 Engine 验收发现暂停门会在进入协议前返回，已在该失败位置提供固定用途与 network_paused，不再误称远端不可用。
- Jaeger 实看数据发现 gate 与协议层重复设置名称导致 client 被拒收。新增 server 必须找到真实 client 父节点的验收，收敛为暂停分支由 gate 命名、正常路径只由协议命名。
- 新父子完整性测试先失败再通过。新的真实交换 `3f3257571ec3e112a724efef05236da4` 在 Jaeger 两端均可见，固定名称与结果正确；暂停请求 `d304bac8ea2e276c9dfca5727ec7cd0d` 明确包含 network_paused。
- 已用本机浏览器展开暂停记录并截图检查，页面直接显示 error.type=network_paused；浏览器进程已停止，临时资料和截图移到回收站。
- 共享观测回归发现旧取消测试按字段出现次数计数（节点和日志现在均含 outcome），已改为按完成日志条数断言，Engine 6 项观测测试通过。
- 最终真实 OTLP 隐私探针、暂停门测试、配对单 trace 回归、workspace all-targets check、metadata、fmt、架构/隐私检查与 diff check 全部通过。本轮完成，未提交/推送；仅保留既有无关编译警告。
