# 仅局域网可信网络

## 状态

- **状态**：已确认，待实施
- **日期**：2026-09-29
- **问题来源**：UniClipboard/UniClipboard#1750，仅局域网在 WireGuard 等自建 VPN 内无法发现、配对和连接
- **产品规格**：[PRD-022](../../product-specs/022-lan-only-trusted-networks.md)
- **技术设计**：[仅局域网与可信网络](../../design-docs/lan-only-trusted-networks.md)
- **完整负责人**：
  - 可信判定规则：`uc-core` 的 `TrustedNetworks`，发布与拨号两侧共用
  - 设置保存与迁移：既有设置流程与 `SettingsMigrator`
  - 连接重试与恢复：既有 `PeerConnectionCoordinator`，本计划不新增重试循环
  - 地址写回时机：既有成员维护流程（`HistorySynchronizer`）与 `RefreshVerifiedPeerAddressPort`
- **调用方唯一动作**：宿主保存网络设置（可信网段、固定端口）；配对继续使用既有邀请生成与输入操作
- **成功结果**：设置保存成功并在重启后生效；可信网段内的对端可配对、连接和同步，全程不访问 relay、公共 DNS 或云 rendezvous
- **失败结果**：设置校验失败返回稳定分类且不保存；候选地址被全部排除或端口占用时按稳定分类失败，不降级到 relay
- **重试与重启责任**：连接负责人保持既有退避与机会合并；本机重启后按既有启动流程主动拨号成员，对端由入站写回更新地址

## 范围

本计划只覆盖 Engine（上游）。Desktop 与 Mobile 的界面、文案和绑定对接在各产品仓的独立任务中完成，
依赖关系见“下游衔接”。

与[已知设备联系驱动的成员恢复与地址更新](2026-09-17-known-peer-contact-recovery.md)的关系：该计划负责已知设备联系触发成员确认，
以及已验证 relay 地址写回；本计划在同一写回端口上增加 LAN-only 直连地址条件和入站触发，不修改联系通知语义。
两者同时实施时，切片 4 须在该计划的写回改动合入后开始。

## 切片

### 切片 1：`TrustedNetworks`、设置迁移与公开契约

- Core 新增 `TrustedNetworks`：CIDR 解析、校验（拒绝公网段、`0.0.0.0/0`、`::/0`、格式错误、重复项）、
  永久排除集合（198.18.0.0/15、169.254.0.0/16）与包含判断。
- 网络设置删除 `allow_overlay_network_addrs`，新增可信网段列表与固定端口字段。
- `CURRENT_SCHEMA_VERSION` 从 3 升到 4；旧值为真迁移为 `100.64.0.0/10` 与 `fd7a:115c:a1e0::/48`，为假或缺失时列表为空。
- `addr_filter.rs` 的 overlay 判定改读 `TrustedNetworks`，行为与迁移前等价。
- `uc-engine` 设置契约同版本删除旧字段、新增新字段。移动绑定不暴露网络设置，不受影响。
- 验证：Core 单元测试覆盖校验与迁移映射；迁移测试覆盖 schema 3 的真、假、缺失三种输入。

### 切片 2：发布与拨号两侧共用可信过滤

- 用户已确认保留黑名单（2026-09-29）。
- 本端邀请与发布候选、对端拨号候选统一经同一判定过滤；出站拨号读取绑定时安装的 `DialPolicy`。
- 候选被全部排除时不提前失败（mDNS 仍可解析），沿用既有失败分类，不改走 relay。
- 新增测试固定：LAN-only 下解码出的邀请路由不含 relay。
- 验证：Infra 单元测试覆盖两侧一致性；日志与观测不含地址或网段内容。

### 切片 3：固定端口设置

- 固定端口成为持久设置；`UC_IROH_BIND_PORT` 保留为运行期覆盖，优先于设置，启动记录标明生效来源但不输出端口值。
- 端口被占用时启动失败并给出稳定分类，不回退随机端口。
- 验证：装配测试覆盖设置、环境变量、两者并存与端口占用。

### 切片 4：LAN-only 直连地址保存与入站写回

- LAN-only 下，写回条件扩展为：成员历史同步成功、地址为本次实际使用的直连路径、地址在有效可信集合内。
- 在成员历史交换的服务端完成点新增入站写回，证据标准与出站一致。实施前先确认处理器完成分支能取得已验证身份与连接远端路径；
  取不到时回到设计文档记录取舍，不另建证据来源。
