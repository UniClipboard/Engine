# Space 模块出口进度

## 2026-08-26

- 明确完成标准：先建立 `space/mod.rs` 唯一出口，不在本次整理内部业务目录。
- 读取当前公开入口、SpaceFacade、deps 和跨模块深层引用。
- 确认本次为内部结构调整，产品行为和公开调用必须保持不变。
- 在仓库架构检查中加入 Space 出口规则。
- 首次运行规则失败，失败原因与目标问题一致，完成测试先行的失败阶段。
- 将 SpaceFacade 的实现、输入输出、错误和依赖定义迁入 `space/facade`。
- 将旧 `facade/space_setup` 收成公开白名单，不保留第二份实现。
- 将 Space 子模块全部改为私有，并在 `space/mod.rs` 建立调用方与组装层出口。
- 将 deps、AppFacade、settings 和 clipboard 改为只依赖 Space 根出口。
- 新增结构规则由失败转为通过，完整仓库架构检查通过。
- 首次 application 编译发现一个根出口漏项和三个测试专用引用警告；补齐后重新编译通过。
- 将旧 SpaceFacade 实现文件加入永久退役清单，防止实现回流到公开目录。
- 更新 Space 维护地图和架构圣经，记录唯一根出口和两份公开白名单。
- 新增 crate 外部视角的 Space 公开路径测试，1 项通过。
- Space 聚焦测试 113 项全部通过。
- application 全部单元测试 657 项全部通过。
- `cargo metadata --locked --format-version 1` 通过。
- `cargo fmt -p uc-application -- --check` 通过。
- `node scripts/architecture/check-engine-repository.mjs` 通过。
- `git diff --check` 通过。
- 最终差异审计确认 SpaceFacade 实现只改变所属位置和内部引用，业务方法正文未变化；deps、commands 和 errors 除归属说明外未改行为。
- `cargo check --workspace --all-targets --locked` 未通过：Infra 仍引用 application 已删除的旧接口，并有既有 admission delivery error 构造不匹配；错误与本次出口收口前一致。
- `cargo fmt --all -- --check` 未通过：仅报告 `crates/uc-engine/src/assembly/sync_engine.rs` 和 `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs` 的既有格式差异。
