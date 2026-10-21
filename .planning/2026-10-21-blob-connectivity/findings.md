# 发现

- 起始分支 feat/active-clipboard-connectivity，HEAD 2154b959；保留原有规划目录和未提交文档。
- 已 fetch origin/main，冻结基线 a461b894；新分支 feat/blob-connectivity 从该基线创建。
- spec 明确阶段 0 任一技术证明失败／无法证明即设计阻塞，不允许提前替换生产路径。
- implement 要求最后 code-review；当前技能目录未发现该技能，继续查找；如缺失执行等效差异审查并如实记录。

- 停止前置失败已复现：固定 SDK 网络错误返回时，受测试门阻塞的同次 import 仍活跃。源码对应 remote.rs get_blob_ranges_impl 的 try_join 提前丢弃私有 handle.rx。
- 全局 Store::wait_idle 不是单项 join，SDK 文档明确并发时可能永不空闲。
- 不宣称所有低层 SDK 组合不可行；可选 fork 完整 handle 或 Infra 持有 get::fsm／ImportBaoHandle 两份完成权，provider export 也须证明。
- /code-review 在可访问技能目录不存在；本轮执行手工调用图与差异审查，不假称调用该技能。
