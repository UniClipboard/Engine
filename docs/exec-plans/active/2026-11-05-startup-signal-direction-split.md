# 启动信号方向拆分：progress 作为输出，lifecycle 作为输入

状态：提议（Proposed）；未实施。

基线：`68bfa3abbf3c66371b05fdb0e45d06c49809349c`（`refactor(engine)!: collapse the startup entry points into Engine::start`）。
该提交只合并了入口层数，没有改变本文要处理的方向问题。

关联文档：[Engine 接口](../design-docs/uc-engine-interface.md)、[Engine 资源释放契约](../design-docs/engine-resource-release.md)、
[ADR-035](../design-docs/decisions/035-recoverable-process-and-startup-release-owners.md)、
[启动资料升级进度](../design-docs/startup-upgrade-progress.md)。

# 1. 问题

`Engine::start(config, host, StartupSignals)` 要求宿主先创建两个只能消费一次的输入。这两个输入的方向不同：

| 输入 | 本质 | 产生者 | 消费者 |
| --- | --- | --- | --- |
| `progress` | 引擎在启动过程中产生的只读状态 | 引擎 | 宿主（可选） |
| `lifecycle` | 宿主在启动开始前已有的通知源 | 宿主 | 引擎 |

把一个**输出**（progress）和**输入**（lifecycle）打包成同一个参数后，每个不观察进度的宿主都必须写
`StartupSignals::unobserved()`。这不是声明，而是签收一个自己不想读的输出。当前树中有 179 处这样写。

这个方向错误在下游产生了三样只为它存在的东西：

- `StartupSignals` 及其 4 个构造器（`startup_signals.rs`，55 行）；
- `StartupProgressInput`，以及它的 `track_cleanup(&Arc<StartupCleanupOwner>)` 反向接线：因为 store 由外部创建，
  需要事后把它挂回清理负责人；
- 绑定层创建进度通道后立即丢弃：`let (progress, _) = StartupProgress::channel();`（uniffi 与 OHOS 各一处）。

同时确认一个**不是**问题的部分：启动所有权与失败清理的复杂度来自 ADR-035 的释放回执契约，与本次改动无关。
`Engine::drop` 已经通过 `orphan_retention.retain` 把实例交给进程级 closer，所以那套 guard 不是在防泄漏，
而是在保证宿主拿到的 `Pending` / `Released` / `Failed` 不是假回执。本文不动这个契约。

# 2. 目标

- 引擎自己创建进度 store，并把只读观察者作为**返回值**交给宿主，不再要求宿主注入。
- `lifecycle` 保持注入，因为它必须在宿主能力准备之前存在；它只出现在真正需要启动期暂停/恢复与释放回执的宿主入口上。
- 删除 `StartupSignals`、`StartupProgressInput` 与 179 处 `unobserved()`。
- 公开入口从「一个函数 + 一个打包参数」变成「一个基础入口 + 一个回执入口」，第二个入口的理由是能力差异，
  不是省略参数。
- 不改变四个所有权窗口、不改变 `StartupCleanupOwner` 的回执语义、不改变绑定层对外构造器签名。

# 3. 非目标

- 不修改 ADR-035 的 `Pending` / `Released` / `Failed` 分类与错误保留规则。
- 不把构造改成同步 spawn（理由见 5.4）。
- 不合并 `Engine` 与 `RecoverableRuntime`，不改变可恢复运行期的重建路径。
- 不修改 `MobileEngine` / `OhEngine` 的公开构造器集合。
- 不调整进度快照的字段、单位或去重语义。

# 4. 现状

## 4.1 完整负责人

`Engine::start` 是唯一启动入口，同时承担三件事：接管进程资源、证明这些资源被释放、构造可用实例。
本次改动只拆第一件与第三件对外的**形状**，不拆负责人。

## 4.2 唯一调用与结果

宿主对一次启动只有一个动作：交出 `(config, host, signals)` 并等待。
成功结果 `(Engine, EventStream)`；失败结果 `EngineError`，清理结果通过 `StartupLifecycle` 独立报告。
调用方丢弃等待后，恢复责任仍在引擎：未 poll 由 `UnpolledStartupCleanup` 关闭宿主，构造中断由
`StartupConstructionGuard` 关闭已建运行期，构造完成但未交付由 `StartupHandoff` 关闭实例。

## 4.3 证据

