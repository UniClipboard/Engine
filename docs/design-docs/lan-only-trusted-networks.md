# 仅局域网与可信网络

本文是 [PRD-022](../product-specs/022-lan-only-trusted-networks.md) 的技术设计，覆盖可信网段、组播不可用时的配对、
直连地址的保存与恢复，以及设置迁移。产品承诺与验收以 PRD 为准。

## 现状与约束

- LAN-only 在 bind 时清除 pkarr/DNS 发现，只保留 mDNS；本端禁用 relay，拨号前移除对端地址中的 relay。
  `crates/uc-infra/src/network/iroh/{node.rs,connect.rs}`。
- 地址过滤 `addr_filter.rs` 是虚拟网卡黑名单：198.18.0.0/15、169.254.0.0/16 永远排除；
  100.64.0.0/10 与 `fd7a:115c:a1e0::/48` 仅在 `allow_overlay_network_addrs` 为真时放行。它不是白名单，`10.x` 不被过滤。
- 邀请由 `rendezvous/invitation_adapter.rs` 生成。LAN-only 下短码在本地生成并只经 mDNS 发布；短码不含连接信息。
  完整邀请（`ucspace1_` 前缀）含邀请 ID、准入路由（节点 ID 与候选 IP:端口）和过期时间；加入方直接输入完整邀请时跳过 mDNS 与云解析。
- 已保存的对端地址由 `persistable_addr.rs` 处理：有 relay 时去掉直连 IP；写回入口 `stable_remote_addr` 只接受“正在使用的 relay”。
  因此 LAN-only 下几乎没有地址被写回。这一规则的原因是随机端口重启后失效，旧地址会让拨号等满 QUIC 握手预算。
- 固定端口目前只有环境变量 `UC_IROH_BIND_PORT`（`uc-engine/src/assembly/network.rs`）。
- 设置 `CURRENT_SCHEMA_VERSION = 3`，已随发布出现，不能靠“未发布分支只增一个版本”吸收本次变更。
- 连接的完整负责人是 `PeerConnectionCoordinator`（见[已配对设备自动连接](automatic-peer-connections.md)）。
  重试、退避、机会合并与活性检查都归它，本设计不新增第二套。

## 责任划分

| 层 | 负责 | 不负责 |
| --- | --- | --- |
| Core | `TrustedNetworks` 值类型：CIDR 解析、校验、包含判断、永久排除集合；设置字段 | 网络 I/O |
| Application | 设置保存的校验结果、成员历史成功后的地址写回时机 | 判定某个 IP 是否被放行 |
| Infra | 用同一份过滤规则处理地址发现结果、本端邀请地址与对端拨号地址；按设置固定监听端口；地址仓储 | 决定何时重连 |
| Engine | 组装与稳定操作转换 | 网段判定、重试 |

同一份规则同时用于“本端可写入邀请或发布的地址”和“可拨号的对端地址”，不允许两边各自实现。

## 可信网段

**语义边界**：过滤规则是排除式的。可信网段只把本来会被跳过的 CGNAT/Tailscale 段地址放行，不限制任何其他可达地址，
也不阻断流量；它不是隔离边界，不能据此宣称“只在可信网段内直连”。仅局域网的承诺只是“不使用中继与公共发现”。
字段名 `trusted_networks` 保留，产品文案按 PRD-022 的建议使用放行含义。

- `uc_core::network::TrustedNetworks` 是用户 CIDR 列表的唯一判定。只接受整体位于私有地址空间内的网段：
  IPv4 的 10/8、172.16/12、192.168/16、CGNAT 100.64/10，IPv6 的 ULA fc00::/7。公网段、`0.0.0.0/0`、`::/0`、
  链路本地与 Clash fake-ip 都不在其中，因此无法声明为可信；格式错误与重复项同样拒绝。
- 保存时严格解析，拒绝原因只含条目位置与固定分类（`SettingsValidationError::rejection_reason`），不回显网段内容。
  网络启动与邀请生成时宽松解析，手工编辑造成的无效条目被跳过并只记录数量，不阻断启动。
