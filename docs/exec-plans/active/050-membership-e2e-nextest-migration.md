# 050 成员多设备真实场景迁入 nextest 与 testkit 架构

## 状态与完整责任

- **状态**：实施中；S1–S3、S5 已完成，S4 本地完成、远程运行待合并后登记。
- **日期**：2026-09-24。
- **依据**：[Engine 测试架构](../../design-docs/testing-architecture.md)（nextest 负责进程调度、分组、超时与 JUnit，
  `uc-testkit::Scenario` 负责单场景预算、阶段、事件、资源与工件）；[049](../completed/049-single-owner-space-membership-rewrite.md)
  S3 验证中发现该套件长期游离于上述架构之外：只在 `dev-tools` 下编译，`cargo test -p uc-engine` 不运行它；
  CI 只运行其中 `automatic_connections::`；超时是分散常量，失败只留 stdout。
- **完整负责人**：
  - 测试进程发现、过滤、分组、并发与总超时：cargo-nextest 与 `.config/nextest.toml`。
  - 单场景生命周期（节点目录、预算、阶段、事件、失败工件、清理）：测试台模块
    `crates/uc-engine/tests/space_membership_auto_pairing_e2e/harness/`，内部持有 `uc_testkit::Scenario`。
  - 场景正文只描述设备动作与业务断言，不管理目录、超时或工件。
  - 本计划顺序与验收：本计划。
- **调用方唯一动作**：`bash scripts/testing/run-test-group.sh membership-e2e [nextest 参数]`；CI 通过同一入口。
- **成功结果**：同一组场景以按类别划分的模块运行，按类别受 nextest 并发与期限约束；每个场景留下
  `result.json` 与 `summary.txt`；PR 运行冒烟子集，夜间与手动运行全组并保留 JUnit 与场景工件。
- **失败结果**：场景失败保留首次失败的分类、条件、最后事件与阶段耗时；清理失败不覆盖原始失败；超时是失败，
  不以放宽断言或自动重试掩盖。
- **重启与重试责任**：testkit 与 nextest 都不重试；产品恢复仍由各业务负责人拥有。

## 范围

### 目标

1. 把 6281 行单文件拆为共享测试台与按类别的场景模块，仍是一个测试二进制（只链接一次 Engine）。
2. nextest 按模块路径为不同类别设置并发与期限：十节点拓扑低并发，两三节点场景较高并发。
3. 测试台接入 `uc-testkit::Scenario`：节点目录走租约，统一预算替代分散超时常量，拓扑动作记为阶段与事件，
   结束时写出工件。
4. CI：PR 检查以 nextest 运行冒烟子集（含现有 `automatic_connections::`）；`engine-real-environment.yml`
   夜间与手动触发运行全组并上传 JUnit 与场景工件。
5. 更新测试指南与测试采用清单。

### 非目标

- 不改任何场景的业务断言；049 要求 F2–F7 不改即通过，本计划只移动代码与替换测试台机制。
- 不改产品代码、公开接口或 dev-tools 能力。
- 不把协议矩阵下沉到 Application 虚拟网络（034 的后续工作，049 完成后另行安排）。
- 不删除 `cargo test` 入口（测试指南要求新旧入口并存）。
- 不修复既有基线失败；它们在迁移前后保持同一结果并如实记录。

## 目标结构

```text
crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs   入口：cfg 门控与模块声明
crates/uc-engine/tests/space_membership_auto_pairing_e2e/
  harness/            宿主替身、DeviceHarness、rendezvous、MembershipTopology、通用操作与等待
  clipboard_trace.rs  剪贴板与发送的业务 trace
  pairing.rs          配对热路径、配对观测、准入 trace
  admission.rs        加入、最终确认、重启、重复身份与同设备重新加入
  topology.rs         F0–F7、交叉移除、交接、离线成员与拓扑脚本
  removal_convergence.rs  R1–R4（已存在）
  space_switch.rs     空间切换与会话交接故障
  membership_history.rs   成员历史失败与拒绝
  automatic_connections.rs、six_digit_pairing.rs（已存在）
```

## 切片

### S1 结构拆分（纯移动）

- 按目标结构移动代码；只调整可见性与导入，不改函数体。
- **验证**：拆分前后 `cargo nextest list` 的场景集合一致（名称只增加模块前缀）；`cargo check` 与
  `membership-e2e` 全组结果与拆分前一致（同样的通过项与既有失败项）。

### S2 nextest 分类约束

- `.config/nextest.toml` 以 `test(/^topology::/)` 等过滤为拓扑类单列测试组与期限，其余沿用 `membership-e2e` 组。
- **验证**：全组耗时与失败集合；拓扑类不再与其他重场景过度并发。

### S3 测试台接入 testkit

- `DeviceHarness`/`MembershipTopology` 内部持有 `Scenario`；节点目录改用 `Scenario::temp_dir`；等待使用场景预算；
  拓扑动作记录阶段与事件；场景结束写出工件。场景预算短于 nextest 期限。
