# 发现

- 起始分支 feat/active-clipboard-connectivity，HEAD 2154b959；保留原有规划目录和未提交文档。
- 已 fetch origin/main，冻结基线 a461b894；新分支 feat/blob-connectivity 从该基线创建。
- spec 明确阶段 0 任一技术证明失败／无法证明即设计阻塞，不允许提前替换生产路径。
- implement 要求最后 code-review；当前技能目录未发现该技能，继续查找；如缺失执行等效差异审查并如实记录。

- 停止前置失败已复现：固定 SDK 网络错误返回时，受测试门阻塞的同次 import 仍活跃。源码对应 remote.rs get_blob_ranges_impl 的 try_join 提前丢弃私有 handle.rx。
- 全局 Store::wait_idle 不是单项 join，SDK 文档明确并发时可能永不空闲。
- 不宣称所有低层 SDK 组合不可行；可选 fork 完整 handle 或 Infra 持有 get::fsm／ImportBaoHandle 两份完成权，provider export 也须证明。
- /code-review 在可访问技能目录不存在；本轮执行手工调用图与差异审查，不假称调用该技能。

- 续接已证明原阻塞仅限高层 Remote::execute_get：公开 get::fsm + ImportBaoHandle 可保留同次完成权；SDK provider parser + export byte stream 可在取消后排空本次 export。
- SDK Raw blob 应用 GetRequest::blob；旧 sibling error 3 来自误用 all（HashSeq），已纠正并成功读回。
- 本机真实 Uni ALPN、共用连接、并行暂停 A/B 后取消 A、B 精确读回4MiB通过，backend 仅一次获取；实际原生半流显式 stop/reset，StreamWorkLease 在 I/O 释放后仍保留额度。
- 当前阶段0不再是 SDK 技术无路可走；仍需原始重试材料／历史端当前成员授权与资源等前置证明，未切生产路径。

- 阶段 0 前置已完成并提交 dc0c3b22：原材料两项、真实 Uni 当前准入和同内容隔离、固定旧 SDK Raw/空向量、两旧锚点 × Direct/Relay × 升级两侧共 8 组资料基线、1GiB 迁移前资源均已核对。正式新 owner 的验收不能沿用旧适配器结果。
- 阶段 1 已实现但尚未接生产的规则在 transfer/receive/scheduler 与 timing；单调授予时刻保存在 DownloadPermit.started_at，不能从 acquire 返回时重计首数据预算。调度器不 spawn 网络工作，后续完整 owner 必须把许可移入受监督任务，不能在取消调用或外层 future drop 时直接释放。
- 生产取消仍有已知待迁移点：facade 先 remove 注册、await reporter、cancel select 丢弃 fetch、shutdown 对端连接、吞 session 清理错误；不能简单把新队列套在这条路径上假报独立停止。实际改造须让原任务持续被监督，5 秒仅结束调用等待，重复取消等同一结算。
- 现有 SDK fetch/fetch_to_path 的 TempTag 只覆盖 adapter 方法，到永久 tag 还有微小窗口；新交换结果须保留 store 保护，直到引用/输出/tag 必要结算，不复制“理论窗口可忽略”的旧理由。
- 现有 Uni policy 的 incoming 非 presence 默认取 waiters_per_session，accept 以连接为单位占该许可；accept_bi 默认先等许可再取流。blob 的未知入站 16、交换 4/2、每方向一条连接且退出未完成仍占名额须独立落实，不能仅设置现有几个数字就宣称满足。也不能保留第二份原生连接缓存。
- 下一步：Application transfer/blob/transport 的专属完整交换 port；仅接收已准入 Uni StreamLease，SDK/BAO/store 留在 Infra。下载及提供都必须监督准备、import/export 与输出，取消/超时发送停止意图后继续 join，不丢弃 SDK future。采用时迁走并删除 tests/support/blob_exchange.rs 原型副本；生产仍旧，阶段 2–4 全部待完成。