- 当前过滤器是虚拟网卡黑名单：198.18.0.0/15、169.254.0.0/16 永远排除；CGNAT 与 Tailscale ULA 段内地址只有落在
  可信网段内才保留；其他地址（含 `10.x` 等 WireGuard 常用地址）本来就不被过滤。切片 1 只把旧开关换成可信网段，
  行为与迁移前等价。
- 2026-09-29 用户确认保留黑名单，不改为“本机物理私网加可信网段”的白名单：白名单会丢弃 IPv6 与经路由可达的
  私网地址，破坏升级前可用的环境；而仅局域网的隐私承诺已由关闭中继与公共发现保证。`10.x` 等 WireGuard 常用
  地址无需配置即可使用，可信网段只在 CGNAT/Tailscale 段内起作用。
- 同一判定用于三处：endpoint 的 `AddrFilter`（地址发现结果）、邀请写入的本端地址、出站拨号的对端地址。
  已保存地址与邀请路由直接交给 `connect`，不经过 `AddrFilter`，因此出站拨号统一经 `connect::prepare_dial_addr`
  读取节点绑定时安装的 `DialPolicy`（LAN-only 与可信网段），再过滤直连地址并在 LAN-only 下剥掉 relay。
- 直连地址被全部过滤时不提前失败：iroh 仍会经 mDNS 解析对端当前地址；最终失败沿用既有连接失败分类，不改走 relay。
- 过滤日志只记录丢弃数量与可信网段数量，不输出地址。
- 修改后需重启生效，与现有 bind-time 常量一致。设置保存成功与生效是两个结果，界面分别表达。
- “一键加入 VPN 网段”是产品仓功能：读取本机网卡信息后走同一个保存操作，Engine 不新增额外入口。

## 组播不可用时的配对

- 不新增协议。完整邀请已含路由，加入方已支持直接输入。
- 发起端选择写入邀请的本机地址：复用已有 `list_invitation_addresses` 与 `issue_invitation_for_address`，
  候选先经 `TrustedNetworks` 过滤。
- LAN-only 下邀请路由不得含 relay 提示。`encode_space_admission_route` 原样序列化传入的 `EndpointAddr`，不做 relay 处理；
  LAN-only 的本端 endpoint 使用 `RelayMode::Disabled`，`endpoint.addr()` 本身不含 relay，`issue_invitation_for_address`
  再经地址过滤只保留 IP。因此路由天然不含 relay，但这是间接保证，实施时要用测试固定：LAN-only 下解码出的路由不含 relay。
- Desktop、移动端的完整邀请复制与粘贴由产品仓实现，Engine 只保证邀请内容与稳定操作。

## 直连地址的保存与恢复

**原则**：保留“随机端口的直连地址不可靠”这条现有结论，只在有依据时保存直连地址。

- LAN-only 下，对端地址在满足以下全部条件时写回既有加密地址仓储：
  1. 已通过既有规则：当前成员历史同步成功并提交；
  2. 该地址正是本次成功连接实际使用的直连路径；
  3. 地址未被同一过滤规则排除（即拨号时会被使用的地址）。
- 保存的直连地址不替代 mDNS。拨号顺序是 mDNS 已发现的地址优先，其次是已保存的直连地址。
- 已保存直连地址不需要另设预算：`connect.rs` 已对每次拨号使用 3 秒单次超时与 0/500/1500ms 错峰重试，
  `persistable_addr.rs` 模块文档中“约 30 秒握手”的描述已经过时。失败后沿用现有退避，不删除记录（下一次成功会覆盖）。
- 双向学习需要新增入站写回点。现有写回只发生在出站：`HistorySynchronizer` 在某个对端同步结果为 `Confirmed` 后调用
  `RefreshVerifiedPeerAddressPort`，由 Infra 读取该次连接的远端路径。入站处理器没有对应调用，因此接收方不会更新对端地址。
  入站写回的证据标准与出站相同：传输层已验证身份、该身份映射到当前成员，且该次成员历史交换成功。
  写回仍由同一个端口实现，Application 不新增第二个证据来源；入站与出站只在“谁触发调用”上不同。
