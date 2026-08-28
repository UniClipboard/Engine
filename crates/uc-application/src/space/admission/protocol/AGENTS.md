# Space Admission Protocol 维护规则

本目录只向调用方提供单一 `SpaceAdmissionProtocol`。它负责一次 Space 加入从开始、恢复、消息处理到最终结果的完整流程，内部按 Joiner、Sponsor 和 Recovery 三个角色集中各自知识。

## 内部负责人

- `JoinerAdmissionService` 负责开始加入、处理 Candidate 以及后续 Joiner 推进；它持有设置、开始材料、开始状态、Joiner 候选准备和成功后的维护唤醒能力。
- `SponsorAdmissionService` 负责处理 JoinRequest 以及后续 Sponsor 推进；它持有 Sponsor 状态和候选准备能力。
- `AdmissionRecoveryService` 负责扫描待恢复记录、建立或恢复连接、交换消息和保存恢复推进；它持有恢复状态和 transport。
- `SpaceAdmissionProtocol` 只选择一个完整角色动作并执行 profile 级串行约束。三个内部负责人不得从 `protocol` 模块外取得，也不得成为调用方需要编排的步骤入口。
- 一个 Port 由对其业务结果负责的内部负责人持有。不得为缩短构造参数把无关能力集中到 `SpaceAdmissionProtocol` 或新增无生命周期职责的 `AdmissionRuntime`。

## 按业务动作组织

- 子目录按完整业务动作命名，例如 `start_join/`、`recover_pending/` 和 `handle_authenticated_message/`，不得按 `models/`、`errors/` 或 `ports/` 这类技术类别建立横向总目录。
- 每个业务动作在自己的目录内管理实现、模型、外部能力、错误和测试。通常分别放在 `execute.rs`、`model.rs`、`ports.rs` 和 `tests.rs`。
- 业务动作目录不是对外负责人，也不得向调用方公开步骤式入口。动作实现放到对应内部负责人，总入口只委托一个完整动作；调用方仍只使用 `SpaceAdmissionProtocol` 的完整方法。

## 公共内容门槛

- 只有两个或更多业务动作已经使用，并且业务含义确实相同的内容，才允许提升到 `protocol/` 根目录。
- 不得因为“以后可能复用”提前创建公共模型、公共错误或公共能力。
- 底层表示相同但用途不同的凭证、错误或状态视图仍应留在各自动作中，避免被错误交叉使用。
- `protocol.rs` 只保存三个内部负责人和跨动作执行约束；`joiner.rs`、`sponsor.rs`、`recovery.rs` 只保存各自能力；`mod.rs` 只负责模块声明和必要导出。
- 测试支撑只有在多个业务动作共同使用时才能放在根目录；单一动作的测试资料留在该动作目录。

## 能力与错误

- 外部能力由需要它的业务动作定义，Infra 提供实现，Engine 负责组装。Application 不得引用具体网络、数据库或密码库类型。
- 一个能力应隐藏该动作需要的完整外部行为，不得暴露成员账本内部字段、版本、历史摘要或步骤式存储方法。
- 外部能力产生的错误放在对应动作的 `ports.rs`；动作自己的输入、读取视图、完整变化和结果放在 `model.rs`。
- 只有稳定产品结果才能继续向 `space/admission` 或更外层导出，内部阶段和实现错误不得泄露给调用方。

## 修改检查

- 测试通过 `SpaceAdmissionProtocol` 的完整方法观察行为，不直接测试内部辅助函数。
- 新增业务动作前先写清楚完整结果、唯一调用、成功和失败以及重启恢复责任。
- 修改本目录时同步更新 `docs/architecture/architecture-bible.md` 的正文或维护记录。
