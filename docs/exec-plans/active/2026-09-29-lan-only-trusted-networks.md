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
- **成功结果**：设置保存成功并在重启后生效；经局域网或用户 VPN 直接可达的对端可配对、连接和同步（CGNAT/Tailscale 段需放行），全程不访问 relay、公共 DNS 或云 rendezvous。可信网段只是放行规则，不提供网络隔离
- **失败结果**：设置校验失败返回稳定分类且不保存；端口占用时启动按稳定分类失败；直连地址被全部排除时不提前失败（mDNS 仍可解析），最终连接失败沿用既有分类；任何情况都不降级到 relay
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

### 切片 4：私网直连地址保存与入站写回

- 2026-09-29 用户确认：LAN-only 与默认模式统一，写回本次正在使用的私网直连路径（经同一过滤规则），默认模式另保留正在使用的 relay。
- 写回触发点改为连接负责人 `PeerConnectionCoordinator` 收到当前成员的 `Online` 通知（2026-09-29 真实网络 L03 证明成员历史交换
  不会在已一致对端重连时发生，用它触发学不到端口变化）；历史交换里的两处触发已删除。
- 拨号不加排序代码：已保存地址与 mDNS 结果由 iroh 同时尝试。
- 同步修正 `persistable_addr.rs` 模块文档中已过时的“约 30 秒握手”描述，并修订联系恢复计划中“动态直连地址不写回”的不变量。
- 验证：写回条件的正反用例；协调者只对范围内成员的 Online 刷新、暂停与成员移除时取消、重叠通知合并；iroh 路径标记行为；
  容器内真实网络 L03（随机端口一方重启后由保存的新地址恢复）。

### 切片 5：真实网络验收

- 构造“单播可达、组播被阻断”的独立网络场景，优先复用
  [034 确定性虚拟 Peer Network 测试套件](034-deterministic-virtual-peer-network-test-suite.md)，不足时在实施中扩展并记录。
- 场景：完整邀请配对；单端重启在固定端口与随机端口下的恢复；两端随机端口同时重启的预期不可恢复；
  全程 relay、公共 DNS 与云 rendezvous 访问计数为零。
- 独立网络部分须在支持命名空间和 nftables 的 Linux 环境执行；未执行项记为“跳过”。

## 追加范围：移动端网络设置绑定（2026-09-30 用户批准，含 HarmonyOS）

- **负责人**：设置门面负责校验与持久化，Engine 只做契约映射，两个绑定只做投影，不保存状态、不重复校验。
- **调用方动作**：查询、更新；新设置经 `recover_network` 立即生效（会中断传输），或下次启动生效。
- **结果**：`Saved`（已保存）、`Rejected`（整次不保存，带字段、条目位置与固定分类），或 Engine 错误码。
- **重试**：没有自动重试，由用户改正输入后再提交。
- 已实现（本地未提交）：Engine 结构化拒绝、UniFFI 与 napi 各两个函数、`RecoverNetwork` 保留 `1102`。
  实测发现的缺口（保存了被占用的端口后宿主无法自行改回）2026-09-30 已由用户决定：报错，由产品侧提示，不做引擎侧自救；
  并批准新增三处稳定信号（公开 1102 常量、`StartupFailureReason::ListenPortUnavailable`、恢复状态失败类别），已实现。

## 公开契约缺口（切片 1 核查）

- iOS、Android（`bindings/uc-engine-uniffi`）与 HarmonyOS（`bindings/uc-ohos-napi`）没有绑定任何设置读写操作，
  移动端目前无法读写可信网段、固定端口或 `allow_relay_fallback`。移动端设置界面需要 Engine 先新增绑定，属于未规划的新范围。
- 发起端选择写入邀请的本机地址只存在于开发操作（`ListPairingInvitationAddresses`、`IssueInvitationForAddress`），
  不是公开操作。产品界面要选地址，需要先把它提升为公开契约，同样属于未规划的新范围。
  2026-09-29 用户决定本次不做，列为后续可选改进：不选时邀请已包含所有未被过滤的本机地址（通常含 VPN 地址），
  #1750 不依赖它；真实网络验收若发现地址过多导致配对慢或失败再加入，届时宜同时返回网卡名。
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
- [x] 切片 3（2026-09-29，本地）：设置端口进入绑定配置；环境变量只在设置了有效值时覆盖，未设置时保留设置值；
  启动记录标明端口来源且不输出端口与公网地址；端口占用返回 `ListenPortUnavailable` 并映射为公开错误 `1102`（不可重试）。
  新增占用端口绑定失败、设置与环境变量优先级、错误映射测试。已运行 workspace check、lan-compat check、fmt、仓库检查、
  diff check、`uc-infra` network 模块 266 项、`uc-engine` 相关单元测试与 public_contract。端口占用测试只在 macOS 本机运行，
  Linux/Windows 的 `AddrInUse` 来源链未验证；真实网络与 E2E 未运行。
- [x] 切片 4（2026-09-29，本地）：Core 公开 `is_private_address`；Infra `reusable_remote_addr` 保存正在使用的私网直连路径
  与 relay（无正在使用的 relay 时沿用已保存 relay）。写回时机最初挂在成员历史交换上（提交 650b763f），真实网络 L03 证明
  已一致对端重连不发生历史交换后，改为 `PeerConnectionCoordinator` 收到当前成员 `Online` 时调用 `RefreshVerifiedPeerAddressPort`
  （端口移入 `space/connectivity`，历史交换里的两处触发已删除）。新增 Core、Infra（含双端点回环的 iroh 路径标记测试）与
  协调者（范围内刷新、范围外与暂停不刷新、重叠通知合并、成员移除与暂停取消）测试。
- [ ] 切片 5（进行中，2026-09-29）：真实网络验收的测试环境已修正——`uc-infra` 新增 `in-process-multi-node` 特性，进程级
  单节点租约、LAN-only 标记与拨号策略只在该特性或 `cfg(test)` 下空操作，`test-util` 与 `dev-tools` 不再关闭它们；`lan-only`
  场景在配对前检查 rendezvous 请求数，策略被空操作时判环境无效。此前所有使用空操作策略的运行都不能作为仅局域网的证据。
  L03 暴露地址写回触发点错误（成员历史交换不会在已一致对端重连时发生），已改为连接负责人收到当前成员 Online 时写回。
  本机 Docker 容器（Ubuntu 24.04，arm64）、生产策略下：`lan-only` L01–L05 通过（组播屏蔽下完整邀请配对且 rendezvous 请求为 0；
  设置固定端口；随机端口一方重启 1.4 秒内恢复；两端同时随机端口重启在无发现时不能恢复、恢复组播后 0.6 秒恢复；测试网络之外
  出站包为 0）；`direct` 全量与 `relay` 通过；`known-peer` E13 在本机 Docker 环境中于分支基点即失败，与本计划无关。
  VPN 形态（`lan-only-vpn`，单播、无组播）：V01 可信网段为空时 100.64/10 地址被拒绝（稳定错误码 1227）；V02 加入可信网段后
  配对、传输、随机端口重启恢复且保存了 VPN 地址；V03 WireGuard 真隧道（MTU 1420）承载 10.x 地址，无需配置可信网段，
  底层网络隔离，512 KiB 双向传输，隧道计数增长约 623 KB；V04 测试网络之外出站包与 rendezvous 请求为 0。两个新 mode 各重复
  3 次全部通过，已加入夜间矩阵与手动触发（不进 PR 检查），CI 尚未实际运行。
  仍是两节点、单次通过的容器证据：不覆盖 Tailscale 产品本身、真实 Wi-Fi/路由器组播和各平台网络栈；不是发布验收。
