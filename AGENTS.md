# UniClipboardEngine 维护地图

本文件只提供进入仓库所需的硬约束和导航。长期知识位于 `docs/`，当前架构从
[`ARCHITECTURE.md`](ARCHITECTURE.md) 进入，文档总索引见 [`docs/README.md`](docs/README.md)。

## 先读

- [核心信念](docs/design-docs/core-beliefs.md)
- [工程与模块设计原则](docs/design-docs/engineering-principles.md)
- [错误处理与转换](docs/design-docs/error-handling.md)
- [运行期观测装配](docs/design-docs/observability.md)
- [安全架构](docs/SECURITY.md)
- [执行计划与技术债](docs/PLANS.md)

进入具体目录后继续读取最近的局部 `AGENTS.md`；它们同样只作范围地图，细则链接回 `docs/`。

## 不可破坏的规则

- **持久化默认密文**：写入 SQLite、磁盘缓存或搜索索引的业务负载默认先经 MasterKey AEAD 加密，严禁明文落库。
- 仅内容类型枚举、文件内容本体，以及入站文件在受管缓存中经安全清理且只作实际 basename 的原始文件名可明文保存；原始路径、数据库/搜索字段、日志与关联元数据仍须加密或脱敏。
- 新持久字段或文件默认敏感；主张明文例外必须在 PR 说明并取得明确批准。
- 日志、公开错误和观测事件不得包含剪贴板内容、密码、密钥、完整令牌、邀请、设备名、地址、文件名或路径。
- 核心问题只在本仓修复，产品仓不维护补丁副本。
- `uc-engine` 是唯一稳定 Rust 入口；iOS、Android、HarmonyOS 绑定只依赖它并使用同一版本。
- P2P 是默认能力；LAN 兼容线只由用户明确选择，不因 P2P 失败自动切换。
- 内部 crate 与绑定不发布到 crates.io；交付只通过带校验信息的 GitHub Release。

## 目录地图

- `crates/`：Core 规则、Application 流程、Infra 实现与 Engine 稳定入口。
- `bindings/`：iOS、Android、HarmonyOS 薄绑定。
- `compatibility/`：独立版本、独立发布的 LAN 兼容线。
- `tests/hosts/`：移动平台验收宿主，不承载产品功能。
- `scripts/architecture/`：所有权、依赖方向和发布来源检查。
- `scripts/release/`：产物归集、清单与发布前核验。
- `docs/design-docs/`：长期设计、稳定契约与 ADR。
- `docs/exec-plans/`：active/completed 计划与技术债。
- `docs/product-specs/`：产品需求与验收。
- `docs/generated/`：可再生的 schema 与图表快照。
- `docs/references/`：领域词表与来源映射。

## 修改约束

- 项目文档与代码注释使用中文；代码标识符、提交信息使用英文。
- 保持单一事实来源，不长期保留新旧两套实现或文档入口。
- 文档中的仓库路径使用相对路径。
- Rust 命令从仓库根目录运行。
- 生产代码禁止 `unwrap()`、`expect()`、`println!()` 和 `eprintln!()`。
- Application 下层失败保留完整 source chain；禁止字符串化或吞错。详细规则见错误处理文档。
- 跨层功能必须有唯一完整负责人；Core 保存规则、Application 负责流程、Infra 提供能力、Engine 只组装。
- 跨层持续计时与结果分类只通过 Engine 组装层的领域 port decorator 实现。
- 新功能开工前写清完整负责人、调用方唯一动作、成功/失败结果及重启/重试责任。
- 任何 Agent 修改仓库内容时，同步检查并更新 `docs/architecture/architecture-bible.md`；无架构变化也在“文档维护记录”增加记录。

## 交付前检查

不涉及行为改动时至少运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

涉及发布时另运行：

```bash
node scripts/release/verify-release-bundle.mjs <产物目录>
```

设备矩阵中未执行的项目记为“跳过”，不得记为“通过”。
