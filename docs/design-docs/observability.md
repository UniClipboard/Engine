# 运行期观测装配

## 唯一范式

跨层业务链路的持续计时、结果分类和阶段诊断通过 `uc-engine` 组装层的 port decorator 实现。
Application 业务调用点不得散布 `Instant`、`tracing` 或手工阶段记录函数。

Engine 私有 decorator 放在 `crates/uc-engine/src/assembly/observability/<domain>.rs`；领域规模增长后
可以拆成同名子目录。每个观测 seam 拥有自己的具体 `ObservedX`、私有操作枚举、明确 observation policy
和结构化事件 schema。

## 装配与传播

- Application 拥有真实消费者所需的 adapter bundle；Engine 在每个真实装配 seam 提供一个主要入口，按值接收并返回
  同一 bundle，集中包装后再交给 Application。
- 宽泛业务领域若跨越多个真实装配时点，可以拥有多个 seam-specific 入口；不得为凑成一次调用而延迟 Application
  构造、保留可绕过 decorator 的 raw clone，或引入跨阶段 registry。
- 返回另一个 port 的能力必须继续包装返回值；例如 transport 建链成功后继续包装 authenticated exchange。
- 只复用“在 Engine 组装边界装饰 port”的结构，不创建跨领域万能 `Observed<T>`、字符串 phase 注册表或通用阶段记录函数。

Space 当前以 `SpaceAdmissionAdapters` 与 `SpaceMembershipAdapters` 两个 Application-owned bundle 作为真实 seam。
Admission 保留 recovery、认证 transport、Sponsor 状态与 settlement、Joiner candidate、activation 与重新配对状态的
调用级观测；transition adapter 内部阶段不再由 Engine 嵌套计时。Membership 观测
ledger load/commit、history exchange、restricted delivery、group update dispatch，以及 branch recovery 的
group-info/external-commit 两步。ledger load 的成功调用仅在耗时达到 50ms 时记录，所有失败均记录；其余批准方法
记录全部调用。两组事件分别只写入 `admission.performance` 与 `membership.performance`。

Profile V1/V2 到 V3 的启动升级由组装层围绕 Infra 深模块的一次完整调用记录，固定写入 `storage.performance`；事件只含
`profile_storage_upgrade` 操作名、耗时、结果或稳定失败类别，不记录 profile、Space、路径、错误文本或升级内容。

## 行为与隐私

- Decorator 只观察依赖调用，不改变业务结果、错误 source、重试、持久化或通信顺序。
- 观测失败不得影响业务结果。
- operation、outcome 与分类字段来自固定枚举或固定映射；失败事件必须包含 `error_kind`。
- 准入恢复读取记录 `startup`、`resume`、`periodic`、`state_changed` 或 `peer_online` 固定触发分类；
  `peer_online` 不附带设备身份。
- 事件只包含批准的稳定分类、计数与耗时；不记录错误文本或任何业务/身份/凭据/路径负载。

## 测试

新增或修改 decorator 时至少覆盖成功、失败的 policy 决策，适用的空结果降噪，以及返回 port 的继续包装；
同时检查 Application 调用点没有重新引入计时或 tracing。
