# 已关闭的执行计划

这里保留已完成、已由后续方案取代或仅供历史追溯的实施记录。它们不自动代表当前架构事实。

- [资料密钥丢失恢复与旧设备迁移](2026-09-19-profile-key-recovery.md)（Engine 实现与本地验收完成；Desktop、实体平台和旧版本程序回退验收跳过）

- [会话恢复后的成员资料立即就绪](2026-09-18-session-membership-readiness-recovery.md)（本机正式版冷启动中，会话恢复约 0.12 秒完成，成员列表约 0.66 秒可读）

- [Space 可用性启动门槛](2026-09-17-space-readiness-startup-gate.md)（本机正式版启动与设备名单验收完成；多设备全离线的独立进程场景未构造）

- [配对到期、本机终止与新意图替换](2026-09-13-admission-expiry-and-replacement.md)（S0–S8 完成；真实设备、跨真实外网和产品前台验收跳过）

- [已有连接活性与故障恢复](2026-09-12-connection-liveness-and-recovery.md)（实现与本地三轮完整验收完成；远程 CI、发布和产品采用未执行）

- [已配对设备自动连接与恢复](2026-09-11-automatic-peer-connections.md)（实现与 Mac/iOS 模拟器验收完成；其他实体平台跳过）

- [040 以完整业务动作组织观测记录](040-business-observability-records.md)（本机验证完成，产品宿主与真实后端未验收）

- [043 单设备修改加密口令](043-single-device-passphrase-change.md)（Engine 与移动绑定完成；实体设备和产品界面验收跳过）

- [015 离线优先成员移除](015-offline-first-member-removal.md)（部分由 ADR-020 取代）
- [016 工作空间全局收敛](016-workspace-wide-convergence.md)（部分由 ADR-020 取代）
- [017 配对作为工作空间准入](017-pairing-as-workspace-admission.md)（wire/runtime 由 028 取代）
- [018 Application 按业务领域收口](018-domain-oriented-application-layout.md)（剩余实施由 031 取代）
- [023 可持续验证的成员历史与准入激活](023-durable-membership-proof-and-admission-activation.md)
- [024 成员收敛内部职责边界](024-workspace-convergence-internal-boundaries.md)（由 027 取代）
- [025 用户明确加入安全取代](025-user-initiated-join-supersession.md)
- [026 旧资料独立化与重新配对](026-legacy-profile-isolation-and-re-pairing.md)
- [027 Application Space 一次性重写](027-application-space-membership-one-shot-rewrite.md)
- [028 单一 Space 准入协议](028-single-space-admission-protocol.md)
- [029 持久化成员历史反熵](029-durable-membership-history-anti-entropy.md)
- [030 成员分叉选择与复杂拓扑验证](030-membership-conflict-resolution-and-chaos-validation.md)
- [031 Application 依赖表面深化](031-application-dependency-surface-deepening.md)
- [032 退役 Legacy Space Transition](032-admission-space-transition-internal-refactor.md)
- [033 不可变内容保护上下文与一次性密文升级](033-immutable-content-protection-context.md)
- [035 Space 观测装配 interface 收敛与仓库推广准则](035-space-domain-observability-assembly.md)
- [036 关键模块深化与退役路径 clean cutover](036-architecture-deepening-clean-cutovers.md)
- [037 OpenTelemetry tracing 与结构化日志 clean cutover](037-opentelemetry-tracing-and-structured-logs.md)
- [移动端日志文件层与连接中继日志](mobile-log-file-layer.md)（保留、导出和远程发送由 037 取代）
- [Profile 内容密钥运行期复用](profile-content-key-runtime-reuse.md)（实现完成；含已知全量测试失败与 GUI 验收跳过记录）
