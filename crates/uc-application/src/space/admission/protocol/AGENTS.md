# Space Admission Protocol 维护规则

本目录只向调用方提供单一 `SpaceAdmissionProtocol`。它负责一次 Space 加入从开始、恢复、消息处理到最终结果的完整流程，内部按 Joiner、Sponsor 和 Recovery 三个角色集中各自知识。

## 内部负责人

- `joiner/` 包含 `JoinerAdmissionService`、开始加入、处理 Candidate、Commit、Complete、执行本机激活和处理 Settled；设置、开始材料、开始状态、各阶段准备、本机激活状态与执行、成功后的维护唤醒能力都留在本目录。
- `sponsor/` 包含 `SponsorAdmissionService`、统一认证消息分发、处理 JoinRequest、Prepared、Applied 和 CompleteAck；角色内共享状态与各阶段回复准备能力都留在本目录。
- `recovery/` 包含 `AdmissionRecoveryService`、扫描待恢复记录、建立或恢复连接、交换消息和保存恢复推进；恢复状态、transport、触发原因和恢复报告都留在本目录。
- `SpaceAdmissionProtocol` 只选择一个完整角色动作并执行 profile 级串行约束。三个内部负责人不得从 `protocol` 模块外取得，也不得成为调用方需要编排的步骤入口。
- 一个 Port 由对其业务结果负责的内部负责人持有。不得为缩短构造参数把无关能力集中到 `SpaceAdmissionProtocol` 或新增无生命周期职责的 `AdmissionRuntime`。

## 先按角色，再按业务动作组织

- 协议根目录只按 `joiner/`、`sponsor/` 和 `recovery/` 三个角色划分，不在根目录平铺角色文件或业务动作目录。
- 每个角色内部再按完整业务动作组织，例如 `joiner/start_join/`、`joiner/handle_candidate/`、`joiner/activate_complete/`、`sponsor/handle_join_request/`、`sponsor/handle_complete_ack/` 和 `recovery/recover_pending/`。
- 每个业务动作在自己的目录内管理实现、模型、外部能力、错误和测试。通常分别放在 `execute.rs`、`model.rs`、`ports.rs` 和 `tests.rs`；只有角色内两个或更多动作已经使用且含义相同的内容，才提升到该角色的 `mod.rs`。
- 业务动作目录不是对外负责人，也不得向调用方公开步骤式入口。动作实现放到对应内部负责人，总入口只委托一个完整动作；调用方仍只使用 `SpaceAdmissionProtocol` 的完整方法。

## 公共内容门槛

- 只有两个或更多角色已经使用，并且业务含义确实相同的内容，才允许提升到 `protocol/` 根目录。
- 不得因为“以后可能复用”提前创建公共模型、公共错误或公共能力。
- 底层表示相同但用途不同的凭证、错误或状态视图仍应留在各自动作中，避免被错误交叉使用。
- `protocol.rs` 只保存三个内部负责人和跨动作执行约束；三个角色的 `mod.rs` 保存各自负责人、角色内动作和必要导出；协议根 `mod.rs` 只负责私有模块声明和稳定契约转出。
- 测试支撑只有在多个业务动作共同使用时才能放在根目录；单一动作的测试资料留在该动作目录。

## 能力与错误

- 外部能力由需要它的业务动作定义，Infra 提供实现，Engine 负责组装。Application 不得引用具体网络、数据库或密码库类型。
- 一个能力应隐藏该动作需要的完整外部行为，不得暴露成员账本内部字段、版本、历史摘要或步骤式存储方法。
- Application 的准入生产代码只能接收 `JoinerAdmission` 或 `SponsorAdmission` 角色能力对象及对应变化结果，不得接收或引用完整 `SpaceAdmissionAggregate`。完整记录只供 Core 内部规则与 Infra 密文编解码使用。
- Joiner J0 生成的本机私密材料必须与 JoinRequest 在同一 Initiated 记录内加密保存；认证期间保留，Candidate 准备只能借用，Candidate 状态提交后由 staged target input 替代。不得把私密材料交给 transport、写日志或在状态提交前单独删除。
- 云端短码是一次性查询凭证。Application 必须先保存 Ready，再提交 Started 并从持久状态删除短码，之后才能调用解析能力一次。响应必须先保存完整邀请再连接 Sponsor；Started 重启、超时、响应丢失或保存失败均不得重用短码，必须结束并要求新邀请。
- 外部能力产生的错误放在对应动作的 `error.rs`，稳定分类必须携带 `#[source] anyhow::Error`，通过构造器和 `From` + `?` 保留来源与回溯；不得用无来源 unit variant 或字符串化 `map_err` 抹平错误链。动作自己的输入、读取视图、完整变化和结果放在 `model.rs`。
- 只有稳定产品结果才能继续向 `space/admission` 或更外层导出，内部阶段和实现错误不得泄露给调用方。

## 修改检查

- 测试通过 `SpaceAdmissionProtocol` 的完整方法观察行为，不直接测试内部辅助函数。
- 新增业务动作前先写清楚完整结果、唯一调用、成功和失败以及重启恢复责任。
- 修改本目录时同步更新 `docs/architecture/architecture-bible.md` 的正文或维护记录。
