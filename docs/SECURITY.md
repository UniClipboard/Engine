# 安全架构

安全问题的私密报告方式和受支持版本见仓库根目录 [`SECURITY.md`](../SECURITY.md)。本文件是工程安全知识入口。

## 不可破坏的边界

- SQLite、磁盘缓存和搜索索引中的业务负载默认先经 MasterKey AEAD 加密。
- 可明文保存的只有内容类型枚举、文件内容本体，以及入站文件在受管缓存中经安全清理且仅作实际 basename 的原始文件名。
- 原始目录路径、数据库/搜索字段、日志和其他关联元数据仍须加密或脱敏。
- 新增持久字段或文件默认按敏感数据处理；明文例外必须在 PR 说明并取得明确批准。
- 日志、公开错误和观测事件不得包含内容、密码、密钥、完整令牌、邀请、设备名、地址、文件名或路径。
- 网络使用认证加密通道；业务授权仍需核对当前成员范围，不能把认证身份等同于权限。

## 深入阅读

- [密文持久化规则](security/encrypted-persistence.md)
- [发布完整性](security/release-integrity.md)
- [不可变内容保护上下文实施记录](exec-plans/completed/033-immutable-content-protection-context.md)
- [错误处理与安全上下文](design-docs/error-handling.md)

## 交付检查

持久化变更必须覆盖创建、升级、回退与失败恢复，执行明文探针和相关真实存储测试。发布资产必须来自
同一提交与版本，并由 `release-manifest.json` 记录大小、SHA-256 和设备矩阵状态。
