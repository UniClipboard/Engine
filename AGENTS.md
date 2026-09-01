# AGENTS.md

本文件是 `UniClipboardEngine` 的维护入口。

## 不可破坏的规则

- **持久化默认密文**：任何写入 SQLite、磁盘缓存或搜索索引的业务负载，默认必须先经 MasterKey AEAD 加密，严禁明文落库。剪贴板正文、标题、预览、搜索渲染字段、标签名、文件名和文件路径均在此列，但下述受管文件缓存例外除外。
- 允许明文保存内容类型分类枚举、文件内容本体，以及入站文件在受管文件缓存中的经安全清理的原始文件名。该文件名只能作为实际缓存文件的 basename；原始目录路径、数据库/搜索字段、日志和其他关联元数据仍须加密或脱敏。
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

- 项目文档和代码注释使用中文；代码标识符、提交信息使用英文。
- 任何 Agent 修改仓库内容时，必须同步检查并更新 `docs/architecture/architecture-bible.md`。架构语义变化必须修改对应正文；确认无架构变化时也必须在“文档维护记录”中增加本次修改记录。未更新不得交付。
- 保持单一事实来源，不长期保留新旧两套实现。
- 文档中的仓库路径使用相对路径。
- Rust 命令从仓库根目录运行。
- 生产代码禁止 `unwrap()`、`expect()`、`println!()` 和 `eprintln!()`。
- 日志不得包含剪贴板内容、密码、密钥、完整令牌、文件名或文件路径。

### 异常处理与转换

- Application 对依赖、存储、网络、系统或密码能力失败进行稳定分类时，错误 variant 必须使用 `#[source] source: anyhow::Error`，或携带另一个实现 `std::error::Error` 的具体 source。构造方式遵循 `crates/uc-application/src/error.rs`；不得为了 `Clone`、`Copy`、`Eq` 或简化匹配而丢弃 source。
- 错误向上传播优先实现 `From<LowerError>` 并使用 `?`。转换实现归目标错误所在模块所有，来源错误模块不得反向依赖上层错误。
- 只有需要改变语义分类或补充安全上下文时才允许使用 `map_err`。转换后仍必须把原错误作为 source；禁止 `map_err(|error| Error::X(error.to_string()))`、`map_err(|_| Error::X)`、无来源 unit variant，以及其他字符串化或吞掉原错误的写法。
- 使用 `anyhow::Context` 或构造 source 时只能增加固定、脱敏的动作上下文，不得写入剪贴板内容、密码、密钥、令牌、文件名、文件路径、设备名或其他敏感负载。
- 纯业务判断在没有任何下层异常时可以使用普通枚举或明确结果；不得伪造 `anyhow::Error`。一旦错误来自被调用能力，就必须保留完整 source chain 和 backtrace。
- 新增或修改错误转换时，测试必须至少验证稳定分类和 `source()` 非空；只断言显示文本不算完成。

### 防止复杂度外泄

- 禁止把同一功能的判断规则、流程推进、通信、持久化、失败恢复、后台重试和启动接线同时暴露给调用方或评审者。内部实现可以复杂，但使用者必须只需理解一个主要入口、必要输入和少量明确结果。
- 跨层功能必须先指定一个唯一负责完整流程的模块。`uc-core` 保存业务规则，`uc-application` 负责流程，`uc-infra` 提供具体能力，`uc-engine` 只负责组装；不得让多个层分别掌握一段流程，再依赖调用顺序拼成完整行为。
- 不得为每个内部步骤创建一一对应的公共接口并由上层逐步编排。接口如果接近实现本身的复杂度，或测试必须了解并手工拼装内部步骤，必须暂停扩展并先重新设计。
- 新功能开工前必须写清楚：谁负责完整结果、调用方唯一需要执行什么、成功和失败分别返回什么、重启或重试由谁负责。回答不清楚时不得进入实现。
- 评审跨层改动时必须做“删除检查”：设想删除负责该功能的模块；如果复杂度只是重新散落到多个调用方，说明模块真正隐藏了复杂度；如果删除后几乎没有变化，说明它只是转发层，应当合并或重新划分职责。
- 文件多不是问题，知识分散才是问题。一个行为即使需要修改多个层，也必须能从负责模块的入口和测试读懂；不得要求维护者同时追踪多个文件才能还原基本流程。

### 运行期观测装配范式

- 跨层业务链路的持续计时、结果分类和阶段诊断必须通过 `uc-engine` 组装层的 port decorator 实现；不得在 `uc-application` 业务调用点散布 `Instant`、`tracing` 或手工阶段记录函数。
- Engine 私有 decorator 统一放在 `crates/uc-engine/src/assembly/observability/<domain>.rs`，领域规模增长后可拆为同名子目录；每个业务领域拥有自己的具体 `ObservedX`、私有操作枚举、明确 observation policy 和结构化事件 schema。
- 每个领域必须提供一个主要装配入口，将真实 port 集中包装后交给 Application。返回另一个 port 的能力也必须继续包装返回值，例如 transport 建链成功后包装 authenticated exchange，不能只观察外层调用。
- 允许复用的是“在 Engine 组装边界装饰 port”的结构，不得创建跨领域的 `Observed<T>`、万能 `record_performance_phase()`、字符串驱动 phase 注册表，或由调用方传入开始时间、成功布尔值和可选字段的通用埋点接口。
- decorator 只能观察依赖调用，不得改变业务结果、错误 source、重试、持久化或通信顺序；观测失败不得影响业务结果。
- operation、outcome 和分类字段必须来自固定枚举或固定映射。日志只能包含经批准的稳定分类、计数和耗时；错误文本、业务负载、邀请、设备名、地址、凭据、密钥、令牌、文件名和路径不得进入观测事件。
- 新增或修改 decorator 时，测试至少覆盖成功与失败的 policy 决策、适用的空结果降噪策略，以及返回 port 的继续包装；同时验证 Application 调用点未重新引入计时或 tracing。

## 交付前检查

不涉及行为改动时，至少运行：

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

涉及发布时，还必须运行：

```bash
node scripts/release/verify-release-bundle.mjs <产物目录>
```

设备矩阵中未执行的项目必须记为“跳过”，不得记为“通过”。
