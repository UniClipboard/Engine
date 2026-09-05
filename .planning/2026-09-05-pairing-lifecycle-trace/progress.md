# Progress: Pairing Lifecycle Trace

## 可读动作与准确时间修复

- 用户授权修复：17 节点只有两个名字，以及建链完成后时间条仍被后台任务延长。
- 命名红测：五种动作只有两个不同名称；已确认失败。
- 耗时红测：连接 span 为 3458ms，与动作完成日志不一致；已确认失败。
- 源码原因：noq Connecting 创建 ConnectionDriver 时保存 `Span::current()`；默认过滤掉库内 span 后继承了调用者的认证 span。
- 修复：固定动作显示名与分类分离；Application 在现有消息处理位置提供动作含义；底层建链隔离长期驱动的 tracing 上下文。
- 新 E2E 逐轮核对发送、接收、处理和首次/恢复认证名称，检查建链 span 与完成日志误差不超过 20ms。
- 两条红测转绿；Sponsor 名称更新须在编码前处理，已用固定内部字段转换并严格校验、删除后发送。
- 完整合同 50、运行模块 42、Application 750、准入网络 13 项通过；正常与重启 E2E 通过。
- 新真实 trace：`f76a32a127260291000eb004a42f702f`，17 节点、17 完成日志；12 个不同显示名。认证 79.580ms，重连 3.237/4.454/3.655ms，日志 79/3/4/3ms。
- 浏览器截图已实际查看：四轮目的、发送/接收/处理可读，时间条不再延长；全仓所有目标编译通过。
- 最终 metadata、fmt、架构与隐私门禁通过；新增负向检查拒绝 Engine 接触固定业务动作语义。本轮未提交或推送。

## 2026-09-05

### Phase 0

- **Status:** complete
- 已确认当前 Jaeger 中最多三个 span 是实际建模结果，不是页面显示问题。
- 已记录同一 flow 下 4 条建链 trace 与 4 条三层交换 trace。
- 已冻结初步方向：正常连续配对一个 trace，重试/重启新 trace 但同 flow，Engine 不感知内部步骤。
- 已读取准入目录局部规则，确认 `SpaceAdmissionProtocol` 是唯一入口并提供 profile 级串行约束。
- 已定位现有 E2E 中需要反转的旧合同：client 无 parent、至少四个 TraceId。
- 已确定句柄归 `JoinerAdmissionService`，不改变协议根结构；start 持久成功后创建，取消请求不提前结束。
- 已冻结结束边界：Settled 成功关闭；延期、拒绝、升级阻塞和 recovery-required 结束当前 trace；下次重试同 flow 新 trace。

### Phase 1

- **Status:** complete
- 已新增独立的真实双设备 trace 结构测试，只要求未中断配对的 TraceId 唯一，不混入一秒性能门槛。
- 红测精确执行 1 项并按预期失败：实际 4 个 TraceId，目标 1 个。
- 首版句柄实现把结果收敛为 2 个 TraceId，精确分布 3+1，确认断点是本机激活后的 Space 运行对象重建。
- 已定位跨 session rebuild 的现有稳定边界：`SessionSupervisor` 持有的 Application assembly；下一步确认其构造接口能否由 Application 自己创建并透传不透明 registry。
- 已确认 `ApplicationAssembly::assemble_network` 可在 Application 内部把同一私有 registry 传给每次新建的 Space/Joiner 对象，无需修改 Engine 接口。
- 稳定 registry 接线后，正常配对 E2E 已从 4 个 TraceId 收敛为 1 个并通过。
- 现有重启 E2E 按预期红：旧 helper 只接受 client root，需改为识别生命周期 parent 后验证新 trace。
- 用户校正所有权：根属于完整 Space Admission，不属于 Joiner；后续实现改为 admission 中性 registry 与 local root。

## Test Results

| Test | Expected | Actual | Status |
| --- | --- | --- | --- |
| 未中断双设备配对 trace | 1 个 TraceId | 4 个 TraceId | RED |
| 首版生命周期句柄 | 1 个 TraceId | 2 个 TraceId | RED，继续定位 |
| 稳定 Application registry | 1 个 TraceId | 1 个 TraceId | GREEN |
| 重启 trace 旧证据模型 | 重启前可找到已完成子树 | 0 条，旧 helper 拒绝非 root client | RED |
| 生命周期 root 发送前检查 | 根和两个子节点均通过 | root 被 `flow scope` 拒绝 | RED |
| 未中断双设备完整结构 | 唯一 root、一个 TraceId、四轮三层交换 | 全部满足 | GREEN |
| Engine 重启边界 | 新 TraceId、同 flow | 全部满足 | GREEN |
| admission 结果边界红测 | 延期/拒绝/升级后活动根为 0 | 18 项中 5 项失败，均为实际 1 | RED |
| 未完成句柄释放 | 1 条 deferred 完成记录 | 0 条日志 | RED |
| 新加入覆盖旧加入 | 只保留新根 | 活动根从 1 变成 2 | RED |
| admission 结果边界 | 18 项全部通过 | 18 项全部通过 | GREEN |
| 未完成句柄释放 | deferred 根和完成记录 | 通过 | GREEN |
| 覆盖旧加入 | 旧根结束，只保留新根 | 通过 | GREEN |
| 三设备旧成员更新 | 三方收敛且旧成员可发送 | 通过 | GREEN |