- 拨号顺序：mDNS 已发现地址优先，其次已保存直连地址，沿用 `connect.rs` 既有超时与错峰。
- 同步修正 `persistable_addr.rs` 模块文档中已过时的“约 30 秒握手”描述。
- 验证：写回条件的正反用例；入站写回不在未验证身份、非成员或交换失败时发生；保存内容走既有加密仓储。

### 切片 5：真实网络验收

- 构造“单播可达、组播被阻断”的独立网络场景，优先复用
  [034 确定性虚拟 Peer Network 测试套件](034-deterministic-virtual-peer-network-test-suite.md)，不足时在实施中扩展并记录。
- 场景：完整邀请配对；单端重启在固定端口与随机端口下的恢复；两端随机端口同时重启的预期不可恢复；
  全程 relay、公共 DNS 与云 rendezvous 访问计数为零。
- 独立网络部分须在支持命名空间和 nftables 的 Linux 环境执行；未执行项记为“跳过”。

## 公开契约缺口（切片 1 核查）

- iOS、Android（`bindings/uc-engine-uniffi`）与 HarmonyOS（`bindings/uc-ohos-napi`）没有绑定任何设置读写操作，
  移动端目前无法读写可信网段、固定端口或 `allow_relay_fallback`。移动端设置界面需要 Engine 先新增绑定，属于未规划的新范围。
- 发起端选择写入邀请的本机地址只存在于开发操作（`ListPairingInvitationAddresses`、`IssueInvitationForAddress`），
  不是公开操作。产品界面要选地址，需要先把它提升为公开契约，同样属于未规划的新范围。
- 完整邀请的复制与粘贴不需要 Engine 改动：`IssueInvitation` 已返回 `full_invitation`，`JoinSpace` 已接受完整邀请，两个绑定均已暴露。
- 设置更新的拒绝原因只有英文文本；下游若需本地化或逐条定位，需要 Engine 提供结构化拒绝。

## 下游衔接

| 下游工作 | 依赖 | 可开始时间 |
| --- | --- | --- |
| 完整邀请复制与粘贴入口 | 无 Engine 新能力 | 立即 |
| 仅局域网说明文案重写 | PRD-022 | 立即 |
| 可信网段列表、固定端口设置界面 | 切片 1 契约 | 切片 1 合入后 |
| 一键加入 VPN 网段 | 切片 1 契约 | 切片 1 合入后 |
| 连接失败原因展示 | 切片 2 失败分类 | 切片 2 合入后 |

产品仓联调可以使用本地 Engine worktree；发布前必须改为已合入且可永久引用的 Engine 版本并重跑验证。

## 验收

- PRD-022 验收标准逐条对应切片 5 的场景或各切片的自动测试。
- 交付前检查按仓库 `AGENTS.md` 执行；持久化与公开契约变更另跑迁移测试与绑定生成检查。
- 完成后把稳定结论回写技术设计，本计划移入 `../completed/`。

## 进度

- [x] 切片 1（2026-09-29，本地未推送）：Core `TrustedNetworks`、schema 3→4 迁移、`addr_filter`/节点/邀请改读可信网段、
  Application 校验与视图、Engine 契约。`SettingsFacadeError::Invalid` 改为携带 `SettingsValidationError` 来源。
  已运行 workspace check、`uc-infra --features lan-compat` check、fmt、Rust 风格与仓库检查、diff check，以及
  uc-core/uc-infra/uc-application/uc-engine 相关单元测试与 `uc-engine` public_contract；完整 workspace 测试未运行。
- [x] 切片 2（2026-09-29，本地）：`runtime_consts` 的 LAN-only 布尔改为 `DialPolicy`；`connect::prepare_dial_addr`
  取代 `strip_relay_if_lan_only`，普通拨号与 blob 拨号共用；过滤日志改为只记数量；新增拨号策略与“relay 关闭时路由不含
  relay”测试。已运行 workspace check、lan-compat check、fmt、仓库检查、diff check，以及 `uc-infra` network 模块
  265 项与相关单元测试。真实网络与 E2E 未运行。
- [ ] 切片 3
- [ ] 切片 4
- [ ] 切片 5
