# `uc-infra-profile` 维护地图

完整规范见 [`docs/design-docs/layers/infrastructure.md`](../../docs/design-docs/layers/infrastructure.md)，安全边界见
[`docs/SECURITY.md`](../../docs/SECURITY.md)，可靠性入口见 [`docs/RELIABILITY.md`](../../docs/RELIABILITY.md)。

## 范围

- Profile 持久布局升级、备份/恢复/启动与跨存储的完整安全激活能力：`config_migration/`、
  `security/profile_*`、`security/v3_*`、`security/space_control_generation/`、
  `security/space_transition_activation/`、Space 准入准备/激活与 membership record。
- 依赖 `uc-infra-local`/`uc-infra-crypto`/`uc-infra-security`/`uc-infra-storage`/
  `uc-infra-content`/`uc-infra-p2p`，组合出完整的 Profile 技术能力。
- 不定义业务真相、用户流程、UI/API 表示或 composition root。

## 硬约束

- 具体库类型与格式向下收敛，不泄露到上层契约。
- 持久化业务负载默认先经 MasterKey AEAD，加密迁移不得落明文或靠删除数据恢复。
- Adapter 不扩展 port 语义，不补造业务默认值，不静默吞错。
- 长期后台任务必须可关闭且失败可见；禁止丢弃 `JoinHandle` 的永久循环。
- 关键 adapter 测试覆盖真实存储/协议、损坏数据、边界值和恢复路径。
- 日志只含批准的稳定分类、计数和长度，不含业务、身份、凭据、文件名或路径。

交付前运行相关真实 Infra 测试、workspace check、fmt、架构检查与 diff check；架构事实变化时同步对应主题文档。
