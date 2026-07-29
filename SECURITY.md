# 安全说明

UniClipboardEngine 负责端到端加密、设备身份、P2P 协议和本地密文持久化。未公开修复的安全问题请不要提交公开 Issue。

## 报告方式

请在本仓库的 GitHub `Security` 页面选择 `Report a vulnerability`，通过私密安全报告提交复现条件、影响范围和已知缓解办法。

## 支持范围

项目仍处于 1.0 之前，只维护最新的 `v*` 发布线。安全修复通过新版本发布，不覆盖已有标签或资产。

## 发布校验

每次发布都包含 `release-manifest.json`。该文件记录来源提交、锁文件校验值、每个资产的大小和 SHA-256，以及已执行或明确跳过的设备验收。使用方必须同时校验清单和目标资产，不能只依赖文件名或可变分支。

持久化和发布安全的详细规则见：

- `docs/security/encrypted-persistence.md`
- `docs/security/release-integrity.md`
