# 044 Engine testkit、测试分组与结构化报告基础

## 状态

- 状态：待实施
- 日期：2026-09-21
- 完整负责人：`uc-testkit::Scenario`
- 唯一调用：测试用稳定名称、固定 seed、预算和复现命令创建场景，最后提交一次成功或失败结果
- 成功结果：结构化 JSON、人类摘要、阶段耗时、最后事件、资源与清理结果
- 失败结果：稳定失败分类、未满足条件、最后事件、阶段耗时、工件位置和复现命令
- 重试责任：testkit 不重试；nextest profile 负责 runner 重试，本轮默认关闭
- 恢复责任：资源由 lease RAII 回收；process/network/device 的外部恢复留在各自 driver
- 长期设计：[Engine 测试架构](../../design-docs/testing-architecture.md)

## 1. 问题与目标

当前仓库缺少跨测试层复用的场景证据、资源租约和失败分类，慢测试只能依赖各自文本输出和总超时。0040 调查已确认真实网络测试承担过多业务状态验证，但本计划不迁移这些测试，只建立后续迁移所需的基础设施。

本轮目标：

1. 建立独立 `uc-testkit` workspace crate，不接触产品业务。
2. 引入 cargo-nextest，建立六类测试边界、统一命令和 JUnit。
3. 用成功示范与受控故意失败示范证明 JSON、摘要、事件等待、阶段计时、资源与清理结果可用。
4. 在 PR CI 增加非破坏入口，保留全部现有 cargo test/coverage/network job。

## 2. 非目标与边界

- 禁止修改、迁移、复制或重写 t-0010 工作区及其场景。
- 不实施规格 034 的虚拟网络或业务虚拟时间。
- 不修改生产 crate 接口、行为、持久格式或协议。
- 不移动现有测试，不删除旧 runner，不替换现有 CI 门禁。
- 不为未来进程/设备控制预先增加空接口。

## 3. 文件与模块

```text
tests/uc-testkit/
  Cargo.toml
  src/lib.rs
  src/identity.rs
  src/budget.rs
  src/event.rs
  src/failure.rs
  src/report.rs
  src/resource.rs
  src/scenario.rs
  tests/scenario_demo.rs
  examples/failure_demo.rs

.config/nextest.toml
scripts/testing/run-test-group.sh
.github/workflows/pr-check.yml
docs/design-docs/testing-architecture.md
docs/architecture/architecture-bible.md
```

模块职责以设计文档为准。本轮若实现中证明可以合并文件，应以最少清晰模块为准，不为了目录形状制造转发层。

## 4. 工具与兼容策略

- 使用固定版本 cargo-nextest；CI 通过官方安装 action 安装，本地脚本检查版本与可用性。
- 使用现有 Tokio、Serde、Serde JSON、tempfile 和 thiserror 生态；新增依赖先确认 workspace 锁文件已有或确有必要。
- cargo test 不读取 nextest 配置，保持现有行为。
- nextest 组先按 package/binary/name pattern 映射；real-network/device 继续调用现有专用脚本，统一入口明确转交，不伪装为普通 Rust test。
- JSON schema 从版本 1 开始；本轮无历史兼容负担。后续只做向前兼容增加，破坏性变更提升 schema version。

## 5. 阶段与验收

### Phase 0：规格与计划

- [x] 阅读架构、文档规则、规格 034 和 0040 报告。
- [x] 确认 `CONTEXT.md` 缺失并记录，不猜测。
- [x] 完成设计文档、执行计划、索引和架构圣经记录。
- [x] 运行 Markdown 链接和 `git diff --check` 自检。

出口：另一个未参与调查的开发者只读两份文档即可实现；明确不迁移 t-0010。

回退：删除尚未实施的文档和索引，不影响代码。

### Phase 1：testkit 最小切片

- [ ] 新增 workspace crate 和最小模块。
- [ ] 场景身份、seed、预算、阶段、事件等待、失败分类、目录/端口 lease、报告和清理结果可用。
- [ ] 添加 testkit 自测试。

