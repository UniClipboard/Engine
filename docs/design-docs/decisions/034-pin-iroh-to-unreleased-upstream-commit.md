# ADR-034：iroh 固定到包含路径队列修复的上游提交

- **状态**：已采纳；退出条件见下
- **日期**：2026-10-15
- **范围**：根 `Cargo.toml` 的 `[patch.crates-io]`、`uc-observability-runtime` 网络库日志的路径匹配；不改变任何业务接口
- **部分取代**：[ADR-031](031-product-owned-builtin-relay-list.md) 的补充“依赖补丁不依赖下游 `[patch]`”，仅对 iroh 例外；iroh-blobs 与 swarm-discovery 的处理方式不变
- **来源**：[Engine #214](https://github.com/UniClipboard/Engine/issues/214) 的诊断

## 背景

iroh 1.3.0（当前最新发布版）的 `RemoteStateActor` 在连接停留于 `RemoteCidsExhausted` / `MaxPathIdReached` 时，
每 333 ms 重试并无去重地向 `pending_open_paths` 追加同一批候选地址（`iroh/src/socket/remote_map/remote_state.rs`），
同一对端有多条连接时还会成倍放大。上游已在 `43e232fbcbf11aa9cf03a78ac83f52296f01d6b3`（PR #4551）用包含检查给队列设上限，
但该提交晚于 1.3.0，也没有新的发布版。#214 中守护进程匿名堆在 20–24 小时内增长到 17 GiB；这个队列是依赖内部的无界结构，
是目前证据最强的候选，但**没有在本机复现，也没有在受影响机器上证实**。

## 决定

1. 在根 `Cargo.toml` 用 `[patch.crates-io]` 把 `iroh`、`iroh-base`、`iroh-dns`、`iroh-relay` 一起指向上游仓库的该提交。
   四个包同时覆盖，是因为 `iroh-tickets`、`iroh-util` 是 registry crate，只覆盖其中一部分会产生第二份 `iroh-base`。
   不 fork：固定的是上游提交本身，不引入需要维护的分支。
2. 该提交同时带入此前的 8 个提交，其中两项行为变更：IPv6 字面量 relay URL 的 QAD 探测，以及新增发送
   `X-Iroh-Endpoint-Id` relay 头（同时弃用 `X-Iroh-NodeId`）。其余为 CI、文档与 `iroh-dns-server` 的 clap 更新，不在 Engine 依赖内。
3. 网络库日志按“注册表目录名（`iroh-1.3.0`）”匹配规则；固定后调用点位于 cargo 的 git 签出目录，没有这样的目录名，
   所有 iroh 规则会静默失效。`callsite_path` 因此额外接受 `…/iroh-<16 位十六进制>/<提交前 7 位>/iroh/src/…`，
   且提交必须在 `PINNED_IROH_REVS` 允许列表内，仍按逻辑版本 `iroh-1.3.0` 匹配。测试要求允许列表包含根清单固定的提交；
   换提交必须同时评审规则。

## 后果

- `[patch]` 只读取构建根清单。依赖 Engine 的仓库（Desktop）必须在自己的根清单重复同一段，否则它们仍链接 iroh 1.3.0 注册表版本，
  得不到修复，网络库日志规则也按原样匹配。这与 ADR-031 补充的原则相反，只对 iroh 例外，需要在交接文档中通知下游。
- 构建依赖 GitHub 上游仓库的可用性与提交不被改写；与既有的 iroh-blobs git 依赖同类风险。
- 固定后能否消除 #214 的增长**尚未验证**，不能据此关闭 #214。
- 队列有上限后，卡在路径上限的连接仍会每 333 ms 重试一次；CPU 抖动不被此提交消除。
- 不改变 noq-proto 1.3.0 的流重组与 CID 退役队列（上游仍无修复）、noq#750、iroh#4599。

## 退出条件

iroh 发布包含 `43e232fb` 的版本后，删除根清单的 `[patch]` 与 `PINNED_IROH_REVS`，规则改回注册表目录匹配，并通知下游删除重复段。
