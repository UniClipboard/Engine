# Issue #34 总验收清单

状态：**结算中，议题保持开放**（2026-10-29）。本清单对应
[Issue #34](https://github.com/UniClipboard/Engine/issues/34) 的 Definition of Done 和补充分层合同，
不把切片合并、平台编译或重跑通过当作整项完成。

## 完整负责人和退出条件

各业务仍由其 Application 完整负责人执行，Engine 只组装现有完整能力；本计划只统一验收与交付状态。
调用方继续发起复制、发送、加入或恢复等完整动作，不拼装协议步骤。
成功以相应业务持久结果和真实内容读回为准；错误、重启和重试仍归各业务负责人。

总验收须逐项给出代码／运行证据、确切候选及覆盖范围，保持首次失败和跳过项。
第四、七、九片达到各自退出条件后才归档；真实设备与完整历史矩阵未跑时仍列为跳过。
本次不关闭远端 issue、不合并交付 PR、不发布或更改产品仓依赖。

## 切片交付状态

| 范围 | 当前状态 | 唯一证据入口 |
| --- | --- | --- |
| 1：发现与目录 | 已完成；公网解析与设备验证跳过 | [计划](../completed/2026-10-11-peer-discovery-slice.md)、[稳定合同](../../design-docs/peer-discovery.md) |
| 2：普通剪贴板 | 已完成；G2 历史间歇失败保留 | [计划](../completed/2026-10-11-clipboard-connectivity-plan.md) |
| 3：组更新 | 已完成；设备和跨网络扩展矩阵跳过 | [计划](../completed/2026-10-11-group-update-connectivity-plan.md) |
| 4：成员历史 | #180 最终同 head PR／Windows CI 成功，已结算归档 | [最终结算](../completed/2026-10-12-membership-history-connectivity-plan.md#最终线上验收与归档结算2026-10-29) |
| 5：当前剪贴板与按需拉取 | 已完成 | [计划](../completed/2026-10-14-active-clipboard-connectivity-plan.md) |
| 6：设备可达性 | 已完成；设备和额外历史矩阵跳过 | [计划](../completed/2026-10-14-peer-reachability-connectivity-plan.md) |
| 7：文件／blob | 后续#340统一候选补齐原安全阻塞，48项计划／13项验收已结算归档 | [计划](../completed/2026-10-21-blob-connectivity-plan.md) |
| 8：反向进度 | 已完成并归档；物理挂起／设备后台按原授权跳过 | [最终结算](../completed/2026-10-24-transfer-progress-connectivity-feasibility.md#最终候选ci与交付结论) |
| 9：成员分支恢复 | #345 已合并；原attempt 2网络24格已核验通过，Engine待结算；#350新候选网络在升级前旧旧Relay文件接收失败，仍未闭环 | [计划](2026-10-28-membership-branch-recovery-connectivity-plan.md#合并后最终-ci-结算2026-10-29) |
| Space 加入／续接 | #346 已合并并归档；旧发布二进制、Relay 和设备矩阵跳过 | [计划](../completed/2026-10-29-space-admission-connectivity-plan.md) |

## Definition of Done 逐项对应

“已有证据”表示所属切片已证明该范围，不表示当前候选全部回归完成；“待总审”必须补齐后才能关闭 #34。

| 编号 | 原要求 | 当前证据与结论 |
| --- | --- | --- |
| D01 | DiscoveryProvider 是正式抽象 | 已有证据：Application discovery 的 provider／manager 与第一片定向测试；[发现合同](../../design-docs/peer-discovery.md) |
| D02 | Cloud 包装为 provider | 已有证据：Infra CloudDiscoveryProvider 适配 DNS／PKARR，按需解析；真实公网成功解析仍跳过 |
| D03 | mDNS 发现已配对 LAN 设备 | 已有证据：实际 mDNS adapter 订阅／更新／快照与第一片独立进程 Direct 同步；不据此断言所有平台网络栈 |
| D04 | 同 peer 多来源并存 | 已有证据：`sources_coexist_update_and_expire_independently`，每来源独立额度 |
| D05 | TTL／生命周期正确 | 已有证据：原观察时间不因读取续期，来源结束／掉包／节点关闭不得永久保留或复活候选 |
| D06 | mDNS 消失不删除 Cloud | 已有证据：目录来源测试及真实 QUIC lookup 用例的 Cloud 候选保留断言；后者 Cloud 是测试 MemoryLookup |
| D07 | 发现仅作线索，连接后密码认证 | 已有证据：逻辑成员与认证公钥映射、未知／歧义／错 peer／撤销拒绝；不是由 mDNS 宣称成员身份 |
| D08 | PeerDirectory 是发现／连接边界 | 已有证据：生产 DirectoryAddressLookup 消费 DiscoveryManager，provider 不拨业务连接 |
| D09 | 全部出站通过统一管理 | 当前生产入口源码总审完成，见下表；包括 #345 恢复及 #346 非成员准入，运行验收仍待最终 CI，不按新增 trait 数量结项 |
| D10 | 全部入站经同一连接边界 | Router 的16个当前／兼容 ALPN 已逐类对应受管 handler，源码总审完成；最终回归仍待结算，不能用 presence 的健康连接代替内容验收 |
| D11 | 当前唯一生产 backend 为 Iroh | 已有证据：生产 Iroh backend；第二 backend 仅测试使用，不增加产品配置或 Nearby 实现 |
| D12 | Direct／NAT／Relay 路径由 Iroh 负责 | 已有证据：业务连接实际 selected path 和 Direct／强制 Relay 矩阵；不把候选存在当成送达，也不据局域网拓扑宣称全部 NAT 类型已测 |
| D13 | 业务／领域不依赖 Iroh 类型 | 源码与依赖总审完成：Core／Application／协议生产源码未引入 Iroh／noq 类型，架构门禁及负fixture通过；Core 注释中的历史类型说明不作为生产依赖 |
| D14 | 原剪贴板／文件／Space 行为保持 | 各切片 workspace、真实 SQLite／MLS、内容读回、取消与重启证据；#345 首轮未闭合步骤须补齐，历史失败保留 |
| D15 | 云发现故障时已配对 LAN 设备仍可发现并同步 | 已有“慢／失败 Cloud 不阻塞本地候选”、LAN-only 无 Cloud 装配、实际 Direct 同步等分层证据；总审核实尚缺排除持久地址缓存等替代路径的端到端证明，保持未完成 |
| D16 | 第二 provider／backend 不改业务即可接入 | 已有证据：发现测试 provider、第二内存 backend 经真实 Application 恢复 transport／receiver／wire 完成两次交换；签发方为替身，不作真实 MLS／网络证明 |

## 当前生产入口总审

源码基线 `d000f5fa`；本次修复只修改测试驱动、CI测试登记和文档，没有修改 Rust 生产实现。
[Engine 组装](../../../crates/uc-engine/src/assembly/sync_engine.rs)创建或安装完整领域能力，
[节点](../../../crates/uc-infra-p2p/src/network/iroh/node.rs)的 `session_protocol_alpns` 列出16个生产ALPN，
全部进入会话 dispatcher 后交当前 handler；旧版本ALPN仍是同一领域边界的兼容编解码入口。

| 完整能力 | 出站及入站管理边界 |
| --- | --- |
| 普通剪贴板3个ALPN | 专属manager、完整dispatch和receiver；单流／多流／归属扩展仍经同一受管边界 |
| 组更新2个ALPN | group manager与Application完整endpoint；旧wire保持业务分类，不独立拨号 |
| 成员历史2个ALPN | history manager、完整exchange／receiver，按实际协商选择复用或单次连接 |
| 当前状态与按需拉取2个ALPN | active／pull各自完整能力与独立容量，入站交既有owner |
| 可达性2个ALPN | presence manager及完整探测owner，不借给内容传输 |
| 文件／blob | Application BlobTransport及独立manager；SDK只消费受管流，不拥有第二拨号池 |
| 反向进度2个ALPN | Application完整双版本endpoint及独立manager，旧reporter原生旁路已删除 |
| 成员分支恢复 | branch manager、transport与receiver；原生旧adapter不存在 |
| Space加入／续接 | [专属manager](../../../crates/uc-infra-p2p/src/network/iroh/space_admission/managed.rs)在非成员认证前以QUIC公钥为资源键；不伪造DeviceId，不授予普通成员能力 |

原生拨号实际保留在 Iroh backend、上述非成员准入 backend 及底层 Relay 诊断；诊断探针不作业务发送。
节点测试辅助方法、测试模块和协议探针中的直接原生连接明确为测试，不算生产旁路。
Core／Application／协议的生产包依赖与源码由[仓库检查](../../../scripts/architecture/check-engine-repository.mjs)
核对；故意加入恢复原生拨号的负fixture也被拒绝。静态边界正确不替代各协议真实网络回归。

D15 的结论是“证据不足”：现有Direct完整同步可以使用已有地址材料；目录里的本地候选与慢Cloud替身测试
不能证明真实Engine在排除所有缓存后仅依赖mDNS完成发现、认证和同步。该项应在后续补齐独立进程证明，
或由issue维护者明确接受范围边界；本次没有将其勾为完成。

## 补充分层合同与覆盖边界

### 关联议题的责任与结算边界

2026-10-29重新读取原#34列出的关联议题与#169／#172／#173合并记录。下列GitHub状态只说明议题是否关闭，
不单独证明代码完成或仍缺实现；本次不代替这些议题逐项验收，也不自动关闭它们。

| 关联范围 | 已有实现或明确边界 | 仍须独立结算 |
| --- | --- | --- |
| [#68](https://github.com/UniClipboard/Engine/issues/68)（open） | 会话代次与长期网络owner已有接缝；[043规格](043-engine-network-runtime-and-space-session-handover.md)将网络寿命限定为Engine活跃期，挂起不无限保活endpoint | 完整生命周期、安全、性能与设备矩阵按043计划核验，不能用本片连接租约证明整项已完成 |
| [#70](https://github.com/UniClipboard/Engine/issues/70)（open） | 第三／四片的历史恢复与逐页ACK保持原业务退避，不引入批量协议 | 取消周期积压发送、连接后完整拉取及旧版上线发送的全部合同须另行核验，不把历史通信迁移等同#70完成 |
| [#170](https://github.com/UniClipboard/Engine/issues/170)（open） | 第二／四／八片按协议专属连接复用；presence健康连接不直接借给内容协议 | 原议题要求的Direct／Relay／高延迟／丢包握手次数与首条延迟对照，及收益结论须按其原验收结算 |
| [#140](https://github.com/UniClipboard/Engine/issues/140)（open） | 第三片及当前总审已证明组更新的完整Application接收负责人和受管入站；不是仍缺owner | 源码／切片证据可用于议题结算，GitHub开放状态不自动改变本片已完成事实 |
| [#141](https://github.com/UniClipboard/Engine/issues/141)（open） | 第三／四／六片及协议向量已有framing、ALPN和旧字节兼容证据 | 按原协议清单逐项核对余项与记录，不仅凭统一crate存在就关闭议题 |
| [#142](https://github.com/UniClipboard/Engine/issues/142)（open）、[#171](https://github.com/UniClipboard/Engine/issues/171)（closed） | [#172](https://github.com/UniClipboard/Engine/pull/172)已合并，区分明确拒绝／暂不可用／旧端未分类，处理历史sticky rejection；#171已关闭 | #142提出的其余拒绝粒度与发送行为须按原合同核对，不把两项完全等同，也不再次实现已合并分类 |
| [#166](https://github.com/UniClipboard/Engine/issues/166)（open） | [#169](https://github.com/UniClipboard/Engine/pull/169)已合并，Joiner／Sponsor生成材料时采样当前地址；Relay延迟及双角色互通已有独立证据 | 现场后台／蜂窝等边界保持原PR限制；议题仍开放不代表仍使用启动地址快照 |
| [#167](https://github.com/UniClipboard/Engine/issues/167)（open） | 对端握手失败与本机Relay健康是不同事实；不能直接推断全局endpoint重建 | 按原现场失效层、有限恢复与真实前后台证据结算，不由#166地址修复或单次Relay成功覆盖 |
| [#134](https://github.com/UniClipboard/Engine/issues/134)（open） | 与#167分开；现有Relay恢复矩阵只证明各自固定场景和预算 | 底层退避及三秒目标按[专属计划](2026-09-18-relay-only-three-second-recovery.md)与原议题逐项结算，不能推广为所有公网／设备场景 |
| [#104](https://github.com/UniClipboard/Engine/issues/104)、[#72](https://github.com/UniClipboard/Engine/issues/72)（均open） | 当前已有逻辑时间规则、真实网络分层和并行模式入口；不改生产期限 | 原各项性能、固定等待移除与重复压力要求继续由各自测试计划核验，不通过减少必需网络矩阵完成本次CI收尾 |

[#173](https://github.com/UniClipboard/Engine/pull/173)也已合并，覆盖当时连接生命周期计划的1／2／4／5项；
原问题3明确转#170。关联计划仍有未结算矩阵，不能把其active状态解释成上述四项都未实现。

### 分层合同

| 要求 | 验收入口与边界 |
| --- | --- |
| 多流、独立取消、复用与撤销 | 第二、四、七、八、九片；协议专属池，授权和会话代次在每次使用前重检，不跨协议借用健康状态 |
| 字节与协商兼容 | 固定旧向量、新旧角色 Direct／Relay、长度／FIN／超限拒绝；版本和二进制摘要按各份原工件保留 |
| 本地成功／远端保存／系统写入分别结算 | 第七、八片真实输出、接收权威、ACK 与 Collector／Jaeger；发送 ACK 不表示系统剪贴板写入成功 |
| 重启、积压和事务边界 | 第三、四、七、九片真实 SQLite／MLS、强杀与幂等；第二 backend 的虚拟时间不能代替网络及持久证明 |
| 设备网络运行期与 Space 会话 | 原 generation／迟到结果／切 Space／关闭测试；#68 的完整 endpoint 生命周期仍按其所属计划独立验收，不因 #34 切片完成自动关闭 |
| 有界资源与关闭 | 两轮各 1GiB、SDK worker／tag／staging、连接／任务／FD；第九片跨进程容量不执行 MLS，不能外推全部真实恢复压力 |
| 平台 | 原 CI 按实际 macOS／Windows／iOS／Android／HarmonyOS 编译步骤结算；编译成功不代表设备运行 |

## 首次失败与未完成清单

- [ ] 结算 #345 原最终 head 的失败／取消 CI；保存首轮失败、重跑结果与每一步跳过原因。
- [x] 第七片逐项回填 48 项计划／13 项验收与后续安全修复证据，按后续统一候选结算归档。
- [ ] 第九片逐项回填 AC01–AC12、最终候选及平台范围；满足原退出条件后归档。
- [x] 完成 D09／D10／D13 的当前生产入口总审与 D15 的证据边界核对；D15 端到端缺口继续明确保留。
- [ ] 最终文档相对链接、根六项门禁及交付候选检查通过。

原失败与债务不能通过勾选总清单消除：旧旧 a07 Relay 接收拒绝、#345 旧端关闭 `1104`、成员 F2／G2、
成员收敛和 Space 切换的间歇失败分别保留，由[技术债跟踪](../tech-debt-tracker.md)及所属计划维护。
目录再次激活触发重复拉取、完整升级非 smoke、物理切网／睡眠／后台和产品宿主采用也不能从本表推导完成。
