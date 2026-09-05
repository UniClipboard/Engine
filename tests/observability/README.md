# 本地运行诊断验收

该环境只用于本地验收。客户端把 trace 和 log 发给 Collector；Collector 将 trace 转给 Jaeger，
并把两类数据以可解码形式写入自身输出。

```bash
docker compose -f tests/observability/collector/docker-compose.yml up -d
cargo run -p uc-observability-runtime --example observability_probe -- \
  http://127.0.0.1:4318/v1/traces http://127.0.0.1:4318/v1/logs
curl -fsS http://127.0.0.1:16686/api/services
docker compose -f tests/observability/collector/docker-compose.yml logs collector
docker compose -f tests/observability/collector/docker-compose.yml down -v
```

Jaeger 页面位于 `http://127.0.0.1:16686`。查询 `uc-engine` 后应看到固定 operation 名；用同一 `uc.flow.id` 可列出完整配对的
多次在线往返。每次消息交换应直接显示三层：发送端 `network_transport`、认证后的接收端 `network_transport`、完整 Sponsor
`space_admission` endpoint；只有发送端根节点携带 flow，日志不复制 flow。Collector 输出应同时包含 `uc.operation.completed`，事件名
稳定、正文为空；认证后的三层节点各自恰有一条完成日志，并使用本节点的 TraceId/SpanId。认证前失败以及其他没有合法 span 的
本机事实保持不关联，不伪造父子关系。

双设备配对真实样例：

```bash
UC_TEST_OTLP_TRACE_ENDPOINT=http://127.0.0.1:4318/v1/traces \
UC_TEST_OTLP_LOG_ENDPOINT=http://127.0.0.1:4318/v1/logs \
cargo test -p uc-engine --features dev-tools \
  --test space_membership_auto_pairing_e2e \
  topology_script_builds_a_two_node_space_through_public_operations \
  --locked -- --exact --nocapture
```

测试结束前会显式刷新。Jaeger 中应出现四组三层消息交换树和一个 admission flow；同一 flow 下另有四条连接建立记录。不得用
没有 server 子节点的连接建立 span 推算网络耗时。

## 生产 Collector 合同

`collector/collector.posthog.yaml` 只保存无秘密的生产路由模板。部署侧提供：

- `POSTHOG_CLIENT_API_HOST`：项目所在区域的 Client API host；
- `POSTHOG_PROJECT_TOKEN`：以 `phc_` 开头的项目 token，不是个人 API key。

模板只把审核目标送出，并在 Collector 再次收窄资源和业务字段；错误 trace 全部保留，其他 trace 固定保留 10%，日志不采样。
客户端始终只连接 Collector。路由和认证分别与 PostHog 官方 Collector 配置中的 `/i/v1/traces`、`/i/v1/logs` 及 Client API
Bearer project token 合同一致。
当前工作区没有生产项目凭据，因此模板可做静态校验，本地不能冒充真实 PostHog 项目验收。