- 本机监听端口改变（含固定端口设置变化）需要重启。重启后 `PeerConnectionCoordinator` 按既有启动流程主动拨号所有成员，
  对端在入站成功后更新它保存的本机地址。本机保存的对端记录不受本机端口改变影响；对端自己改了端口，则靠对端重启后主动拨回。
- 仍无法自动恢复的情形：两端都改端口或都以随机端口重启，且没有可用的 mDNS。恢复路径是固定端口或用完整邀请重新配对。
- 保存的记录继续属于敏感数据，走既有加密仓储，日志与观测不含地址。

## 固定端口设置

- 固定端口成为持久设置，取值 1–65535；空值表示随机端口。当前 `UC_IROH_BIND_PORT` 由 `apply_iroh_direct_reachability_from_env`
  在装配时读取；`iroh_bind_port_override` 只是测试注入。设置成为唯一持久来源。
  环境变量作为运行期覆盖保留给无头和容器部署（ADR-007 的用途），优先于设置，并在启动记录中标明生效来源；不作为第二份持久配置。
- 端口被占用导致 bind 失败时，启动失败并给出稳定分类，不静默回退到随机端口，否则用户以为已固定。
- 修改后重启生效，语义同可信网段。

## 设置迁移

- `CURRENT_SCHEMA_VERSION` 从 3 升到 4，由 `SettingsMigrator` 完成。
- 网络设置删除 `allow_overlay_network_addrs`，新增 `trusted_networks`（CIDR 文本列表）与 `listen_port`（`None` 为随机端口）。
  两者按 `docs/SECURITY.md` 的已批准例外明文保存。
- 旧字段删除后 `Settings` 反序列化会丢掉旧值，因此持久格式所有者 Infra 用 `LegacySettingsFields` 从同一份原始
  settings.json 单独读取旧开关，再交给 v3→v4 迁移。Core 模型不保留旧字段。
- 旧值为真：列表写入 `100.64.0.0/10` 与 `fd7a:115c:a1e0::/48`；旧值为假或缺失：列表为空。固定端口默认为空。
- 迁移只前进。旧版本不能读取 schema 4；这与既有设置迁移的方向一致。
- 公开契约 `uc-engine` 的 `NetworkSettingsSummary` 与 `NetworkSettingsPatch` 同版本删除旧字段并新增新字段，
  产品仓同步升级，不保留兼容读取。补丁中 `listen_port = Some(0)` 恢复随机端口。iOS、Android、HarmonyOS 绑定
  当前不暴露网络设置，本次不受影响。

## 业务记录

- 设置保存：触发原因（用户保存）、动作（校验并保存网络设置）、结果（成功或稳定错误分类）。
  不记录网段内容、端口或地址。
- 地址保存与拨号使用的是已有连接维护记录，本设计不新增独立业务入口；失败分类沿用稳定分类，新增“候选被可信网段全部排除”这一类别。

## 验证

- Core：`TrustedNetworks` 的校验、永久排除、包含判断与迁移映射。
- Infra：过滤在发布与拨号两侧使用同一规则；LAN-only 下路由不含 relay；直连地址写回条件与短预算。
- 真实网络：需要“单播可达、组播被阻断”的独立网络场景。测试基础见
  [确定性虚拟对端网络测试套件](../exec-plans/active/034-deterministic-virtual-peer-network-test-suite.md)，是否可复用需在实施计划中确认。
  验收覆盖 PRD 中 WireGuard 类场景：完整邀请配对、单端重启的固定端口与随机端口恢复、整个过程无 relay、公共 DNS 或云 rendezvous 访问。

## 未决问题

- 入站写回落在哪个 Infra 位置：`IrohMembershipHistoryExchangeHandler` 的服务端完成点是否已能拿到已验证的对端身份与该连接的远端路径，
  需要在实施计划中读取处理器完成分支确认。
- 环境变量覆盖设置时，Desktop 与移动端是否需要显示“端口由环境变量决定”，由产品仓决定。