### Phase 3

- **Status:** complete
- 正常结清、取消、拒绝、升级阻塞、延期、状态失败和旧加入覆盖均能结束对应 root。
- Engine 重启建立新 trace 并沿用同一 flow；未完成句柄最后释放时记录 deferred。
- 三设备真实测试通过，旧成员最终看到新成员并能向其发送内容。

### Phase 4

- **Status:** complete
- 开始稳定文档、本地 Jaeger 页面和全量门禁收口。
- 本地 Collector 0.160.0 重载新配置，公开双设备配对通过。
- Jaeger API 显示唯一 admission trace 含 17 个 span：1 local、8 Joiner、8 Sponsor；完成日志 17 条且无 flow 泄露。
- 实际页面截图确认 Total Spans 17、Depth 4、Duration 5.3s，四次交换均位于同一树。
- 架构门禁旧规则按预期红，已改为检查中性 registry 只由 Application admission owner 持有，Engine/Infra 不得引用。
- 完整合同 49 项、运行模块 40 项、Application 750 项通过。
- 三轮交错性能中位数：远程关闭约 5.59s、健康接收端约 4.84s；新增 root 未造成可见回退，不将样本解释为性能提升。
- 依赖锁定、全仓所有目标编译、格式、架构门禁和差异检查全部通过。
- 本地 Collector/Jaeger 保持运行；截图临时目录已移动到废纸篓，原路径不存在。

## Error Log

| Error | Attempt | Resolution |
| --- | --- | --- |
| E2E assertion left=4, right=1 | 1 | 预期红测；不是编译、环境或超时错误 |
| observation scope 对 Recovery 不可见 | 1 | 改为仅 protocol 内可见，保持对外隐藏 |
| observation 类型的可见范围仍过窄 | 2 | 类型本身同步限定为 protocol 内可见 |
| 首版实现仍分成两条 trace | 1 | 扩充测试输出，按真实分界修正 |
| registry 初次提升后 Application check 编译失败 | 1 | 修正 crate 内重导出及测试夹具参数位置 |
| 第三次遇到嵌套可见性错误 | 3 | 重构归属：Space 根私有上下文，Joiner 只持有 Arc |
| 两处旧类型路径未更新 | 1 | 当时先改用 Space 根重导出；最终类型已收口为中性 admission registry |
| 重启测试找不到 pre-restart trace | 1 | 预期结构变化；更新 helper，不改变重启语义 |
| 完整 root 断言等待超时 | 1 | 定位为设备/Collector 仍拒绝 internal root 上的 flow |
| Collector privacy 4/5 | 1 | 配置已更新，测试中的第二份旧 role rule 同步修正 |
| Cargo 未进入编译，sccache 返回 Operation not permitted | 1 | 遵守新规则，不绕过缓存；改查可写缓存运行方式 |
| 任务专用 sccache server 无法启动 | 2 | sandbox 禁止 server process；转查无后台模式 |
| `SCCACHE_NO_DAEMON=1` 仍被拒绝 | 3 | Cargo 环境阻塞已确认；不清空 wrapper，不重复失败 |
| `cargo fmt --check` 仅报告本次文件格式差异 | 1 | 按格式器输出手工调整 |
| `SCCACHE_IGNORE_SERVER_IO_ERROR=1` 仍被拒绝 | 4 | 确认外部权限阻塞；停止 Cargo 尝试 |
| 用户继续后默认 Cargo 仍为 Operation not permitted | 5 | 权限未恢复；最后尝试可执行临时副本，不关闭缓存 |
| 新 Drop 红测因测试辅助类型不匹配未执行 | 1 | 修正 AnyValue 字符串提取后重跑 |
| supersede 测试错误地复用了相同 admission id | 1 | 新旧夹具改用 0x11/0x21 两个合法固定编号 |
| `rm -rf` 清理截图目录被拒绝 | 1 | 未删除任何内容；改用精确路径移动到废纸篓 |

### Current verified boundary

- 中性 admission 根的目录、命名、角色和 Collector 规则已调整。
- `cargo fmt --all -- --check` 通过。
- Collector privacy 5/5 通过，两份配置保持一致。
- 正常单 trace 与重启同 flow 的 E2E 曾在 Joiner root 版本通过；改为中性 local root 后尚未获得 Cargo 复验，不能记为通过。
- 延期/拒绝/升级/取消的活动句柄红测已写入，但因 Cargo 阻塞尚未执行，生产收口未开始。
