# Engine 测试使用指南

本指南面向第一次在 Engine 仓库新增或修改测试的贡献者。长期边界见
[Engine 测试架构](testing-architecture.md)，当前采用清单见
[测试采用清单](../references/test-adoption-inventory.md)。

## 1. 先选择测试层级

| 需要证明什么 | 首选层级 | 是否使用 testkit |
| --- | --- | --- |
| 单个纯函数、值对象、状态转换 | Core/Application 普通单元测试 | 否。直接使用 `cargo test` |
| 一个 Application 完整负责人，使用内存 port、可控消息或可控时间 | Application 场景测试 | 仅在需要阶段、等待、资源或结构化失败证据时使用 |
| SQLite、文件、密码、codec、provider 合同 | Infra integration/provider | 有多阶段、资源或失败分类时使用 |
| 稳定 `uc-engine` 公开入口的短链路 | Engine smoke/contract | 有跨阶段诊断需要时使用 |
| 独立进程、崩溃、重启、持久恢复 | process | 使用，统一记录退出、预算与清理 |
| network namespace、relay、真实发现与断线 | real-network | 保留专用脚本；testkit 不能替代真实链路 |
| 模拟器、实体设备、绑定和宿主 | device | 保留平台宿主；未运行只能记为跳过 |

简单、快速、没有资源生命周期的单元测试不强制套 `Scenario`。不要为了统一外观把纯规则测试改写成场景 DSL。

## 2. 复用现有支撑

新增 fixture 前按顺序检查：

1. 目标业务目录现有 `tests` 或 `test_support`，优先调用真实完整负责人。
2. `crates/uc-application/src/test_support/` 中跨业务测试已经共用的窄支撑。
3. `uc-testkit` 的场景身份、预算、事件等待、临时目录、端口和子进程能力。
4. `.config/nextest.toml` 已有分组是否能准确选中测试。

testkit 不解释 admission、membership、provider 或存储状态，也不复制生产状态机。只有多个真实调用方已经需要相同的测试生命周期能力时，才向 testkit 增加入口。

### 文件放在哪里

1. 场景只需要 crate 公开接口：放在该 crate 的 `tests/`。
2. 场景必须读取私有状态或调用私有测试装配：放在业务模块的 `src/**/tests/`，由模块的 `#[cfg(test)]`
   入口接入。
3. 只服务该领域场景的 fixture：放在相邻 `tests/support/`；已有明确领域测试网络可放在 `testing/`。
4. 跨领域通用能力才进入 `tests/uc-testkit/`。

不要为了把测试移到 crate `tests/` 而把生产私有接口改为 `pub`，也不要新增生产测试开关。短小的既有
`tests.rs` 和明确命名的历史 `test_support.rs` 可以保留；新增长场景不能继续内嵌在业务实现文件中。

## 3. 可直接运行的最小示例

从仓库根目录执行：

```bash
cargo test -p uc-testkit --test scenario_demo --locked
```

运行成功和受控失败示范，并把两次工件写到同一根目录：

```bash
export UC_TEST_ARTIFACTS_DIR=target/test-artifacts/guide
cargo run --quiet --locked -p uc-testkit --example scenario_demo -- success
cargo run --quiet --locked -p uc-testkit --example scenario_demo -- failure
find "$UC_TEST_ARTIFACTS_DIR" -name result.json -o -name summary.txt
```

受控失败示范本身以成功退出，因为它先验证失败分类和工件；`result.json` 的 `outcome` 仍为 `failed`。每次运行会创建独立的安全实例目录，相同场景并行或重复运行不会覆盖。

## 4. 场景最小结构

```rust
use std::time::Duration;

use uc_testkit::{Scenario, ScenarioBudget, ScenarioConfig};

async fn run_scenario() {
    let scenario = Scenario::start(ScenarioConfig::new(
        "provider-contract",
        0x504f_5254,
        ScenarioBudget::new(Duration::from_secs(2)),
        "cargo nextest run -p my-crate -E 'test(provider_contract)'",
        "target/test-artifacts/provider-contract",
    ))
    .expect("scenario starts");

    scenario.record_event("request-sent");
    scenario.record_event("response-accepted");
    scenario.finish(Ok(())).expect("scenario report");
}
```

场景名、阶段名、事件、资源 kind/label 和 condition 必须是静态、脱敏的稳定类别，不能包含设备名、地址、邀请、令牌、文件名、绝对路径或业务正文。

## 5. 预算、等待和资源

- `ScenarioBudget` 是整个场景的 wall-clock 保护预算，不是产品业务 deadline。
- `stage("stable-name")` 只记录阶段耗时；guard 离开作用域时结束。
- `wait_for_event` 使用事件通知，不用固定 sleep 轮询产品状态；失败包含 condition 与最后事件。
- `temp_dir` 创建权限受控并在 Drop 时清理的目录。
- `tcp_port` 持有已绑定 listener，避免“探测后释放”的端口竞争。
- `run_child_process` 在显式预算内 wait；超时后 kill 并再次 wait，资源 cleanup 写入报告。非零退出码由业务场景决定属于产品失败还是预期故障注入。
- 外部资源只有调用方确实拥有其生命周期时才用 `record_external_resource` 登记结果。

