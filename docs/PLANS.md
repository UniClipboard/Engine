# 执行计划

执行计划记录尚需推进的具体工作和已经关闭的实施历史。它们不是当前架构事实；计划完成后，
稳定结论必须回写设计文档或 ADR，然后移入 `completed/`。

## 生命周期

```text
提议/实施/阻塞 → exec-plans/active/
                 ↓ 完成或明确被取代
                exec-plans/completed/
```

- [进行中的计划](exec-plans/active/)
- [已完成或被取代的计划](exec-plans/completed/)
- [技术债跟踪](exec-plans/tech-debt-tracker.md)

每份 active plan 必须写明状态、完整负责人、唯一调用、成功/失败结果、恢复责任和验收条件。
未执行的设备或发布矩阵项只能标为“跳过”。

## 当前专项计划

- Issue #34 第八切片：[反向进度与发送侧状态投影执行计划](exec-plans/completed/2026-10-24-transfer-progress-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-24-transfer-progress-connectivity-spec.md)及[验收结论](exec-plans/completed/2026-10-24-transfer-progress-connectivity-feasibility.md#最终候选ci与交付结论)。实施与验收结算完成，同head主CI13项及Windows双架构通过；物理挂起／设备后台按用户要求跳过，历史旧端Relay／成员F2间歇失败与远端日志丢弃边界保留。未合并或发布stack，不关闭整个#34。

- Issue #34 第七切片：[文件／blob 受管传输执行计划](exec-plans/active/2026-10-21-blob-connectivity-plan.md)，对应[实现规格](exec-plans/active/2026-10-21-blob-connectivity-spec.md)。状态为阶段 0 前置证明完成、阶段 1／2 完整结算与阶段 3 手动选择重试验收推进：公开底层 SDK／Uni、原始重试材料、当前授权、两个冻结旧端 Direct／Relay 资料矩阵及协议资源基线通过，见[复现与方案](exec-plans/active/2026-10-21-blob-connectivity-feasibility.md)。目标包含单任务取消隔离、有限调度与重试、独立文件部分失败及手动重试，保持文件夹完整发布和旧版文件互通。SDK 接入及原始项重试材料须先通过阶段 0，反向进度完整迁移留在第八片。

- Issue #34 第六切片：[设备可达性、准入与存活检查执行计划](exec-plans/completed/2026-10-14-peer-reachability-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-14-peer-reachability-connectivity-spec.md)。状态为完成；完整 Application presence owner、独立 Uni manager、3N 长期保留与暂态额度分离、本机读取故障来源和显示语义落地；本地完整回归、旧新双角色 Direct／Relay 及全部必需平台 CI 通过，真机与额外历史矩阵跳过；文件／blob／进度等剩余范围继续保留。

- Issue #34 第五切片：[当前剪贴板状态与按需拉取执行计划](exec-plans/completed/2026-10-14-active-clipboard-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-14-active-clipboard-connectivity-spec.md)。状态为完成，本地验收与代码提交线上必需检查通过；四向 Uni 迁移、每次网络交换 10 秒、每轮 4 目标、最新待发状态与独立容量；保留原恢复语义和文件本体链路。

- Issue #34 第四切片：[成员历史完整迁移与连接复用执行计划](exec-plans/active/2026-10-12-membership-history-connectivity-plan.md)，对应[实现规格](exec-plans/active/2026-10-12-membership-history-connectivity-spec.md)。状态为本地实现与验收完成、待线上检查；成员历史独占连接池、新多流 ALPN 与旧 `/4` fallback、每方向一条、当前成员 30 秒空闲保留；不新增批量协议。

- Issue #34 第三切片：[组更新收发与主动恢复执行计划](exec-plans/completed/2026-10-11-group-update-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-11-group-update-connectivity-spec.md)。状态为完成；完整 Uni 收发、统一应用与 /4 主动逐页恢复落地，最终 workspace、成员、真实 SQLite/MLS 强杀断点、独立宿主和固定 main 双方向互通通过；物理设备与跨网络矩阵跳过，不新增批量协议，不关闭整个 #34。
- Issue #34 第二切片：[普通剪贴板连接边界执行计划](exec-plans/completed/2026-10-11-clipboard-connectivity-plan.md)，对应[实现规格](exec-plans/completed/2026-10-11-clipboard-connectivity-spec.md)。状态为完成；切片验收及根门禁通过，完整 workspace 的成员 G2 一项失败与隔离通过均保留记录，原因待查；平台与历史发布矩阵明确跳过。

- [网络栈 submodule 交付](exec-plans/completed/2026-10-24-network-stack-submodule.md)
