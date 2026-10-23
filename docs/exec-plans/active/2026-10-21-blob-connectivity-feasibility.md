# 第七切片：实现方案与关键取舍

状态：**方案已采用，最终交付验收仍由执行计划跟踪**。
需求、参数与完整验收标准见[实现规格](2026-10-21-blob-connectivity-spec.md)，
工作范围与验证入口见[执行计划](2026-10-21-blob-connectivity-plan.md)。
本文保留难以从源码恢复的原因和取舍，不保存逐轮排障记录。

## 冻结来源与兼容边界

| 来源 | 精确修订 | 用途 |
| --- | --- | --- |
| 开工 main | `a461b89444efd60081c7ef63f927946f9dfc072d` | 本片冻结的旧端，不随 main 后续推进变化 |
| 历史 rc.15／a07 | `f6f305d9689e4e79e7ab6d0e4921061f9416e4a6` | 历史资料、双角色互通和协议向量 |
| 开工 iroh-blobs 0.103.0 | `124780fb69768b8506565b2760c435182f9e2d20` | 高层获取与 store 关闭缺口的来源 |
| 采用的 iroh-blobs 0.103.0 | `152c42a9217f1377a998b271e0052311e8ada789` | 同一 fork 的兼容 store 生命周期修复 |
| Iroh 1.3.0 | `43e232fbcbf11aa9cf03a78ac83f52296f01d6b3` | 底层连接来源 |

SDK 修订限于已批准的同 fork 生命周期修复，保持版本系列、ALPN、ticket、BAO 与磁盘格式。
旧库不打补丁；测试宿主只通过对应版本已有的公开操作生成和重开合成资料。
协议向量由冻结旧 SDK 生成，不能从当前实现自行生成再比较当前实现。

## 同连接双流与完成权

旧路径的 warmup 与 SDK Downloader 分别建立连接，按对端取消还会关闭兄弟任务使用的连接。
因此下载和提供都迁入 Application 的独立 blob Uni manager，SDK 只消费已经准入的通用流；
删除生产自拨池、对端级取消和旧 provider handler，不能保留并行旧路径。

固定 SDK 的高层获取以 `try_join!` 等待网络与 import。网络分支失败时，import 完成 receiver
随另一个 future 丢弃，但 store 私有运行时中的本地工作可能仍在执行。丢弃网络 future、
reset 半流或观察主 runtime 任务数，都不能证明同次 import 已结束。
整个 store 的 `wait_idle` 又会等待无关工作，不能用作单项 join。

采用同一 SDK 的公开 `get::fsm` 和 `ImportBaoHandle`，由 Infra 分别持有网络与 import 完成权；
错误或取消后关闭输入，仍等待同次 import。提供端使用 SDK 原请求解析及 export 字节流，
停止自身网络后继续排空本次 export 到通道关闭。协议解析、校验与编码继续归 SDK。

Uni 将半流 I/O 与 `StreamWorkLease` 分开交接。本地完成权持有原连接、活动与许可，
网络结束不提前归还额度；每个逻辑项独立结算，SDK 同内容共享不转移兄弟项的清理权。
真实双流测试暂停 A、B，取消 A 后恢复 B，并检查同连接、实际尝试次数及精确内容。

实现入口：Application [transport](../../../crates/uc-application/src/transfer/blob/transport/)、
Infra [blob_exchange](../../../crates/uc-infra-p2p/src/network/iroh/blob_exchange/)。
停止探针见 [blob_connectivity_feasibility.rs](../../../crates/uc-infra-p2p/tests/blob_connectivity_feasibility.rs)，
同连接与来源授权测试位于 [Iroh connectivity](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/)。

## 响应结束与错误来源

SDK export 的 `into_byte_stream` 可能把通道异常呈现为 EOF，不能仅凭流结束报告成功。
Infra 使用 SDK 同源 BAO 遍历器计算期望响应长度，核对实际输出与 store 状态；
继续使用原 SDK 编码，不新增线上帧。空内容、完整／稀疏范围和越过末尾范围都由真实 SDK 对照。

网络错误、停止错误与本地结算错误保留各自来源；自动网络恢复只接受具体来源白名单。
存储、认证、校验、未知错误不能因宽泛的 `Unavailable` 分类而自动重试。
本地事件、输出或提交失败由原负责人修复同一事实，不重新下载或伪造网络终态。

## Store 完整关闭

