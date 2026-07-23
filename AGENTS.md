# AGENTS.md

本文件是 `UniClipboardCore` 的维护入口。

## 不可破坏的规则

- **持久化默认密文**：任何写入 SQLite、磁盘缓存或搜索索引的业务负载，默认必须先经 MasterKey AEAD 加密，严禁明文落库。剪贴板正文、标题、预览、搜索渲染字段、标签名、文件名和文件路径均在此列。
- 仅内容类型分类枚举和文件内容本体可以例外。文件内容可以由 blob store 或核心导入目录按原始字节保存，但文件名、路径和关联元数据仍须加密。
- 新增持久化字段或文件时默认按敏感数据处理。若主张明文保存，必须在 PR 中说明理由并获得明确批准。
- 核心问题必须在本仓修复，产品仓不得维护补丁副本。
- `uc-engine` 是唯一稳定的 Rust 入口；外部使用方不得直接依赖内部 crate。
- iOS、Android 和 HarmonyOS 绑定只依赖 `uc-engine`，并与其使用同一版本。
- P2P 是默认能力。LAN 兼容线必须由用户明确选择，不得因 P2P 失败自动切换。
- 内部 crate 和绑定均不发布到 crates.io；交付只通过带校验信息的 GitHub Release。

## 目录归属

- `crates/`：核心领域、应用编排、基础设施和稳定入口。
- `bindings/`：iOS、Android 与 HarmonyOS 的薄绑定。
- `compatibility/`：独立版本和独立发布的 LAN 兼容线。
- `tests/hosts/`：移动平台验收宿主，不承载产品功能。
- `scripts/architecture/`：仓库所有权、依赖方向和发布来源检查。
- `scripts/release/`：产物归集、清单生成和发布前核验。

## 修改规则

- 项目文档使用中文；代码标识符、代码注释、提交信息使用英文。
- 保持单一事实来源，不长期保留新旧两套实现。
- 文档中的仓库路径使用相对路径。
- Rust 命令从仓库根目录运行。
- 生产代码禁止 `unwrap()`、`expect()`、`println!()` 和 `eprintln!()`。
- 日志不得包含剪贴板内容、密码、密钥、完整令牌、文件名或文件路径。

## 交付前检查

不涉及行为改动时，至少运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-core-repository.mjs
git diff --check
```

涉及发布时，还必须运行：

```bash
node scripts/release/verify-release-bundle.mjs <产物目录>
```

设备矩阵中未执行的项目必须记为“跳过”，不得记为“通过”。
