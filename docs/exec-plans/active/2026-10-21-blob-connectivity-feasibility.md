# 第七片阶段 0：SDK 停止可行性证据

状态：**阶段 0 重验中；底层 SDK 与 Uni 原型通过，生产迁移未开始**（2026-10-21）。对应[规格 §11](2026-10-21-blob-connectivity-spec.md#11-未决问题open-questions)和[执行计划阶段 0](2026-10-21-blob-connectivity-plan.md#阶段-0冻结基线与证明可行性)。

## 冻结来源

| 来源 | 精确修订 | 本轮用途 |
| --- | --- | --- |
| 开工 origin/main | `a461b89444efd60081c7ef63f927946f9dfc072d` | 新分支 `feat/blob-connectivity` 的父提交 |
| 设计盘点锚点 | `2154b959fa16ccbdb165f8a92a295a039833bf47` | 用户提供规格的原盘点，不冒充最新 main |
| 历史 rc.15／a07 | `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6` | 沿用前片支持锚点；本轮未构建／运行 |
| iroh-blobs 0.103.0 | `124780fb69768b8506565b2760c435182f9e2d20` | 工作区固定 fork，未修改 |
| Iroh 1.3.0 | `43e232fbcbf11aa9cf03a78ac83f52296f01d6b3` | 工作区固定来源，未修改 |

本轮没有复制真实用户资料、创建临时旧源码树或启动持久旧端进程。测试只用临时合成内容、真实 FsStore 与禁用 Relay 的本机 QUIC。

## 当前调用图盘点

- Core [`BlobTransferPort`](../../../crates/uc-core/src/ports/blob/transfer.rs) 包含发布、ticket、获取、导出、tag／untag、取消。现有取消合同明确允许共享传输连带失败，尚未收紧。
- Application [`transfer/blob`](../../../crates/uc-application/src/transfer/blob/) 的 publish／fetch use case 持有该 port；facade 注册在途任务，取消先等待反向 reporter，再 cancel token、关闭对端下载连接及结算。
- 清理消费者位于 [`clipboard/history`](../../../crates/uc-application/src/clipboard/history/) 的 cleanup、clear、delete、reconcile_missing_files；outbound、clipboard assembly 和 Application 根组装也持有 port。Engine testing 的 GatedPublication 是测试替身。
- Infra [`blobs.rs`](../../../crates/uc-infra-p2p/src/network/iroh/blobs.rs) 缓存 SDK Downloader；warmup 与 Downloader 自拨共存，`shutdown_inflight_fetch` 调用 `shutdown_endpoint`。文件获取使用 SDK 流式 export，现有引用／tag 仍归原 owner。
- [`IrohNode::install_blobs`](../../../crates/uc-infra-p2p/src/network/iroh/node.rs) 装配 FsStore、GC 与 `BlobsProtocol`；SDK provider 自行处理连接。Engine [`assembly/sync_engine.rs`](../../../crates/uc-engine/src/assembly/sync_engine.rs) 负责安装。
- 现有 [`Uni StreamLease`](../../../crates/uc-application/src/connectivity/stream.rs) 提供通用 AsyncRead／AsyncWrite、finish／settled 和实际工作许可；原生 bridge 在 drop 时 reset／stop 当前半流。SDK 所需的显式流 ID、reset／stop 尚未接入，不能据候选签名宣称 Uni 双向通过。
- 原始来源候选为 [`ReceiveWorkPlan`](../../../crates/uc-application/src/clipboard/sync/apply_inbound/materializer.rs) 的 snapshot、V3BlobRef、manifest、original_blob_indices。物化会改写成功路径与失败占位，现有文件会话持有文件名／大小／来源和路径，但没有完整 ticket。是否能在既有 owner 中保留准确重试材料尚未完成运行证明；不把源码候选记为成功。

## 最小复现

入口：[`blob_connectivity_feasibility.rs`](../../../crates/uc-infra-p2p/tests/blob_connectivity_feasibility.rs)。

```bash
just cargo test -p uc-infra-p2p --test blob_connectivity_feasibility --locked -- --nocapture
```

探针先由真实 provider 发出合法 BAO 数据，receiver 使用固定 SDK 的通用 `get::StreamPair` 和 `Remote::execute_get`。测试专属 subscriber 在 SDK `importing bao:` 调用点暂时阻塞 import worker；只识别固定调用点，不输出第三方消息或摘要。首个 SDK 进度后，在接收流注入 `io::ErrorKind::ConnectionReset`，由 SDK 自己返回错误，而非 abort 外层 future。

检查点为：获取 future 已返回错误，但同次 import 仍被门阻塞；全 store 的 `wait_idle` 仍不能完成。释放门后等待同一个 idle future，确认本地工作确实退出，再关闭端点和 store。测试门有十秒故障兜底及 RAII 释放，正常路径不等待兜底。探针通过表示**复现了停止前置失败**，不表示受管传输验收通过。

另尝试在暂停 import 时通过同连接获取第二个 blob；SDK 获取返回 stream reset error 3，具体根因未证明。该扩展已从最小停止探针中撤回，首次失败记录保留；它既不是本片双流成功证据，也不据此追加生产缺陷结论。双流硬验收仍未完成。

## 原因与边界

固定 SDK 的 `src/api/remote.rs::get_blob_ranges_impl` 将 `ImportBaoHandle` 的 tx 和 rx 分别移入 write／complete future，并执行 `tokio::try_join!(complete, write)`。write 因网络／校验错误退出时，try_join 丢弃 complete 和私有 rx；调用方没有拿回这次 import 的完成等待权。

`src/store/fs.rs::HashSpecificCommand::spawn` 将 import 交给独立运行时；`EntityApi::import_bao` 会继续消费已接收的数据并执行最后的 `write_batch`，然后才回报完成。丢弃网络 future 或 reset 当前流不能证明这段本地工作已经退出。

SDK 公开 `Store::wait_idle` 明确是整个 store 的瞬时空闲屏障，并说明并行交互时可能永不完成。将其放入每项取消结算会把 A 的等待耦合到其他下载、提供服务和 store 工作；不满足本片单项停止监督。关闭 endpoint 或整个 store 也不满足共用连接与独立任务隔离。

这个复现证明候选 `Remote::execute_get` 直接桥接方案的失败路径没有完整完成等待权，**不宣称所有低层 SDK 组合都不可行**。提供端 handle_stream 的 export 失败退出同样需要检查其后台工作完成权，本轮未完成该运行证明。

## 后续方案

1. 优先在固定 SDK fork 提供每次交换完整的停止／完成 handle，确保网络失败、取消、store 错误均等待该次 import／export 的实际结算并保留原始错误；新增固定修订后重新跑阶段 0。不得将全局 wait_idle 当作单项 join。
2. 或先验证 Infra 通过公开 `get::fsm` 和 `ImportBaoHandle` 持有网络与 store 两份完成权，并为 provider export 构造可排空、可等待的完整实现。需要先写清该方案相对原高层 SDK 桥接的责任变化，再用同一失败探针验证；不能复制第二套生产协议或跳过提供端证明。

本轮不修改 SDK 来源、blob 协议、持久格式或生产路径。按规格的失败即阻塞规则，不进入阶段 1–5；待明确并证明替代方案后续接。

## 验证状态

- 停止探针：已复现候选高层 SDK 的错误返回早于 import 退出；最终验证命令及检查结果随执行计划记录。
- 本机两流的 Uni 真实双向接入、完整取消结算、原始单项重试材料、旧旧／旧新授权与协议向量、1GiB／资源基线：**未执行**。
- Direct／Relay 旧端矩阵、平台 CI、物理设备：**跳过**，不能沿用前片结果作为本片通过证据。

## 首轮检查点（09d06ea5）验证

| 检查 | 实际结果 |
| --- | --- |
| 最小停止探针 | 1 passed；完整 workspace 分组中也通过（0.145 秒） |
| metadata／workspace all-targets check／fmt | 通过；保留既有未使用代码警告 |
| Rust style／Engine repository preflight／差异空白／相对文档链接 | 通过 |
| 完整 workspace 分组 | 555.896 秒，4305 passed、1 failed、27 skipped，退出码 100 |
| 失败诊断项独立复核 | 已有二进制仍失败，退出码 101 |

完整回归命令为 `just mbx-exec bash scripts/testing/run-test-group.sh workspace`。唯一失败是
`uc-infra-profile::peer_address_read_diagnostics::peer_address_failures_export_stable_categories_and_real_stack_symbols`，
断言要求 stack_status 为 `captured`，实际为 `unresolved`（测试第 270 行）。新增 blob 探针已通过。

直接运行现有 `peer_address_read_diagnostics` 测试二进制同样复现该断言。相关 profile、观测源码及锁文件与开工 main 无差异，本次没有修改任何生产源码；具体符号解析原因未证明，不扩大本次修改或放宽断言，完整回归不登记通过。

手工审查已检查测试工作隔离、停止门故障释放、资源关闭、状态文档与范围；没有新增生产接口、依赖或格式。当前环境未安装 `code-review` skill，执行的是手工审查，不冒称技能审查通过。

本轮日志保存在 `.planning/2026-10-21-blob-connectivity/` 的忽略文件中：`probe.log`、`probe-fixed.log` 保留探针首轮错误，`probe-sibling*.log` 保留撤回扩展的失败，`probe-final.log`、`workspace.log` 和 `diagnostic-existing-binary.log` 保存最终与回归证据。Git 中保留本报告和可重复探针，不提交原始编译日志。workspace 分组按既有脚本排除独立成员全集和升级矩阵；本轮这些项目、平台 CI 与设备检查均跳过。

## 续接：同一 SDK 底层组合与 Uni 证明

采用 spec §5.4 已补充的公开底层 API：下载持有独立 import 完成 receiver，在网络分支退出并关闭输入后等待它；提供端使用 SDK 读取原请求和输出原 BAO 字节，取消时 reset 当前流，继续排空本次 export 直至通道关闭。没有改变 SDK 修订或自定义线上编码，没有接入原生 Downloader 自拨。

`blob_connectivity_feasibility` 的复合原型现在同时验证原高层缺口与替代实现：暂停 import 后取消接收，替代 future 保持等待，另一 Raw blob 精确读回；释放后才结束。暂停 export 后取消提供，同样先停止自身网络并继续监督本次 export，另一流仍能成功读回。

此前 sibling error 3 的具体原因为测试误用 GetRequest::all（HashSeq）获取 Raw blob；修正为 blob 后同连接读回通过。这个错误属于探针自身，不再作为未解释的 SDK 缺陷；首次失败日志保留。

真实 Uni 证明位于 `network::iroh::connectivity::stream::blob_probe::uni_blob_stream_cancel_preserves_sibling_and_local_work_capacity`：使用实际 Iroh 半流 bridge、真实 ConnectivityManager 与 StreamLease；A、B 在同一真实 blob ALPN 连接上分别读至 64KiB 后暂停，取消 A，确认 B 未结束且连接仍可用，再恢复 B，精确读回 4MiB，backend 获取计数为 1。固定身份准入 fixture 只验证认证 key 对应，不冒充当前成员／旧资料授权通过。

新增 `StreamLease::into_parts` 返回 I/O 与 `StreamWorkLease`；工作租约持有连接、活动与原许可，SDK 导入／导出和本地结算完成后才释放。App 的原额度测试现同时覆盖原 hold_for_work 和拆分 I/O 路径。SDK 事件追踪沿用原生产 None 配置，避免把高层 SDK &self 方法的 Sync 要求加给通用半流。

本轮命令：

```bash
just cargo test -p uc-infra-p2p --test blob_connectivity_feasibility --locked
just cargo test -p uc-infra-p2p --lib network::iroh::connectivity --locked
just cargo test -p uc-application --lib connectivity::tests --locked
just cargo test -p uc-infra-p2p --lib receiver_lifecycle --locked
just cargo test -p uc-application --lib transport::tests --locked
```

上述分组分别为 1／3／29／6／20 项通过。工作区编译及本轮最终补充检查结果记录于执行日志。原始材料、旧端授权、完整 Engine 收发、1GiB／资源和平台矩阵仍未执行；阶段 0 整体及后续阶段均不宣称完成。

本轮最终补充：固定 SDK 接收普通与合法空 blob 的反向角色探针通过（0.40 秒）；真实 Uni 测试同时验证无效 reset／stop code 保留稳定分类与 source，且不关闭原连接（0.09 秒）。metadata、全工作区全目标 check、fmt、Rust style、repository preflight、文档相对链接与差异检查通过。完整 workspace／旧端矩阵／平台 CI 本轮未重跑，首轮回归失败仍保留，不能将本轮局部通过视为整个 spec 完成。
