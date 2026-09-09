# 已确认事实

- 基线 `af407167` 已合并旧存储升级修复；主线已 fetch，当前独立分支 `feat/startup-upgrade-progress`。
- Engine 返回前，runtime -> host wiring -> ensure_profile_storage_v3 -> ensure_v3 已完成整个升级。
- Primary 转换已有完整行列表，可真实统计 inline representation 与 blob；不可读 blob 保留原密文并标记 Lost。
- Derived 转换对不可读文件清单、搜索预览保留加密恢复快照；警告不能在校验或重启时重复累加。
- 新 profile 也走准备步骤；已有 V3 快速检查不能宣称执行了资料升级。
- 现有 startup error 会压成 1101；升级失败需在 owner 处产生安全摘要，同时保持原始内部错误返回。
- 本任务不自行启动多个 Agent，不修改 Desktop 或 CI。
