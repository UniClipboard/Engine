# 技术债跟踪

只记录已经有明确证据、负责人边界与退出条件的债务；模糊想法应先进入设计讨论。

| 项目 | 状态 | 依赖/退出条件 | 计划 |
| --- | --- | --- | --- |
| 本地产物统一准备 | 待实现 | 脚本、清单和三目标验证完成 | [计划](active/local-artifacts-preparation.md) |
| 双设备配对本机耗时仍约 5 秒 | 实施中 | 可信纯网络扣除门禁通过，预热后不少于 20 次的本机耗时 p95 小于 1 秒 | [计划](active/038-pairing-local-latency-budget.md) |
| 历史本地调试调用点尚未逐项分类 | 待实现，当前默认拒绝输出 | 每个生成清单项完成删除、安全本地化或明确保留，真实 sink 隐私检查通过 | [计划](active/039-local-debug-inventory-cleanup.md) |
| 本地日志并发完成记录偶发缺失 | 已在固定 rc.15 基线复现 | 运行期观测负责人核实并发记录到文件刷新的完整边界；`local_correlation` 独立进程连续运行不再出现 3/4 条记录 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| 十设备分叉场景中新增成员一直等待加入 | 已在固定 rc.15 基线复现 | 成员准入负责人关联 F7 中新成员的启动及邀请处理；完整测试在独立进程通过，不能扩大等待时间或改成刷新连接促成加入 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| iroh-blobs 定时回收延后运行时退出 | 已确认锁定依赖与旧版具有同一行为 | 依赖负责人修复或升级 `b33af91e` 的关闭流程；连续创建/关闭 Engine 后系统线程数返回基线，不能只检查任务登记数 | [发现记录](completed/2026-09-12-connection-liveness-and-recovery.md#11-执行记录) |
| 网络身份被替换后没有恢复方式 | 已能检测并阻止静默替换，恢复方式未定 | 先按只读现场证据判定应“重新配对引导”还是“身份更新事件”，另写计划实施；在此之前不得公开 `re_pair` 恢复动作 | [发现记录](completed/2026-09-23-inbound-peer-admission.md#s5b-恢复方式待定不在本计划实施范围) |
| 已有空间设备的网络身份文件在两次运行之间消失 | 原因未查明（`dev` 现场一次） | 资料与身份存储负责人查明删除来源（疑似开发构建未设 `UC_PROFILE` 落到 `dev` 资料目录）；复现后修复并保留诊断 | [发现记录](completed/2026-09-23-inbound-peer-admission.md#实施记录) |
| Core 规则外泄、效果义务无约束力、存储格式与运行时依赖在 Core | 已盘点，修复未开始 | 清单逐项修复或登记为规范例外；Core 自动检查在 CI 阻断 | [计划](active/2026-09-23-core-boundary-remediation.md) |
| 新会话发布前的出站连接被本机拒绝，每次加入的最终确认多等约 1 秒 | 已定位，候选方案因暴露重启补传问题暂缓 | 先修复同身份重启后握手超时及投递恢复只尝试一次的问题；`interrupted_file_transfer_recovers_after_receiver_process_restart` 连续 10 次通过，且最终确认首连不再 `locally_rejected` | [发现记录](../../.planning/2026-09-26-session-outbound-before-publish/findings.md) |

关闭项目时记录验证证据，更新稳定文档，并将对应计划移入 `completed/`。