| 观察 | 位置 | 说明 |
| --- | --- | --- |
| 绑定层丢弃进度 | `bindings/uc-engine-uniffi/src/runtime.rs`、`bindings/uc-ohos-napi/src/runtime.rs` | progress 只被当作填充 `StartupSignals` 的占位值 |
| 外部 store 需要反向接线 | `crates/uc-engine/src/engine/startup.rs` | `track_cleanup` + `Option<Weak<StartupCleanupOwner>>` 只为外部创建的 store 服务 |
| 228 个调用点 / 4 种构造器 | 全仓库 | 179 × `unobserved()`、13 × `with_progress()`、34 × `new()`、2 × `channel()` |
| 进度由引擎内部层层下传 | `crates/uc-engine/src/runtime/mod.rs` | `ProductionRuntime::start` 直接接收 `Arc<StartupProgressStore>`，说明 store 本来就是引擎内部产物 |

启动相关代码规模（含内联测试）：`startup_owner.rs` 1547、`startup.rs` 466、`owned_executor.rs` 322、
`startup_outcome.rs` 223、`startup_control.rs` 197、`startup_signals.rs` 55。其中 `startup_owner.rs`
有 10 个类型参与一次启动的所有权与清理。本次改动预计删除约 100 行公开形状代码，不减少这些所有权类型。

## 4.4 宿主实际用法

两个宿主仓库的 `origin/main` 当前调用如下，用来确认两个方向各自服务谁：

| 宿主 | 位置 | 入口 | 需要什么 |
| --- | --- | --- | --- |
| 桌面 daemon（产品主进程） | `uniclipboard/apps/daemon/src/daemon/host.rs` | `Engine::start_with_progress` | 只需要进度：绑定 `StartupServer` 把快照通过本地 socket 暴露给 GUI |
| 桌面 CLI | `uniclipboard/crates/uc-bootstrap/src/entrypoint/cli.rs` | `Engine::start` | 两者都不要 |
| iOS | `uniclip/modules/uc-engine/ios/SharedEngineHost.swift` | `MobileEngine.startWithAnalyticsAndLifecycle` | 需要 lifecycle：系统后台回调可能在构造返回前到达 |
| Android | `uniclip/modules/uc-engine/android/.../UcEngineModule.kt` | `MobileEngine.startWithAnalyticsAndLifecycle` | 同上 |

桌面侧没有任何 `StartupLifecycle` 或 `start_with_lifecycle` 引用。这验证了本文的方向划分：
progress 是桌面唯一需要的那一半，lifecycle 是移动端唯一需要的那一半，CLI 两者都不要。

桌面 daemon 已经在用「先 spawn future 再 select」的写法，因此 `Startup` 必须继续实现 `Future`：

```rust
let startup = Engine::start(engine_config, host_capabilities);
let progress = startup.progress();
let mut starting = tokio::spawn(startup);
```

# 5. 方案

## 5.1 公开形状

```rust
impl Engine {
    /// 所有宿主的基础入口。不接收任何观察者。
    pub fn start(config: EngineConfig, host: HostCapabilities) -> Startup;

    /// 需要启动期暂停/恢复与释放回执的宿主（移动端两个绑定入口）。
    pub fn start_with_lifecycle(
        config: EngineConfig,
        host: HostCapabilities,
        lifecycle: StartupLifecycleInput,
    ) -> Startup;
}

/// 一次启动的句柄：既是等待结果，也是读取进度的唯一途径。
pub struct Startup {
    inner: Pin<Box<dyn Future<Output = Result<(Engine, EventStream), EngineError>> + Send>>,
    progress: StartupProgress,
}

impl Startup {
    /// 每次调用返回新的克隆；不取不会影响启动。
    pub fn progress(&self) -> StartupProgress;
}

impl Future for Startup {
    type Output = Result<(Engine, EventStream), EngineError>;
    // poll 转发到 inner，不做额外工作。
}
```

使用方式：

```rust
// 不观察
let (engine, events) = Engine::start(config, host).await?;

// 观察启动进度：必须在 await 之前取走克隆
let startup = Engine::start(config, host);
let progress = startup.progress();
let (engine, events) = startup.await?;

// 启动期退后台 + 释放回执
let (input, lifecycle) = StartupLifecycle::channel();
let (engine, events) = Engine::start_with_lifecycle(config, host, input).await?;
```

`Startup` 实现 `Future` 而不是 `IntoFuture`，这样 `tokio::spawn(Engine::start(...))` 保持可用。

## 5.2 内部改动

- `StartupProgress::channel()` 变为 crate 内使用；`Engine::start*` 在同步段创建 store，把 `StartupProgress`
  （可克隆观察者）留在 `Startup` 中，把 `Arc<StartupProgressStore>` 直接交给运行期。
- `track_cleanup` 与 `StartupProgressInput::cleanup` 字段删除：store 在 `Engine::start` 内创建，清理负责人在
  同一处登记，无需弱引用反查。
