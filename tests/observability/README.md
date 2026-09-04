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

Jaeger 页面位于 `http://127.0.0.1:16686`。查询 `uc-engine` 后应看到
`uc.operation`；Collector 输出应同时包含 `uc.operation.completed`，二者关联号一致。