流级完成权解决单次交换，但不能代替 store 的后台生命周期。旧修订的 GC 持有 store 并睡眠到
下一轮，metadata 关闭确认也不等于外层 actor 和私有 runtime 退出。临时禁用 GC 的实验
没有解决完整关闭责任，因此未作为交付方案。

同 fork 修订由 SDK 生命周期监督器承担完整关闭。调用方仍只调用既有 `Store::shutdown`：
先停止 GC 排队，等待已接受的 GC／import／export，关闭 metadata 与 actor，再回收私有 runtime；
成功或失败回应都在实际退出后交付。加载失败及全部引用自然释放也由同一监督器回收。
确认不能依赖创建 store 的调用方 runtime 继续存活，否则跨 runtime 使用仍会留下退出缺口。
Engine 只消费原 `BlobStoreLifecyclePort`，不编排 SDK 内部步骤。

资源验收分别观察主 runtime 与 SDK 私有工作。进程级 Rayon 共享池只初始化一次后取稳定基线，
同时保留冷计数；不能把共享池初始化误记为每轮泄漏，也不能用丢弃 runtime 伪造任务回收。
SDK 临时 tag 必须在 actor 仍打开时计数，实际连接以弱句柄观察，避免探针本身延长寿命。
资源回归入口为 [blob_store_lifecycle.rs](../../../crates/uc-infra-p2p/tests/blob_store_lifecycle.rs)
和 [默认生命周期资源脚本](../../../scripts/testing/run-blob-lifecycle-resources.sh)。

## 重试材料、身份与交付所有权

物化后的失败占位和本机成功路径都不能重建原始来源。接收负责人保留原始 V3 清单、来源、ticket
及原始位置，使用原项身份创建新 attempt；相同名称或内容的行仍保持独立身份。
材料继续复用已有密文快照及认证产物日志，不新增持久 recipe 或跨重启网络队列。

手动重试以预期旧 attempt 的 CAS 接受新尝试，成功项先复核内容和路径归属再复用，
新尝试不能取得旧成功文件的删除权。来源材料不可用时明确返回失败，不能用当前另一份内容替代。
目录保持完整接收单元和整体发布；暂存区责任在实际创建前通过既有密文日志交接。
完整失败与恢复规则以规格 §5.5–5.7 为准。

消息作者可能与 ticket 中实际提供端不同。当前授权和下载额度都按真实 provider 核对，
缓存命中也不能绕过授权；撤销、旧会话代次与旧 ticket 都不能恢复原工作。
真实三端验收覆盖作者 B、提供端 C、C 的每端上限与 B 的剩余全局额度。

实现入口：[receive](../../../crates/uc-application/src/transfer/receive/)、
[原子接收提交](../../../crates/uc-infra-storage/src/db/repositories/inbound_receive_commit_repo.rs)、
[真实来源授权测试](../../../crates/uc-engine/src/runtime/blob_source_authorization.rs)。
SQLite 读比较后写事务提前取得写权的原因见[Infra 原则](../../design-docs/layers/infrastructure.md)。

## 真实互通与可重复验证

- [协议向量](../../../crates/uc-infra-p2p/tests/blob_protocol_vectors.rs)对照冻结 rc.15 的 ticket、请求及原始／合法空响应。
- [blob 网络矩阵](../../../scripts/testing/run-blob-connectivity.sh)覆盖新新、冻结 main 和 rc.15 的 Direct／强制 Relay 及新版获取／提供双角色。
- 旧旧正常配对后重开，再沿独立资料副本升级一侧；检查身份、成员、历史、原接收文件及新旧双向精确读回。
- 强制 Relay 格配对前隔离双方 IPv4／IPv6 UDP，并核对实际路径、blob 协商和阻断计数。
  完整恢复场景先建立未隔离内容基线，在对应隔离阶段执行严格路径断言，不能用尚未隔离的基线证明 Relay。
- [资源脚本](../../../scripts/testing/run-blob-lifecycle-resources.sh)覆盖六类真实生命周期、实际 provider 配额及连续两份 1GiB 文件，检查 FD、线程、任务、SDK tag 与连接回收。
- 实际命令和最终门禁状态由执行计划及 PR Checks 跟踪。历史探针或局部通过不能替代当前交付候选的完整回归。

原始命令输出、JUnit、计数流、截图、临时源码与失败现场保留本地或 CI Artifacts。
仓库只保存上述原因、实现入口和可重复测试；物理设备未执行时明确记为跳过。
