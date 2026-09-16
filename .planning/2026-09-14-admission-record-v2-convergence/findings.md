# Findings: Admission Record V2 Convergence

## Baseline
- 分支起点 `255e1a68` 只有 admission aggregate V1。
- 当前分支实际已继续演进到 V6；这些格式都尚未发布，不属于外部兼容合同。
- 收敛只针对 Core admission aggregate 的内部记录版本；其他仓储 envelope、成员历史和恢复索引各自的版本不在本切片范围。
- 最终 V2 包含统一五分钟时间线、可选认证尝试摘要，以及确认、加入方终止、双方撤销处理、邀请方到期和本机空间退出计划。
- 现有 Infra repository V3 是早于本分支存在的外层加密仓储格式，不是本次 admission aggregate 的分支过渡版本，必须保留。
- V1 状态枚举末尾新增的消息类型不会改变旧变体编码；旧 pending exchange 缺少尾部字段的真实布局继续由严格无尾随兼容读取覆盖。
