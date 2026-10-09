# 离开空间后的实例恢复

## 目标
`Operation::FactoryResetSpace` 成功后，同一个 Engine 实例处于可查询、可再次创建或加入空间的稳定状态；
无法恢复时返回结构化错误，不把不可用实例报告成成功。关联下游诊断线程 t-0142。

## 完整负责人
`RecoverableRuntime`（`crates/uc-engine/src/runtime/profile_recovery.rs`）：已拥有“当前 ProductionRuntime”的装配与替换，
离开空间的完整流程（重置、释放旧运行期、按启动决策重建）由它统一负责；Engine 外壳与绑定不参与。

## 调用方唯一动作
`execute(FactoryResetSpace)`：
- Ok(SpaceFactoryReset)：新运行期已就绪，可立即查询/创建/加入。
- Err(重置失败码)：数据未全部清除，同一实例可重试同一动作（阶段持久、可续做）。
- Err(FACTORY_RESET_RESTART_REQUIRED)：数据已清除但新运行期无法装配；`ProfileRecoveryChanged.restart_required=true`，
  之后业务操作返回同一错误码，宿主须重启 Engine。

## 阶段
1. [x] 读代码、确认根因
2. [ ] 隔离复现测试（失败）
3. [ ] 宿主能力可复用 + 重建
4. [ ] 失败路径测试
5. [ ] 绑定契约测试、文档
6. [ ] 验证与报告