- 失败路径：等待超时先记录失败分类与最后事件再 panic，保证 panic 前已有工件。
- **验证**：成功与人为失败各一例的工件内容；全组结果不变。

### S4 CI 接入

- `pr-check.yml`：`automatic_connections::` 改由 nextest 运行，并加入少量快速场景。
- `engine-real-environment.yml`：夜间与 `workflow_dispatch` 运行全组，上传 `target/nextest/ci/junit.xml` 与场景工件。
- **验证**：本地以相同命令运行；远程运行结果在合并后登记，未运行记为“跳过”。

### S5 文档

- 更新 `docs/design-docs/testing-guide.md`、`docs/references/test-adoption-inventory.md` 与 034 中对该套件的描述。

## 实施记录

（按切片记录完成内容、验证命令与结果；未执行项记为“跳过”。）

### S1–S5（2026-09-24，分支 `hp/uni/t-0010-android`）

完成内容：

- S1：6281 行入口拆为 `harness/`（`host`、`rendezvous`、`device`、`membership_topology`、`ops`、`scenario`）与
  `clipboard_trace`、`pairing`、`admission`、`topology`、`space_switch`、`membership_history` 场景模块；入口只保留
  门控、公共导入与模块声明。拆分由脚本按“条目被哪些类别使用”自动完成：只被一个类别使用的放入该类别，
  其余放入测试台并改为 `pub(crate)`。去除 `pub(crate)` 与空白后，新旧文件的代码行集合只差函数签名换行与新增
  模块头；拆分前后 `cargo nextest list --run-ignored all` 的 62 项场景一致（名称增加模块前缀）。
- 历史等价等待修正（用户批准）：`wait_for_equivalent_branch_named` 在分支、head 与有效成员数之外，要求各节点
  未完成成员效果为 0。历史提交后效果由执行器随后落实，只比较 head 会读到旧组 epoch；拆分后首轮全组运行中
  F6 因此失败（读到 5，节点随后都为 6，单独运行通过）。F7 此前单独加的等待随之移除。场景正文与断言不变。
- S2：`.config/nextest.toml` 新增 `membership-topology` 测试组（并发 2），以 `test(/^topology::/)` 覆盖在整个
  二进制的覆盖之前；default 与 ci 两个 profile 相同。
- S3：`uc-engine` 增加 `uc-testkit` dev-dependency。每个场景第一行 `let _scenario = TestScenario::start();`
  （62 处，只加这一行）。测试台经进程内当前场景登记设备目录（`TempDirLease`）、拓扑动作事件与两类等待阶段；
  测试台等待超时先记 `product_timeout` 与条件名再 panic（`#[track_caller]` 保留调用处位置）；守卫释放时写出
  `result.json` 与 `summary.txt`，panic 时同样写出，未记录原因的断言失败记为 `product_invariant`。工件根为
  `$UC_TEST_ARTIFACTS_DIR/membership-e2e`，`run-test-group.sh membership-e2e` 设置并打印该路径。
- S4：`pr-check.yml` 新增 `membership-e2e-smoke`（自动连接、R1–R4、基础加入），替代 coverage 任务中以
  `cargo test` 运行的 `automatic_connections::`；`engine-real-environment.yml` 新增 `membership-e2e` 任务
  （nightly 与 `workflow_dispatch` 的 `mode: membership-e2e`），上传 JUnit 与场景工件，并从原手动真实网络任务中
  排除该 mode。
- S5：测试指南、测试采用清单、034 第 6 步与 `tests/observability/README.md`（`--exact` 路径加模块前缀）已更新。
  其他执行计划中出现的旧场景名是当时的运行记录，保持原样。

验证结果：

| 检查 | 结果 |
| --- | --- |
| 拆分前全组（仓库配置） | 44/51 |
| S1 后全组 | 45/51：4 个基线失败、`pending_join` 间歇失败、F6 epoch 竞态 |
| S1、S2 与等价等待修正后全组 | 47/51，281 秒；失败恰为 4 个基线失败 |
| S3 后全组 | 47/51，318 秒；51 个场景均写出工件且设备目录全部清理；失败为 3 个基线失败与 `offline_member_catches_multiple_removals_without_blocking_new_invitations`（组 epoch 等待 60 秒超时，工件分类 `product_timeout`，单独运行通过）；`confirmed_pairing_survives_restart_removal_and_same_device_rejoin` 本轮通过 |
| 工件抽查 | 成功（R1）与失败（handoff 基线失败）各一例：结果、分类、最后拓扑事件、阶段耗时、清理状态与复现命令正确 |
| `cargo metadata --locked`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`check-rust-style.mjs`、`check-engine-repository.mjs`、`git diff --check` | 通过（`uc-ohos-napi` 既有未使用导入告警） |
| 该测试目标 clippy | 新增代码无告警；既有测试中的 `expect`/`unwrap`/`println!` 告警保留 |
| 工作流 YAML 语法 | 通过（ruby YAML 解析）；未安装 actionlint，跳过 |
| PR 冒烟与 nightly 远程运行 | 跳过（需合并或推送后触发）；nightly 全组在基线失败修复前会失败 |
