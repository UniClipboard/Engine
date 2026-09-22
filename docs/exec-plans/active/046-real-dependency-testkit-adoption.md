# 046 真实依赖与独立进程 testkit 采用

## 状态

- 状态：实施中
- 日期：2026-09-22
- 完整负责人：既有 rendezvous invitation adapter、profile storage upgrade 与 `uc-testkit::Scenario`
- 唯一调用：测试通过既有 adapter 或独立进程测试入口执行一次真实依赖动作，再把稳定结果、资源和清理状态提交给 Scenario
- 成功结果：正常、拒绝/无效响应、暂时不可用和独立进程恢复场景均保持原业务断言，并生成 JSON 与文本摘要
- 失败结果：报告明确区分 `product_invariant`、`environment_unavailable` 与 `cleanup_failed`，保留阶段、最后事件、复现命令和工件位置
- 重试责任：产品重试仍由既有 adapter/upgrade owner 负责；testkit 不重试，稳定性重复由 runner 驱动
- 重启责任：profile storage upgrade 继续从现有持久 journal 恢复；testkit 只记录子进程退出和清理结果
- 长期边界：[Engine 测试架构](../../design-docs/testing-architecture.md)

## 1. 目标

阶段一和阶段二已证明 testkit 可用于自身能力与确定性 Application 场景。本阶段验证它能在不改变产品行为的前提下承载两类真实依赖测试：

1. rendezvous HTTP adapter 使用真实 loopback endpoint 与 Wiremock，覆盖正常响应、目录拒绝/无效响应和暂时不可用。
2. profile storage upgrade 使用真实 SQLite、文件系统和独立子进程，覆盖非正常退出后的持久恢复与资源清理。

目标是形成可完整交付的小切片，不迁移全仓 Infra 或 process 测试。

## 2. 非目标

- 不读取、修改、迁移或复制 t-0010 工作区及其修复测试。
- 不改变产品行为、生产公开接口、持久格式、协议或错误分类。
- 不实现通用拓扑、虚拟网络、故障 DSL 或新的 process runner。
- 不删除、替换、改名或降低旧测试和现有门禁的权威地位。
- 不运行真实外网、真实 Iroh 多节点、远程长期趋势或设备测试。

## 3. 场景与覆盖映射

| 场景 | 真实依赖 | 原有权威断言 | testkit 证据 | 预算 |
| --- | --- | --- | --- | --- |
| provider-success | loopback Iroh endpoint + Wiremock HTTP | invitation 可解码且 route/expiry 正确 | passed、阶段耗时、事件、清理完成 | 3 秒 |
| provider-rejected | Wiremock 400 或 malformed 200 | 保留 `DirectoryRejected` / `DirectoryInvalidResponse` | 受控失败工件分类为 `product_invariant` | 3 秒 |
| provider-unavailable | 无监听 loopback port 或 Wiremock 503 | 保留本地 mint 或 `ServiceUnavailable` | 受控失败工件分类为 `environment_unavailable` | 3 秒 |
| process-recovery | 真实 SQLite、文件、当前测试二进制子进程 | exit 73 后 journal 恢复、资料计数不变、再次运行 UpToDate | 子进程阶段、固定边界事件、外部资源清理完成 | 20 秒 |
| cleanup-diagnostic | testkit 外部资源登记 | 不调用产品 | 受控 failed 工件分类为 `cleanup_failed` | 1 秒 |

受控失败工件测试在确认 JSON/摘要分类后正常结束，因此 runner 仍为绿色；产品拒绝与环境不可用本身仍按原测试语义断言。

## 4. 复用与模块边界

- 继续使用 `uc-testkit::Scenario`、`ScenarioFailure`、固定 seed、阶段和事件；不复制报告写入逻辑。
- 继续使用 rendezvous tests 现有 `MockServer`、loopback endpoint、adapter fixture 和错误类型。
- 继续使用 `profile_storage_upgrade_crash` 现有 fixture、子进程入口、退出码和持久恢复循环。
- testkit 只补一个登记外部资源清理结果的窄入口，使子进程等非 RAII 资源进入现有 `resources` 与 `cleanup` 字段。
- `uc-infra` 只增加 `uc-testkit` dev-dependency；生产依赖图不变。

## 5. 报告与隐私

- 工件根使用仓库相对的 `target/test-artifacts/real-dependencies`，运行目录包含进程号，避免并行覆盖。
- 工件只记录稳定场景名、固定 seed、阶段、事件类别、失败类别、资源标签和复现命令。
- 不记录 URL、端口、目录、设备标识、邀请内容、SQLite 内容或子进程环境值。
- nextest JUnit 与 testkit JSON 通过测试名关联；旧 `cargo test` 入口保持可用。

## 6. 实施顺序

### Phase 0：计划

- [x] 选择两个与 t-0010 无关的现有真实依赖测试边界。
- [x] 固定场景、预算、失败分类、复用边界和回退点。
- [x] 形成独立文档提交。

### Phase 1：最小 provider 场景

- [ ] 先让一个正常 invitation adapter 场景生成 passed 工件。
- [ ] 保留原断言并验证资源与摘要内容。

### Phase 2：失败诊断与 process 场景

- [ ] 增加拒绝/无效响应和暂时不可用的受控失败工件。
- [ ] 增加外部资源清理结果登记及 cleanup failure 自测。
- [ ] 将现有 profile storage upgrade crash/recovery 场景接入 Scenario。

### Phase 3：分组、双轨与稳定性

- [ ] nextest 将带 `stage3` 名称的 provider tests 映射到 `persistence-provider`，process test 映射到 `process`。
- [ ] 新入口与原精确测试入口双轨运行，不删除旧入口。
- [ ] 新场景整体至少重复 20 轮，无随机失败；记录实际耗时。

### Phase 4：完整验证与收口

- [ ] 运行 testkit、自身采用场景、相关旧测试、`uc-infra` 相关 integration test 与旧 cargo test 入口。
- [ ] 运行 workspace all-target check、fmt、Rust style、Engine repository、隐私和 diff check。
- [ ] 更新测试架构、架构圣经、本文和 Herdr 报告；创建范围清晰的本地原子提交，不推送。

## 7. 验收标准

- [ ] 正常、业务拒绝/无效响应、环境暂时不可用、进程恢复和清理均有真实执行证据。
- [ ] 报告明确出现 `product_invariant`、`environment_unavailable` 与 `cleanup_failed`。
- [ ] 每个采用场景有固定 seed、明确预算、稳定事件、复现命令和工件位置。
- [ ] 真实依赖场景不使用外网、固定 sleep、本机绝对路径或 t-0010 fixture。
- [ ] 新旧入口双轨且至少 20 轮稳定；旧测试与门禁保持不变。
- [ ] 生产依赖、公开接口、持久格式、协议和业务行为不变。
- [ ] 远程 CI、长期趋势、真实网络和设备明确记为跳过。

## 8. 风险与回退

- Wiremock 或 loopback endpoint 若受环境影响，报告为环境失败，不把测试放入 fast 组。
- process 场景若超过预算，先记录阶段耗时；不删除持久恢复断言或改用固定 sleep。
- 外部资源登记不得拥有或清理资源，只记录实际 driver 结果；资源关闭仍由原测试负责。
- 任一采用点可通过移除 Scenario 包装和 dev-dependency 独立回退，原测试逻辑保持可运行。

## 9. 进度记录

| 日期 | 阶段 | 结果 |
| --- | --- | --- |
| 2026-09-22 | Phase 0 | 完成真实依赖与独立进程场景选择；确认只需 testkit 外部资源记录能力，不需生产接口。 |
