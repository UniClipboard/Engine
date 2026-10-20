# 发现
- 开工 origin/main = b6fa0ce6b2efd1c1764e76c0a76537bdeead4e1b，较审计基线增加 profile 日志目录保留修复（PR #184）。
- 当前分支 feat/active-clipboard-connectivity，HEAD 6bc4d791；用户已有两份规格/计划未跟踪，PLANS 和 active index 已修改。
- a07 = f6f305d9689e4e79e7ab6d0e4921061f9416e4a6；既有 build-upgrade-anchors 可使用同一宿主源码与共享 target，旧生产库无补丁。
- 现有 D2 sequential_upgrade 从真实旧旧配对再单端升级，可用其真实故障证据定位前置；当前网络脚本只有现场混合配对。
- coordinator 常量：MAX_CONCURRENT=4、DIAL_BUDGET=20s、HEALTH_CHECK_INTERVAL=10s、SCOPE_BUDGET=1s、SCOPE_RECHECK=60s、COALESCE=250ms、MIN_START_INTERVAL=1s。
- 原 codec：/2 优先、/1 fallback；准入 01/01/02；challenge 17 字节，10/11/12 加 16 字节 nonce；read_to_end(17) 等待 FIN。
- 生产装配在 sync_engine.rs:177 -> node.install_peer_reachability；native adapter 持 observations、inbound/outbound、gate、generation、accepting，gate 复用 admitted_connections。消费者包括 roster、clipboard active/dispatch/recovery、membership activation/trust、其他 connectivity backend；替换应共享同一 owner。
- 已确认 native 准入首流无总额，重检及 liveness authorize().is_ok() 混同读取故障和拒绝；本阶段只记录，不修改生产。
- 本机资格来源还有一处必修断点：PeerAccess 的 admission_error 将 MembershipLedgerError 变成无 source 的 Core PeerAdmissionError::Unavailable/InvalidState。需要补带来源 variant；否则即使 transport 保留上层错误也无法追溯 SQLite。该修改只修复既有资格错误转换，不增加资格来源。
- v1 新能力 RetainedSingleExchange 的 public Uni 生命周期测试 red（类型不存在）→green；Application 全目标编译通过。
- pending 入口过载 native 失败用例实测失败（17th 未在 1s 内关闭），原始首次失败已保存。
- 当前实现回归网络 E10 Relay-only 原三秒恢复 1 轮通过（14.954s 场景总时长，不是恢复耗时）；不能冒充 20 轮最终验收。
- 重点待验证：真实本机故障保留显示（包括存活流失败后的闭合观察）、Owner actor join 失败的持久 source、coordinator scope 本地读取取消后资源计数、最后一条连接关闭与检查新成功交错、第三条入站替换及跨代次 late 原子提升。
- 改动后的 workspace check 曾通过，但后续预算传播/观测名称等仍需重跑最终门禁。
- 原 native adapter 与协议目前仍存在，Engine 生产已接新 owner；必须迁移有价值测试并删除原生业务及旧 builder 构造，不能把当前状态写为已完成。
- 原 Actor private verify 被 style gate 判为 Core trait 纯转调，已改为同步完整 submit（冻结期限、真实 4 许可、监督 task）加 trait 内明确接收结果/错误边界；style gate 通过。
- 当前成员 owner 的公开有效快照通常命中 published(generation)，真实 SQLite 故障验收必须令其走真实重新读取路径；单纯删表后查询热缓存不能宣称覆盖本机读取故障。
- 原 native adapter 的大部分测试使用内部状态表、成员伪表或强制插入 Online；迁移必须改成完整 owner/Uni/native 网络可观察行为，不能把旧实现留下仅让旧测试通过。

- rc.15 旧 Sponsor 的 address.used 为 direct_count=1/relay_count=0，旧 Joiner 保有 Sponsor relay 路由。将“新端接收”机械固定到升级 Joiner 会把历史启动地址缺陷误当成 presence 迁移失败；最终矩阵 receive 升级 Sponsor，initiate 升级 Joiner，均从独立原资料生成，禁止人工补地址。

### 最终待提交差异审查（2026-10-20）
- 原生 incoming gate 状态、setter 和 Server-side 拒绝均限于 test／test-util；Engine 动作限于 dev-tools，稳定 facade／绑定无新入口。真实原生连接与清除回归、独立进程主动方向证明共同覆盖边界。
- 同一宿主源通过独立 inbound-network-faults 能力适配旧库；冻结 main 显式关闭默认特性，只启用其既有 current-engine，旧库及非宿主锁包保持冻结。
- Relay 三秒门从本轮原生协议成功事件发生时间计算，且要求当前 TCP；旧事件、断开后的成功历史及纯 TCP 探测均不满足门起点。日志刷新／读取成本计入原门限，不以 Online 为起点。
- rc.15 关闭重开基线由旧库读取原 Space／传输身份并完成旧旧精确内容；不补路由、不伪造资格。角色限制在故障前清空，恢复期无刷新／预发送。
- 两项本地生命周期 fixture 用既有配置关闭公网 Relay，原超时及资源阈值不变；业务生产行为不变，真实中转验收独立完成。

- 2026-10-21 成员失败待证假设（按优先级）：① B 重启换端口后 C 仍使用旧路由，预测握手持续超时且端口／发现观察未更新；② 独立网络 dummy 默认路由或 mDNS 条件，预测同一单项在宿主与 namespace 有可重复差异；③ 全集负载／时序，预测串行单项反复通过而同组并发才失败。新增 incoming gate 默认空集合，失败测试未设置该限制，当前没有证据支持它导致拒绝。需先同二进制复现，不提前写根因。

## 最终确认重试根因

固定 B 重启端口不能消除缺失前序恢复的 C→B 确认失败。真实 SQLite 在第二次提前 ReceiverUnavailable 时，把原 30100ms 的重试推到 60200ms；native 三次早期失败进一步推到 120 秒后。提前的上线／历史机会不应惩罚原定重试。Core GroupUpdateRetryPolicy 保存该纯期限规则，仓储在原密文事务内合并，保留实际尝试计数及原退避上限；没有新持久格式、接口或期限放宽。

确定性 red／green、365 项 Core／存储回归、原生并发两项及完整成员 56 项共同确认修复；缺失前序在原生并发为 32 秒、全集为 38 秒，G4 从旧全集约 153 秒降到约 63 秒。完整 workspace 4305 项通过。当前仍须 11 格、最后根检查和最终提交九项 CI 结算，不以历史首轮通过替代。