- 本轮已实现 Application BlobExchangePort 与 Infra IrohBlobExchangeAdapter，前者只有 Uni StreamLease/内容身份/取消上下文，后者只有 FsStore。接收保留独立 import rx，提供取消后排空 export，成功 receipt 保留 TempTag + StreamWorkLease；tests/support/blob_exchange.rs 已删除，旧探针全部调用同一实现。
- SDK into_byte_stream 把通道异常隐藏为 EOF；不能只查 store 状态／至少 8 字节。现在用 SDK 同源 bao-tree 遍历器的预期长度 + 实际长度 + store 状态判定完整响应，编码仍是 SDK。直接 bao-tree 仅新增已有锁版本依赖边。公共遍历须按 SDK 组层数压缩，并把触及／超过末块的尾部归一化为开放范围；首轮 16KiB 稀疏用例曾差 192 字节，修复后全部比较通过。
- 停止错误不能只在显式 cancel 分支捕获：SDK 准备／请求头 future 被取消时可能通过 Drop 停止半流。IoCompletion 最多收集两半流各一项兜底失败，真实 header 等待时注入 stop 权限失败已证明保留 IO source，不能返回普通 Cancelled。
- SDK helper 成功不等于业务完成；当前 production facade/Router 仍旧。下一步必须把全局 DownloadPermit 移入受监督的完整工作任务，覆盖网络、输出、引用与所需结算；调用方取消等待不丢任务，5 秒只结束等待，通知非阻塞。还需 ticket 来源解析／当前身份映射、独立 manager 方向名额／入站资源与 30 秒空闲、生命周期及实际 Node/Engine 接线。
- 后续调度接线要特别处理撤销基线：现有 ConnectivityManager.connect_with_cancel 在自身排队前 begin(target)，但 blob 的 DownloadScheduler 在它外部。必须在逻辑请求接受／外部排队之前冻结同一准入基线，并贯穿复用、开流和提交；不能等拿到下载名额后才取新 epoch，导致排队旧任务在撤销／重新加入后复活。

- 外部队列围栏已改为同步 ConnectionRequest；manager 的旧 connect/connect_with_cancel 已删除并统一迁移内部消费者，refresh 复用原请求快照。新请求跨 disconnect_all 会话重启后仍拒绝旧 token。
- Uni 的 supervise_local_work 与 presence 保留分离；checked helper 不补启动已取消／到期且尚未 poll 的本地读，已开始读结算后复合保留主因与真实失败。共同拨号取消／到期只影响自己，最后等待者才 join；flight 结果不再以 Stopped 覆盖真实 backend failure。
- 该接口改造不等于完整 Blob owner 已接入。manager.accept 仍在 await 内取准入，未知入站与识别后额度尚未分离，方向名额及 closing 资源需后续实现；不能直接把 supervise flag 配上就宣称完成生命周期。

- Uni 现有可配置 InboundAdmissionPolicy，未知与识别后额度在真实身份解析／成员读取之间交接；受管 accept 在入口申请未知额度并冻结 token/fence，超额立即返回，不 spawn 额外拒绝任务。准入本地工作由 JoinSet 持有，等待方 drop 后仍完成；到期先关闭原连接，再等待真实读并保留附带错误。
- 方向名额必须同时被 record（覆盖本地工作）和 native closed 监督持有。ConnectionLifetime 保持唯一 PeerState 及 semaphore，不缓存原生连接；建立中、取消后迟到连接和关闭未完成均占名额。关闭 JoinSet 须收集旧拨号晚登记的新结束任务，watch 必须持原 session token。
- Native Blob profile 已取消内部 staggered retry，每轮只有一次 Endpoint.connect；不能只数 Uni backend.connect 调用而漏算底层自重试。Blob grant 的 can_retain 读取当前权威，其他 profile 不因此开放保留。
- 尚未实现：生产 Blob 提供端 4／2 即时拒绝、准入和首请求头共用 60 秒（不能在 SDK provide 重建期限）、独立完整 owner／输出和引用保护、非阻塞进度与 5 秒未完成、稳定重试／绑定和最终矩阵。生产旧 Downloader、warmup/provider/cancel 路径仍存在，资源策略的真实 Uni 探针通过也不能冒充生产接线已完成。
