# 已确认事实

- 基线 Engine HEAD `fe55e543`，本轮开始工作区干净。
- 四配置 A 选择远端 A/B/C 后，ledger 仍携带旧 RemoveDevice(C)/Activated；C 仍有效，旧事件不在当前历史。清理据此删除 C 投影，设备组查询 Unavailable。
- 五配置 A→D 离线增量重放：传入记录签名通过，主线终点匹配；D 多保留旧 A→移除 D 的旁支事件，导致完整归档摘要不等而 InvalidPersistedHistory。
- 五配置 A/E 互为 Invalid；E→A 当前合法决定可以离线成功应用，但周期调度跳过 Invalid。原始 A/E 首次拒绝的具体报文尚未精确关联，不把 A→D 证据冒充完整原始因果链。
- 既有冻结资料与只读工具在同级 Desktop `.cache/device-group-e2e/engine-investigation-20260907/`，调查文档在同级 Desktop `.planning/device-group-choice-gui/engine-blockers.md`。

## 责任与设计约束

Application 负责完整换组与恢复语义，Core 负责历史规则；Infra 只实现原子持久化、投影及传输。旧历史应保留审计事实，但不再拥有新组执行资格。
修复需恢复已经缺失的投影，不能只保证新运行不会再删；恢复任务也不能覆盖或撤销用户明确的选择。

## 本轮设计核查

- 当前清理 port 由 Infra 自行加载 ledger 并按 Activated 移除派生删除名单，既越过业务所有权，又无法修复既有缺失投影。需要替换成 Application 派生完整当前资料计划、Infra 原子应用的能力。
- V3 transition 目前一次写完整目标关系/安全资料，但 ledger 从 source clone；应由 Application 统一构造目标 ledger，区分历史审计和当前可执行效果，不在 Infra 重建 peer 业务状态。
- 当前 `current_position` 是完整归档摘要，属于签名准入与已存格式的一部分，不宜随意改成名单或 head 哈希。交换证明必须明确区分归档与被证明的更新，不能只删掉摘要校验。

## 历史交换切换方案

保持已签 V2 历史和 `current_position` 归档含义不变；V4 增量携带发送方归档的有界记录索引，仅在第一帧携带。接收方在临时验证视图中按索引重建发送方历史并完整验签/校验摘要，实际本机历史则保留自己的旁支和决定。缺失证明资料或本机位置已变化进入完整证据核对，不转为永久 Invalid。
网络使用新的显式 wire 版本；旧在途分页仅在持久化迁移时解码并退役，不保留旧的增量执行路径。Ledger V4 迁移保留已签历史、决定与效果日志，重建在途传输状态。
已有 Invalid 状态的当前已验证成员通过完整证据恢复，内容权限仍保持关闭，只有成功验证相同选中分支后才能恢复关系。
