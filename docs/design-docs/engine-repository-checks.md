# Engine 仓库检查

仓库通过一个入口检查所有权和发布约束：

```bash
node scripts/architecture/check-engine-repository.mjs
```

检查内容包括：

- 新增 Rust 代码遵守 [Rust 编写规范](rust-style.md)；本地比较当前工作区，CI 比较本次变更的基线，不追溯阻断历史写法。
- 工作区只能包含本仓拥有的 Engine、内部实现、绑定、兼容和验收包。
- 本地路径依赖不能指向仓库外，也不能依赖 desktop、daemon、CLI 或 Tauri 包。
- `uc-engine` 是唯一稳定的 Rust 入口；移动绑定只依赖该入口。
- 所有包均禁止发布到 crates.io。
- UniFFI、HarmonyOS 绑定和 `uc-engine` 必须使用同一版本。
- 三端打包脚本必须记录版本、来源提交和校验值。
- 密文扫描器必须接受干净目录、拒绝含探针明文的目录，并且不能输出探针内容。
- 依赖锁文件不得含 RustSec 漏洞或 unsound 公告：PR Check 用固定版本的 `cargo audit --deny unsound` 检查。当前锁不含任何例外；新公告必须通过依赖升级修复，不得以忽略清单放行。
- 生产与测试代码不得直接使用 `tracing` 日志宏：PR Check 运行 `node scripts/architecture/check-direct-log-macros.mjs`，用 clippy 的 `disallowed_macros`（默认特性与 `lan-compat` 各一轮）检查，任何一处都失败；日志一律使用 `uc_*!` 宏，字段目录与值类别在编译期约束。它需要 clippy，耗时约两分钟，因此不在上述入口内，也不属于每次本地交付检查；改动日志调用点时再运行。决策见 [ADR-030](decisions/030-typed-log-events-and-enforcement.md)。
- LAN 兼容能力默认关闭，P2P 使用方不得隐式启用，也不得出现自动回退逻辑。

检查程序自带三个隔离的错误样例，分别模拟仓库外本地依赖、绑定版本不一致和自动 LAN 回退。每次执行都必须证明三个错误会被拒绝。

消费者是否绕过 `uc-engine`、是否固定不可变提交或完整 Release，由各产品仓继续检查。本仓不读取产品仓文件。
