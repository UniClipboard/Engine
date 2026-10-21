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