- 现有同步段（`accept_host`、`register_requests`、`register_progress`、`UnpolledStartupCleanup::new`）顺序不变。
- `start_owned` 返回的 future 装箱后放进 `Startup`；所有权窗口与 guard 不变。
- `start_with_lifecycle` 复用注入的 `TransitionQueue`；`start` 自建，语义与今天 `unobserved()` 一致。
- **必须迁移的隐含语义：** `StartupProgressInput` 的 `Drop` 会在快照仍非终态时发布 `Interrupted`，清理未确认时发布
  不可重试的 `Failed`。这是「观察者不会永远等下去」的唯一保证。输入对象删除后，这段逻辑必须由三个放弃窗口
  显式承担：未 poll 丢弃走 `UnpolledStartupCleanup`，构造中断走 `StartupConstructionGuard`，构造完成未交付走
  `StartupHandoff`。不得改成依赖 store 的引用计数，因为 store 会被清理负责人继续持有。

## 5.3 删除清单

| 删除项 | 位置 | 行数 |
| --- | --- | --- |
| `StartupSignals` 及 4 个构造器 | `crates/uc-engine/src/engine/startup_signals.rs` | 55 |
| `StartupProgressInput` | `crates/uc-engine/src/engine/startup.rs` | 约 62（含 impl 与 Drop，部分方法迁入 store） |
| `track_cleanup` 与弱引用字段 | `crates/uc-engine/src/engine/startup.rs` | 约 10 |
| `pub use engine::StartupSignals` 与仓库检查中的稳定出口条目 | `crates/uc-engine/src/lib.rs`、`scripts/architecture/check-engine-repository.mjs` | 2 |
| 179 处 `unobserved()`、13 处 `with_progress()` 参数 | 调用点 | — |

新增 `Startup` 约 40 行。每个实例一次额外的 `Box` 分配，只包住外层等待 future（几十字节）；真正的构造任务仍然
spawn 到自有执行器，不因本次改动增加堆分配。

## 5.4 保留项与理由

**保留窗口 1（未 poll 的 future）与 `UnpolledStartupCleanup`。** 曾考虑在 `Engine::start` 内同步 spawn 到
`OwnedExecutor`，从而让「future 创建但从未 poll」不可能发生。实际代价不可接受：`OwnedExecutor::start()` 会
阻塞在 `initialized.recv()` 上等待新线程建好 runtime，把这段阻塞挪进 `Engine::start` 就等于让宿主在调用方线程
上同步等待线程创建；若不阻塞，则需要再起一个线程去执行创建，比现在更重。因此继续把线程与执行器留到首次 poll。

**保留 `StartupHandoff`。** 构造完成但调用方从未取走结果，仍需要一个持有关闭责任的守卫。

**保留 `StartupConstructionGuard`。** 构造中断、panic、执行器丢失时的清理责任不因入口形状改变。

**保留 `StartupCleanupOwner` 与 ADR-035 回执。** 这是移动端运行时锁交接的前提，不属于本次简化范围。

## 5.5 调用点迁移规则

| 旧 | 新 |
| --- | --- |
| `Engine::start(cfg, host, StartupSignals::unobserved())` | `Engine::start(cfg, host)` |
| `Engine::start(cfg, host, StartupSignals::with_progress(input))` 且需要观察者 | `let startup = Engine::start(cfg, host); let progress = startup.progress(); startup.await` |
| `Engine::start(cfg, host, StartupSignals::channel())` | 同上；原来取 `lifecycle` 的调用点改用 `start_with_lifecycle` |
| `Engine::start(cfg, host, StartupSignals::new(progress, lifecycle))` | `Engine::start_with_lifecycle(cfg, host, lifecycle)` |
| `StartupProgress::channel()` + 取 `.0` 只为填充参数 | 删除该行 |

跨仓库的对应迁移（不在本仓库，Engine 发布后随宿主升级一起改）：

| 宿主调用 | 迁移后 |
| --- | --- |
| `tokio::spawn(Engine::start_with_progress(cfg, host, input))` | `let startup = Engine::start(cfg, host); let progress = startup.progress(); tokio::spawn(startup)` |
| `Engine::start(cfg, host)` | 不变 |
| `MobileEngine.startWithAnalyticsAndLifecycle(...)` | 不变（绑定内部改用 `start_with_lifecycle`） |

# 6. 分阶段实施

S1（一个提交，内部 + 公开形状一起改）。新增 `Startup`、`start_with_lifecycle`，删除 `StartupSignals` 与
`StartupProgressInput`，迁移全部 228 个调用点。不得在同一分支合并前保留两套入口。

S2（一个提交）。绑定层两个入口改用 `start_with_lifecycle`，删除为填充参数而创建的进度通道。

S3（一个提交）。文档：更新 `uc-engine-interface.md` 的启动章节、`startup-upgrade-progress.md` 的接入示例，
新增 ADR-036 记录方向拆分与「两个入口而不是一个」的取舍，`PLANS.md` 与 `active/index.md` 登记本计划。