出口：`cargo test -p uc-testkit --locked` 通过；没有产品 crate 依赖。

回退：从 workspace 移除独立 crate；产品代码不需回退。

### Phase 2：示范与报告

- [ ] 成功示范产生 passed JSON 和摘要。
- [ ] 故意失败示范在短预算内产生 failed JSON 和摘要，验证后以成功退出保留 CI 可运行性。
- [ ] 工件不含真实临时路径或自由产品 payload。

出口：实际读取 JSON，字段和失败分类满足设计；工件路径可复现。

回退：示范可删除，不影响 testkit 核心。

### Phase 3：nextest 与本地入口

- [ ] 安装并验证固定 cargo-nextest 版本。
- [ ] 增加 `.config/nextest.toml`、六类边界和 JUnit。
- [ ] 增加 `run-test-group.sh`，fast 组运行非零测试。
- [ ] real-network/device 组只转交现有入口或明确要求参数，不自动运行设备。

出口：fast 组成功，JUnit 可解析；cargo test 同一自测试仍通过。

回退：删除配置和脚本，cargo test 不受影响。

### Phase 4：非破坏 CI

- [ ] PR workflow 新增独立 testkit job。
- [ ] 固定工具版本并上传 JUnit/JSON 工件。
- [ ] 不修改 checks、coverage、connection-recovery 的成功条件。

出口：workflow 语法/仓库检查通过；本地无法证明远程 CI 时明确记录未运行。

回退：删除新增 job，不影响原门禁。

### Phase 5：完整验证与本地提交

- [ ] 相关测试、成功/失败示范和一个现有快速回归通过。
- [ ] 根 AGENTS 交付检查通过。
- [ ] 更新本文实际时长、工件位置、跳过项和后续迁移建议。
- [ ] 创建一个本地原子提交，不推送。

## 6. CI 分组配置

| 组 | nextest/脚本映射 | 本轮状态 |
| --- | --- | --- |
| fast | `uc-testkit` 和后续明确快速 package/binary | 本轮实际接入 `uc-testkit` |
| persistence-provider | 指定 Infra integration binary/name | 只建立配置边界，不迁移测试 |
| engine-smoke | 指定 Engine integration binary | 只建立配置边界，不迁移测试 |
| process | host package/binary | 只建立配置边界，不迁移测试 |
| real-network | `run-connection-recovery-e2e.sh` | 保持现有脚本；不在本轮执行 |
| device | 平台 host 脚本/device matrix | 只打印受支持入口；本轮跳过 |

## 7. 风险与回退条件

- 若 nextest 当前版本不兼容 Rust 1.95，固定最近兼容版本，不升级 Rust。
- 若分组 pattern 会意外包含真实网络长测，fast 只运行显式 `uc-testkit`，不扩大集合。
- 若 testkit API 需要产品内部状态，停止扩大范围；在本文记录后续 seam，不修改生产接口。
- 若 JSON 工件无法保证脱敏，CI 暂不上传，只保留本地临时验证，修正 schema 后再接入。
- 若新增 CI 显著增加 PR 时间，保持 job 非门禁并单独评估；不删除原流程。

## 8. 实际进度与证据

| 日期 | 阶段 | 结果 |
| --- | --- | --- |
| 2026-09-21 | Phase 0 输入 | 完成；仓库未找到 `CONTEXT.md`，本机未安装 cargo-nextest。 |

## 9. 下一阶段候选迁移

本轮完成后，优先选择与 t-0010 无关、运行时间短且没有业务虚拟时间要求的测试采用 testkit，例如：

1. 测试宿主启动失败时的资源释放合同。
2. 独立诊断导出流程的阶段计时与工件验证。
3. 一个 Infra loopback provider contract 的端口租约和失败分类。

t-0010 的配对、候选收敛、三设备、维护恢复和重启场景继续留在其工作区，只有在后续明确交接后才按规格 034 迁移。
