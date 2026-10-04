# `uc-sync-protocol` 维护地图

取舍与边界见 [ADR-032](../../docs/design-docs/decisions/032-transport-independent-sync-wire-crate.md)，
线上字节兼容规则见 [`tests/golden_vectors.rs`](tests/golden_vectors.rs)。

## 范围

- 只放同步协议的线上格式：帧结构、magic 与版本常量、大小上限，以及基于 `AsyncRead` / `AsyncWrite` 的编解码。
- 依赖方向是 `uc-infra-profile`/`uc-infra-p2p` → `uc-sync-protocol` → `uc-core`。
- 不放拨号、重试、准入判断、密钥、存储、span 注入或任何传输库类型；流程负责人仍在 `uc-application`，传输适配器仍在
  `uc-infra-profile`（admission）/`uc-infra-p2p`（pairing/rendezvous）。

## 硬约束

- 任何已发布线上字节的变化都必须提升版本号或 ALPN，不得直接修改 golden 向量。
- 新增帧必须同时新增 golden 向量，覆盖版本、长度边界、截断和多余字节。
- 依赖只能来自 `scripts/architecture/check-engine-repository.mjs` 中的白名单；新增依赖先更新白名单并说明理由。
- 解码在分配前先校验长度上限；错误保留来源，不字符串化。

修改后运行 `cargo test -p uc-sync-protocol`、workspace check、fmt、架构检查和 `git diff --check`。
