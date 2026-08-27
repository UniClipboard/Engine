# Space 内部结构整理进度

## 2026-08-26

- 明确四个责任区和不改变行为/公开出口的完成标准。
- 确认工作只发生在 Space 内部，上一阶段的根出口作为固定保护层。
- 扩展仓库结构规则，固定目标目录和旧路径退役清单。
- 首次运行按预期失败，完成结构整理的测试先行失败阶段。
- 将 11 个生命周期目录迁入 `lifecycle/`，建立责任区出口并更新全部引用。
- 将网络恢复迁入 `connectivity/recovery/`，保持单一实现。
- 生命周期和连接恢复整理后 application 全目标编译通过。
- 将 10 个成员目录迁入 `membership/`，建立责任区出口并清除 113 处旧路径引用。
- 收紧 admission 子模块，Facade、lifecycle 和 application 改为只依赖 admission 根出口。
- 四个责任区整理后 application 全目标编译通过，没有新增编译警告。
- 扩展仓库规则：各责任区不得公开子模块，责任区外不得穿透内部目录。
- 更新 Space 维护地图、架构圣经和规格 027 的全部旧路径与归属说明。
- Space 聚焦测试 113 项全部通过。
- application 全部单元测试 657 项全部通过。
- crate 外部公开路径测试 1 项通过，既有 facade/deps 调用仍可访问。
- `cargo metadata --locked --format-version 1` 通过。
- `cargo check -p uc-application --all-targets --locked` 通过。
- `cargo fmt -p uc-application -- --check` 通过。
- `node scripts/architecture/check-engine-repository.mjs` 通过。
- `git diff --check` 通过。
- 30 个 case 与维护地图逐项一致，维护地图内全部仓库路径存在。
- `cargo check --workspace --all-targets --locked` 未通过：Infra 仍引用 application 已删除的旧接口，并有既有 admission delivery error 构造不匹配；错误与整理前一致。
- `cargo fmt --all -- --check` 未通过：仅报告 `crates/uc-engine/src/assembly/sync_engine.rs` 和 `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs` 的既有格式差异。
