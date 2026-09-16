---
name: uni-security-contract-audit
description: "独立审查 UniClipboardEngine 的密文持久化、敏感信息边界、密钥使用、稳定 uc-engine 入口、绑定、网络降级和发布完整性契约。用于定时安全与契约审查，或存储、日志、公开 API、绑定和发布变更审查；不用于实现修复，也不调用其他 skill。"
---

# Uni 安全与契约审查

## 目的

确认敏感业务数据从入口到持久化、索引、日志、公开错误和发布产物均遵守仓库硬约束，并检查唯一稳定入口与交付来源没有被绕开。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟其他审查 skill。
- 默认只读；除调用方明确指定的报告输出路径外，不修改仓库、Issue 或 PR。
- 开始前完整读取共享的 [finding contract](../uni-audit-report/references/finding-contract.md)。
- 不在报告、命令输出摘要或证据中复制任何真实敏感值。

## 事实来源

先读取根目录和目标目录最近的 `AGENTS.md`，再读取：

- `docs/SECURITY.md`
- `docs/security/encrypted-persistence.md`
- `docs/security/release-integrity.md`
- `docs/design-docs/uc-engine-interface.md`
- `ARCHITECTURE.md`
- `docs/design-docs/error-handling.md`
- `docs/PLANS.md`，仅用于识别已声明技术债

## 工作流

1. 按共享契约确定范围并记录基线。
2. 对新增或变更的数据字段先分类；没有明确依据时默认敏感。
3. 追踪敏感数据从入站到 SQLite、磁盘缓存、搜索索引、日志、观测事件、公开错误和 FFI 的完整数据流。
4. 检查：
   - 业务负载是否在持久化前经 MasterKey AEAD 加密，nonce、关联数据和密钥标识是否符合现有契约；
   - 明文例外是否严格限于已批准类型，原始路径和关联元数据是否仍加密或脱敏；
   - 缓存清理、删除、替换、崩溃恢复和索引重建是否不会留下非预期明文；
   - 日志、公开错误和事件是否可能包含内容、密码、密钥、完整令牌、邀请、设备名、地址、文件名或路径；
   - 密钥材料是否跨越不必要边界，序列化、Debug、克隆和生命周期是否受控；
   - `uc-engine` 是否仍是唯一稳定 Rust 入口，移动绑定是否薄且只依赖同一版本；
   - P2P 失败是否会未经用户明确选择自动降级到 LAN；
   - 内部 crate/绑定是否避免发布到 crates.io，GitHub Release 是否保留校验与来源约束。
5. 正则、字段名或序列化调用只能定位候选项。确认 finding 前必须证明敏感数据可到达实际汇点或契约确实被绕过。
6. 对 release/binding 契约运行相应架构或发布静态检查；没有产物时发布 bundle 验证必须记为 `skipped`，不得记为通过。
7. 已有明确计划和 owner 的同一问题标为 `existing_declared_debt`。
8. 按共享契约输出本 lane JSON；未指定路径时输出等价 Markdown。

## 建议检索

使用 `rg` 检查 schema、SQL、索引、文件写入、serde、AEAD、key、日志/事件、公开错误、绑定 `Cargo.toml` 和发布脚本。查看测试 fixture 时同样不得把可能的秘密或真实内容复制到报告。

## 完成条件

- 每条安全 finding 有清晰的数据来源、变换、汇点和违反的硬约束。
- 每条契约 finding 有实际依赖或发布路径证据。
- 没有把名字可疑或缺少关键词单独当作漏洞。
- 报告已脱敏，且设备/发布矩阵中的未执行项均为 `skipped`。
