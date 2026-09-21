# 进行中的执行计划

这里包含设计中、实施中、待实现或暂时阻塞的工作。状态以各文件开头为准。

- [034 确定性虚拟 Peer Network 测试套件](034-deterministic-virtual-peer-network-test-suite.md)
- [038 双设备配对本机耗时压缩到一秒](038-pairing-local-latency-budget.md)（实施中）
- [039 历史本地调试记录逐项收口](039-local-debug-inventory-cleanup.md)
- [041 可导出的连接故障调查记录](041-exportable-connection-diagnostics.md)（含 Engine、手机、桌面宿主及联合验收，进度见正文）
- [043 升级前备份与软件版本回退](043-pre-upgrade-profile-backup.md)（实施中：启动前备份及恢复出厂清理已接入，完整版本回退尚未完成）
- [043 Engine 网络运行期与 Space 会话安全交接](043-engine-network-runtime-and-space-session-handover.md)（实施中，对应 Issue #68）
- [历史可读时的邀请失败](2026-09-12-invitation-admission-recovery.md)
- [本地产物准备](local-artifacts-preparation.md)
- [配对通信等待诊断](2026-09-12-admission-exchange-diagnostics.md)
- [配对内部处理与排队诊断](2026-09-13-pairing-local-work-diagnostics.md)
- [Engine 空闲重复工作与准入存储性能修复](2026-09-13-engine-idle-work-and-admission-performance.md)
- [已知设备联系驱动的成员恢复与地址更新](2026-09-17-known-peer-contact-recovery.md)（实施中）
- [只走中转时三秒内恢复双向连接](2026-09-18-relay-only-three-second-recovery.md)（核心实施与二十轮验收完成；现有旧版配对基线阻塞完整套件收口）
- [本地就绪与 Space 后台恢复统一责任](2026-09-19-local-readiness-and-background-space-recovery.md)（核心实施完成，实体设备验收待执行）
- [大图发送前本地准备性能与诊断](2026-09-18-large-image-publish-latency.md)（实施中）
- [统一暂停、恢复与中断恢复](2026-09-12-unified-runtime-lifecycle.md)（实施中，剩余设备与产品宿主验收未完成；包含已有后台问题修复记录入口）

计划完成时先更新稳定设计/ADR 和验收证据，再移入 `../completed/`。