# 7. 边界与失败行为

| 场景 | 行为 |
| --- | --- |
| 宿主从不调用 `progress()` | 进度照常写入，观察者随 `Startup` 一起释放，启动不受影响 |
| `Startup` 从未 poll 就被丢弃 | `UnpolledStartupCleanup` 关闭宿主（与今天一致） |
| 首次 poll 后丢弃 | `StartupConstructionGuard` 关闭已建运行期（与今天一致） |
| 构造完成但未取走结果 | `StartupHandoff` 在独立线程关闭实例（与今天一致） |
| 上述三种放弃发生在观察者仍存活时 | 观察者必须收到终态 `Interrupted`（清理未确认时是不可重试的 `Failed`），不得挂起 |
| 宿主在 `await` 之后才想取进度 | 取不到；`Startup` 已消费。文档必须写明「先克隆再 await」 |
| 同时使用 `start_with_lifecycle` 与自建进度 | 允许；进度仍由引擎创建并从 `Startup` 取 |

# 8. 测试策略

现有 E2E 全部保留、断言不改，作为「契约未变」的回归证据：

- `crates/uc-engine/tests/host_contract/startup/`（含 `abandoned_cleanup`、`partial_runtime`、`control`、`failure`）
- `bindings/uc-engine-uniffi/tests/public_contract/startup_lifecycle.rs`
- `crates/uc-engine/src/engine/startup_owner/tests.rs` 中的放弃启动用例，特别是
  `abandoned_startup_cleanup_failure_is_visible_to_the_progress_observer`：它保护的正是 5.2 要求迁移的 Drop 语义。

新增（端到端，真实 Engine，不用 mock）：

1. `progress_is_observable_without_any_precreated_input`：只调用 `Engine::start(cfg, host)`，在 await 之前取
   `progress()`，断言能观察到 `Upgrading` 与终态 `Ready`。
2. `dropping_startup_without_polling_still_closes_host`：改造现有 abandoned 用例，改为不传任何信号直接丢弃。
3. `progress_clone_taken_before_await_receives_terminal_state`：await 结束后用提前克隆的观察者读到最后快照。
4. `abandoned_startup_still_publishes_interrupted_to_the_observer`：真实 Engine，在构造被宿主回调阻塞时丢弃
   `Startup`，断言观察者拿到终态而不是继续等待。

# 9. 验收条件

- `rg "StartupSignals" --glob '*.rs'` 为空，`rg "unobserved" --glob '*.rs'` 为空。
- 228 个调用点全部迁移，`Engine::start` 只剩 2 参形式，`start_with_lifecycle` 只出现在两个绑定入口与启动测试。
- 上述 4 个新 E2E 通过；`host_contract startup::`、`public_contract startup_lifecycle` 全绿。
- 交付门禁通过：`just cargo check --workspace --all-targets --locked`、`just cargo fmt --all -- --check`、
  `node scripts/architecture/check-rust-style.mjs`、`node scripts/architecture/check-engine-repository.mjs`、
  `just cargo metadata --locked --format-version 1`、`git diff --check`。
- 文档更新完成，ADR-036 已写。

# 10. 风险与取舍

- **多一个公开类型。** `Startup` 的存在理由和 `StartupSignals` 相同：给两个方向不同的东西各一个位置。区别是
  progress 走返回值、lifecycle 走参数，符合各自方向；代价是一次 `Box` 与一个 `Future` 实现。
- **迁移面积大。** 228 个调用点，靠编译错误驱动；迁移完成前不得合并。
- **`Startup` 的 `Send` 与 `Unpin`。** 必须保持 `Send`，否则 `tokio::spawn` 不可用；`Pin<Box<..>>` 本身 `Unpin`。
- **取句柄的时机。** 宿主若在 `await` 之后取进度会拿不到。这是新形状唯一的脚坑，必须在接口文档中写明。
- **回执契约不变。** 本次不减少 `startup_owner.rs` 的所有权类型；如果目标是压缩那部分复杂度，需要单独计划。

# 11. 未决问题

1. `Startup` 是否也暴露 `lifecycle()`？若暴露，绝大多数宿主可以只用 `start`，`start_with_lifecycle` 仅保留给
   「控制对象必须先于宿主能力准备存在」的移动端。倾向于**不暴露**，保持第二个入口的唯一理由清晰。
2. `start_with_lifecycle` 的命名是否合适？备选 `start_with_startup_control`，或按能力命名为 `start_observable`。
3. 是否需要在 ADR-036 中同时记录「为什么不做同步 spawn」？倾向于需要，否则后续会重复讨论 5.4 的取舍。