nextest 仍是测试进程的最终超时负责人。场景预算应短于 nextest override，给 JSON/摘要写入和清理留出时间。

## 6. 分组与本地运行

统一入口：

```bash
bash scripts/testing/run-test-group.sh fast
bash scripts/testing/run-test-group.sh evidence
bash scripts/testing/run-test-group.sh persistence-provider
bash scripts/testing/run-test-group.sh engine-smoke
bash scripts/testing/run-test-group.sh process
```

`real-network` 会转交现有 Linux 网络脚本；`device` 要求明确平台与设备，不会自动运行。cargo-nextest 必须为脚本声明的固定版本；脚本不会静默退回语义不同的 runner。

新增测试接入分组时：

1. 优先按 package、test binary 或稳定测试模块名写 nextest filter。
2. 为共享端口、数据库或全局 subscriber 等资源设置串行 test-group。
3. 给测试设置比场景预算更长的 `slow-timeout`。
4. 先保留原 `cargo test` 入口并双轨运行，不直接删除旧门禁。
5. 在 PR workflow 的 evidence 入口增加测试时，同步上传其 JSON/摘要目录。

## 7. 查看和复现失败

nextest 的 JUnit 位于：

```text
target/nextest/ci/junit.xml
```

场景目录包含 `result.json` 和 `summary.txt`。先读摘要，再核对 JSON 的：

- `failure.kind` 与 `failure.secondary`
- `failure.condition`
- `last_event`
- `stages`
- `resources` 与 `cleanup`
- `reproduce`
- `artifact_directory`

运行报告中的 `reproduce` 命令，不要根据失败文本手工猜过滤条件。若测试进程在 Scenario `finish` 前 panic/abort，场景 JSON 可能不存在；此时以 nextest/JUnit 和进程输出为准。本仓不安装全局 panic hook 改变其他测试行为。

## 8. CI 接入检查表

- 旧 `cargo test` 或专用脚本仍可运行。
- 新测试进入正确分组，没有被 `fast` 意外选入真实网络或设备集合。
- CI 上传 JUnit、JSON 和文本摘要，`if: always()` 保留故障证据。
- 默认不重试；任何 retry-pass 必须单独统计，不能当作稳定通过。
- 真实网络和设备未执行时明确写“跳过/未验证”。
- 先读取实际工件再报告成功，不以 workflow 绿色代替报告验收。

## 9. 快速线与真实环境线

- 日常和 PR 使用快速确定性线。单场景尽量控制在 1 秒内，整组控制在 1 分钟内，均不含编译。
- nightly 使用真实 Engine 多进程、独立身份/资料/端口、真实存储和对应的真实网络环境；准备、测试和清理合计
  目标为 30 分钟内。编译耗时和整次总耗时必须单独展示。
- 真实环境 workflow 应允许手工选择单个场景。网络故障必须由 network namespace 或对应真实环境实际施加，不能
  用 mock 返回错误替代。
- 超时是失败，不通过扩大预算、放宽断言、删除覆盖或自动重试隐藏。确需重跑时保留首次失败工件，并把
  retry-pass 单独登记。
- 发布前检查近期 nightly 结果，并补跑受变更影响的配对、文字/文件传输、断线重连、重启恢复或旧资料升级场景。

当前不要把两条线混用：`VirtualMembershipNetwork` 只是配对后成员历史的快速 fixture；快速文字场景证明
`ClipboardSyncFacade` 的 V3 编码与 accepted fan-out，快速文件场景只证明公开 `FileTransferFacade` 的完成生命周期；
完整配对和 E02 文字/文件
传输属于真实 Engine runner。五类逐项状态和实测值以[测试架构的首批业务覆盖矩阵](testing-architecture.md#首批业务覆盖)
为准。

快速 admission 场景的作者入口保持窄小：准备 `JoinSpaceInput`，调用一次 `complete_joiner_pairing`，只断言返回的
Active 状态和最终确认。fixture 内部调用真实 `SpaceAdmissionProtocol`、成员维护与激活负责人；测试不得编排
Candidate/Commit/Complete/ACK 或固定恢复轮次。当前示范可直接运行：

```bash
cargo nextest run -p uc-application \
  -E 'test(joiner_pairing_fixture_reaches_active_settled)' --locked
```

这个快速入口只证明加入方确定性规则。邀请方最终确认唯一性由三设备场景证明；双方 Engine 的 same-space、usable、
online 和真实传输仍必须运行下面的真实 runner。

真实 runner 的最小调用示例：

```bash
# 完整配对
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode direct --repeat 1 --case E01

# 只登记文件传输场景；runner 自动完成未登记的必要配对/文字 setup
bash scripts/testing/run-connection-recovery-e2e.sh --suite network --mode direct --repeat 1 --case E02-file
```

这两个命令需要 Linux network namespace 和相应权限。macOS 上的脚本语法或 host 编译通过不构成场景通过。
runner 工件中的 `timings.prepare_ms`、`scenario_ms`、`cleanup_ms` 和 `total_ms` 用于核对 30 分钟目标；场景 records
仍保留每个业务步骤耗时，二者不能互相替代。
