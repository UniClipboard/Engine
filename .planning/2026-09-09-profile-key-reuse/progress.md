# 进度

- 研究和设计交付完成，无生产代码变更。
- 正式提案已加入 active index，architecture-bible 只记录提案，不宣称实施完成。
- 用户新增 GUI 锁定要求已纳入；后台密码能力继续有效，交互访问由宿主完整授权模块负责。
- cargo metadata --locked --format-version 1：通过。
- cargo check --workspace --all-targets --locked：失败，既有 compatibility/uc-mobile-lan/src/usecases/authenticate_request.rs:235 缺少 tracing_subscriber 测试依赖，完整输出在 cargo-check.log；本次未修改无关代码。
- cargo fmt --all -- --check：通过。
- node scripts/architecture/check-engine-repository.mjs：通过。
- git diff --check：通过。
- 本次修改的三份 docs Markdown 相对链接目标检查：通过。
- 实现测试、新性能测量、真实 GUI/macOS/移动端：跳过，本轮仅设计。

## 实施进度

- 已建立真实 ContentProtection 计数红灯：20 次 open + search_catalog 原先产生 60 次 get，实施后为 1 次冷加载。
- 已实现 vault 有界读视图、搜索根复用、单实例/进程文件租约、安装发布及失败/取消失效。
- 已实现活动会话许可、clear/close 与回滚代次检查、维护 session 隔离；GUI 未新增锁定状态，后台能力保持原有运行语义。
- 11 项活动安全会话测试通过；全 Infra 首轮 802 通过、1 失败、4 跳过，失败来自冷重启夹具保留旧会话，已修正夹具。
- workspace all-targets check 已通过；LAN 测试依赖已补齐。
- 全 workspace 测试进行中；随后重跑自查新增的故障/取消和搜索测量测试。

- 全 workspace 两轮停止于既有 dev-tools 重发断言：期望 SynchronizationDisabled，实际 NoEligibleTargets；尝试显式目标后为 PayloadLost，确认现有 Application 没有构造 SynchronizationDisabled 的代码。已完全撤回该测试改动；HEAD 搜索证据保存在 baseline-resend-evidence.log。
- 第一次独立重跑未启用 dev-tools，实际运行 0 项，不计为通过。
- 正在完整执行 workspace 并仅跳过该已确认失败测试；不把全量原命令记为通过。
- 最后自查修复开启复用与临时读取交错的代次窗口，并添加对应并发回归；不新增 GUI 模拟 boolean 测试，真实 GUI 尚无实现，交互验收明确跳过。
- 默认并行的 space_membership_auto_pairing_e2e 进程持续占满约 8 核、运行约 9 分钟，F2/F4/F5/F6 报告失败且整组未退出；已停止该测试进程（不是用户 daemon）。这轮记为失败/中断，不能当作通过。
- 正在以 dev-tools + --exact + --test-threads=1 单独复跑 F5，确认是否由并行资源竞争引起；在确认前不替换实际 daemon。
- 自查发现 rebind 原先先复制 MasterKey 再写 session，可能跨 clear 复活密钥；已改为同一 session 临界区的完整 rebind 操作，并加 1000 次并发 clear/rebind 回归。
- 已只读确认 Desktop Cargo.toml 的本地 Engine 覆盖、dev daemon 环境及构建入口。后续计划在验证后用 cargo build --manifest-path ../desktop/Cargo.toml -p uc-daemon --bin uniclipd --locked 构建，保留 Desktop 现有 Cargo 修改；尚未构建或重启。

- 最终补跑排除 Engine 的 workspace 全部通过：2851 passed，7 ignored；Infra 808 passed，4 ignored。单独串行 F5 通过，未据此宣称 F2/F4/F6 已修复。设计已归档至 completed，稳定持久化文档与架构圣经同步更新；保留实际运行的 dev daemon。
