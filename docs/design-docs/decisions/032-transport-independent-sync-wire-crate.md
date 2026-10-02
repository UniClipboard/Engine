# ADR-032：传输无关的同步线上格式独立为 `uc-sync-protocol`

- **状态**：Accepted
- **相关文档**：[Port 定义](../ports.md)、[Engine 仓库检查](../engine-repository-checks.md)、[`ARCHITECTURE.md`](../../../ARCHITECTURE.md)

## 背景

同步协议的消息类型在 `uc-core`，流程负责人在 `uc-application`，密码与成员历史在 `uc-infra`，
帧编解码与 Iroh 适配器同样在 `uc-infra/network/iroh`。其中一部分编解码按
`AsyncRead` / `AsyncWrite` 泛型编写，并不依赖 Iroh，但与适配器放在同一目录，
线上字节兼容没有独立的验证位置。

## 决策

新增 `crates/uc-sync-protocol`，只保存已经与传输无关的线上格式：

- 剪贴板（`clipboard-applied`）、活动剪贴板状态与拉取、配对（space-admission）、传输进度、成员分支恢复的帧结构、
  magic/版本常量、大小上限与编解码；
- 有界 W3C trace 载体 `WireTraceContext`（只含载体，不含 span 注入与父 span 设置）；
- 每个帧的 golden 字节向量，作为线上兼容回归。

依赖方向为 `uc-infra → uc-sync-protocol → uc-core`。`encode_header` 与 `write_frame` 由调用方传入
trace 上下文，注入当前 span 留在 `uc-infra` 的传输适配层。

不进入本 crate：拨号、重试、准入判断、OPAQUE/MLS、存储、Iroh 类型、流程编排。
`group-update`、成员历史交换与 presence 的帧处理仍内嵌在各自适配器中，需要先从适配器抽出，不在本次范围。

## 约束与检查

- 本 crate 只能依赖 `scripts/architecture/check-engine-repository.mjs` 白名单内的库。
- 线上字节变化必须提升版本号或 ALPN；不得靠修改 golden 向量放行。
- 编解码逻辑搬迁时保持原样，只调整可见性与 trace 上下文参数。

## 未决事项

- 是否继续抽出 `uc-transport-iroh` 取决于构建耗时测量：仓库目前没有 `cargo --timings` 数据，
  任何构建收益都只是估算。本决策不声称构建加速。
- `uc-infra` 与 `network/iroh` 之间的双向依赖（OPAQUE、会话、rendezvous、路由编解码）需要先拆开，
  才可能进一步分离传输适配器。
