# Issue #34 第九切片：成员分支恢复受管连接实现规格

状态：**实现与验收已完成并归档**（2026-10-30）。逐项证据、首次失败及未执行范围见[执行计划最终结算](2026-10-28-membership-branch-recovery-connectivity-plan.md#最终候选网络核验与归档结算2026-10-30)；#34整体仍保持开放。
核对基线：`80bda137541f1a054897f8d1474634dac9d752e8`（第八切片归档PR #342合并提交）。实施前重新固定实际main、候选及兼容端SHA，不以本文基线自动代表未来发布版本。

## 1. 概述（Overview）

成员历史分叉后，接收方通过目标分支成员取得GroupInfo、生成external commit、取得恢复包，再推进原有持久分支转换。这条恢复业务已存在，但通信仍绕过Uni受管连接：Infra自行解析地址、调用`connect_with_staggered_retry`、操作原生双向流；服务端直接通过`ProtocolHandler`识别身份并调用签发负责人。

本片将这条完整通信能力迁入已有`ConnectivityManager`。要解决的是恢复请求在排队、拨号、读写及本地签发期间的会话失效、取消、资源归还与重启责任，不能仅把原生连接包装为trait。发现层已经接入生产，不重复实现发现模块。

完整负责人及调用合同：

| 项目 | 合同 |
| --- | --- |
| 恢复业务负责人 | Application `space/membership/recover_conflict`；`RecoverMembershipConflictUseCase`决定恢复目标、两阶段顺序、持久阶段及重试 |
| 签发负责人 | 同模块的`IssueMembershipBranchRecoveryUseCase`核验请求与目标分支并完成既有幂等签发 |
| 调用方唯一动作 | 继续调用既有成员恢复维护入口；宿主、Engine和Runtime不拼接GroupInfo、external commit或恢复包步骤 |
| 成功 | 原有恢复包验证和持久转换完成；网络交换成功本身不算成员恢复完成 |
| 失败 | 保留现有Completed／Deferred／StableFailure／Corrupt业务映射；内部保留取消、拒绝、期限、网络及损坏来源 |
| 重试与重启 | 原业务负责人从既有加密记录恢复；通信层不重发业务帧、不回滚正式提交、不新增恢复队列 |

## 2. 目标（Goals）

- 出站和入站均经过Uni连接、租约、流及关闭监督；生产分支恢复不再自行拨号或接受原生流。
- 冻结会话及撤销基线，排队、迟到握手、读写完成均不能重新授予失效请求资格。
- 保持分支恢复专属授权：认证传输身份不能代替冲突、目标分支及recipient资格核验；普通同步的冲突阻断也不能误阻恢复。
- 两个既有请求分别有完整绝对期限，所有等待与读写不能逐步骤续期。
- 保持`/membership-branch-recovery/1`协议、4 MiB帧上限及现有持久状态格式；旧新端双向互通。
- 用第二测试backend证明恢复通信不要求业务层依赖iroh，并以真实网络证明实际分支恢复。

## 3. 非目标（Non-Goals）

- 不迁移Space加入／配对准入，不实现附近设备列表、Nearby或新发现来源。
- 不更改分叉选择规则、恢复目标选择顺序、MLS算法、恢复包签名或nonce验证。
- 不新增ALPN、多流版本、批量消息、跨协议共享连接池或空闲预热。
- 不增加Engine／绑定公共API，不新增持久字段、恢复格式或明文例外。
- 不顺带修复旧旧Relay文件拒绝、成员F2间歇失败等历史技术债；原失败仍须记录。
- 不把本片通过等同整个Issue #34完成、产品宿主采用或发布验收。

## 4. 设计前架构上下文（2026-10-28基线）

本节保留设计前的责任断点；受管实现已替换原生 adapter，当前证据见执行计划，不将下列旧状态当作当前实现。

### 恢复与签发业务

Component: 恢复接收方及签发方。
Path: [recover_conflict](../../../crates/uc-application/src/space/membership/recover_conflict/)。
Responsibility: `use_case.rs`恢复选择后的分支；`issuer.rs`签发；`ports.rs`定义业务信道、资料准备与转换能力。
Relationship: 接收方持久保存RecipientPrepared后提交external commit；签发方通过TargetPrepared／TargetCommitted复用既有结果；响应丢失不代表签发未发生。

### 现有传输实现

Component: 原生分支恢复adapter。
Path: 基线中的 `membership_branch_recovery_adapter.rs`（已删除）；现有受管入口为[branch_recovery_handler.rs](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/branch_recovery_handler.rs)。
Responsibility: 出站地址解析、重试拨号、framing、入站身份解析和业务dispatch。
Relationship: 目前同时承担连接操作和协议交换；逐步骤使用10秒超时。请求／响应为u32长度前缀、编码体和发送方向FIN，每条连接处理一次交换。

### 受管连接

Component: Uni连接与身份／撤销桥。
Path: [Application connectivity](../../../crates/uc-application/src/connectivity/)、[Infra connectivity](../../../crates/uc-infra-p2p/src/network/iroh/connectivity/)。
Responsibility: backend、租约、取消、准入、容量、会话更替及关闭监督。
Relationship: 当前`ConnectivityProfile`和`ConnectionProtocol`未包含MembershipBranchRecovery；不能沿用普通协议profile假装已经支持。

### 组装与协议

Component: 节点、Engine组装及协议合同。
Path: [node.rs](../../../crates/uc-infra-p2p/src/network/iroh/node.rs)、[sync_engine.rs](../../../crates/uc-engine/src/assembly/sync_engine.rs)、[membership_branch_recovery.rs](../../../crates/uc-sync-protocol/src/membership_branch_recovery.rs)。
Responsibility: 节点当前构造原生channel并安装handler；Engine注入完整能力；协议crate定义五种消息及4 MiB上限。
Relationship: Engine仅替换完整对象接线，不获得恢复内部步骤。

## 5. 设计方案（Proposed Design）

### 组件（Components）

1. 在`recover_conflict`内部增加`transport.rs`业务通信模块，按owner、port、模型、错误、测试拆分。它实现现有`MembershipBranchRecoveryChannelPort`，输入仍为既有request／commit，输出仍为GroupInfo／恢复包，依赖manager及专属wire port。它负责一次交换的请求冻结、期限与结果分类，不选择peer或执行MLS。
2. 同模块提供完整入站receiver，消费Uni租约、读取一个领域请求、调用既有issuer、写回一个结果。业务dispatch从Infra移回Application；不得要求Engine选择begin或issue。
3. Infra增加`ConnectivityProfile::MembershipBranchRecovery`，只协商现有ALPN；`IrohPeerConnection`将其识别为`ConnectionProtocol::MembershipBranchRecovery`及`SingleExchange`。handler仅将原生连接包装、交付manager和完整receiver。
4. Infra wire adapter只把Uni流与已有wire编码互转，不拨号、不读成员仓储、不持久化、不决定重试。旧adapter在调用方整体迁移后删除，不能保留原生fallback。
5. Engine创建专属manager，装配完整channel和receiver，纳入现有会话更新、撤销及关闭参与者；不能新增平行生命周期控制器。

### 授权与围栏

连接层核验传输公钥到逻辑设备的唯一映射、原会话generation及撤销基线；入站须在异步身份读取之前冻结基线。无法解析、歧义映射、错peer、错误ALPN、关闭或旧世代均拒绝。

必须先检查普通`PeerAdmissionPort`对分叉、被隔离成员与移除的实际语义。不得直接复制普通同步准入，也不得增加`allow_all`、布尔绕过开关或基于地址的授权。若现有准入会阻断合法恢复，由`recover_conflict`定义并实现专属、最小的恢复连接资格port：输入peer，输出当前可建立恢复通信的资格或带来源的失败；Infra只适配它。其正向事实仍来自当前成员历史和恢复记录，不来自发现目录、presence或历史缓存。

消息级授权保持在issuer：目标必须等于本机持久分支，recipient与认证source绑定且在目标历史有效，本机签发成员也必须有效。接收方继续核验恢复包的conflict、target、recipient、签名、期限与nonce。撤销后重加的同设备不能使旧请求复活。

移除、切Space、取消及最终提交的线性化边界要由原业务owner控制。每次网络返回后，在准备／持久提交前重新核验原操作scope；已经开始的不可取消本地提交由原负责人监督到实际结束。不能对已正式提交的旧Space结果宣称“回滚成功”，也不能将其写进新Space。

### 数据模型（Data Model）

- 复用现有`MembershipBranchRecoveryRequest`、`MembershipBranchRecoveryCommit`及持久恢复session；不新增数据库字段。
- 新增仅内存的交换上下文，包含冻结的连接请求、取消信号、绝对deadline及原业务scope。作用域仅为一次请求，不能持久化或对宿主暴露。
- Application领域请求使用RequestGroupInfo／SubmitExternalCommit，响应使用GroupInfo／RecoveryPackage／Rejected；版本号、编码字节与长度前缀仅在协议／Infra层处理。
- 请求体、GroupInfo、external commit与恢复包均敏感；Debug、错误和日志不输出内容、身份、地址或业务标识。

### 接口（API / Interface）

保留现有业务信道两个方法及返回类型。其实现改为Application受管channel，不把连接、deadline或流暴露给业务调用方。取消来源由原会话及执行owner注入，内部显式传递，不能另造永久未取消token作为替代。

信道在业务动作开始时另提供一次内部范围冻结：返回只供该动作使用的不透明范围，两个既有方法复用此范围。范围持有原会话与撤销基线，成员负责人在准备及条件提交边界核验；读取成员事实与冻结之间若有状态发布，该轮延期。此内部能力不扩展Engine或宿主入口。

专属wire port定义四个完整编解码操作：`write_request`、`read_request`、`write_response`、`read_response`。参数为Uni发送／接收流、领域消息及同一绝对deadline；返回领域消息或携带source的内部wire错误。读取必须验证长度、完整body、版本、消息方向及FIN；不能只用不受限`read_to_end`。写入须先校验编码后的4 MiB上限，再写长度和body并FIN。

入站完整入口`receive`接受受管租约与原绝对期限；内部dispatch至现有issuer。具体构造参数集中在唯一完整构造入口，不增加测试专用空依赖。接口通过`space/mod.rs`现有出口集中引入，内部文件保持私有。

内部错误区分Stopped、DeadlineExpired、Busy、Denied、Unavailable、Invalid并保留来源。映射到旧channel错误时：停止／期限／容量／网络映射Unavailable；资格拒绝和对端Rejected映射Rejected；畸形或错误方向映射Invalid。Stopped仅结束当前执行，不能自动产生永久业务失败；现有业务outcome映射如有冲突，须在原owner统一修正并测试。

### 预算与资源

以下是本片拟定初始策略，实施必须先完成阶段0兼容基线，不得静默放宽：

| 项目 | 策略与原因 |
| --- | --- |
| 单次出站交换 | 从channel调用入口起30秒绝对期限，覆盖地址、排队、拨号、准入、开流、写入及响应；替代原逐步骤10秒反复续期 |
| 入站交换 | 从原生连接交付起30秒绝对期限，首流等待最多10秒且受总期限约束；本地提交开始后的监督规则另行适用 |
| 连接模式 | SingleExchange，每条连接只接受一组请求／响应；两阶段各自建连，与旧端一致，不加空闲保留 |
| 本地并发上限 | 每peer出站1、会话出站拨号4；入站每连接1、会话8；等待每peer4、会话16，超额立即Busy，不新增无限队列 |
| 重试 | 本通信模块不额外重试，底层采用单轮拨号；后续尝试由原业务维护调度触发，并复用原持久恢复事实 |
| 帧 | 保持1至4 MiB，长度前缀本身不计入body上限；禁止解码前按不可信长度无限分配 |

SingleExchange不得合并不同业务交换共享同一条只能完成一次的连接。所有额度覆盖真实结算，取消不提前释放仍在进行的本地工作额度。具体manager配置字段如不足，只补该语义所需最小能力，不建立未来通用调度系统。

### 工作流（Workflow）

1. 原恢复维护入口读取选定冲突及已有session；无工作仍返回原Completed，不发网络。
2. channel入口冻结操作scope、连接请求和deadline，经manager获取租约及流，wire发送GroupInfo请求并验证响应。
3. owner重新核验scope，准备recipient资料并持久保存RecipientPrepared；其后才提交external commit。
4. 第二次交换重复冻结本次请求，但必须仍绑定同一业务scope；不能借重试更新generation绕过旧意图失效。
5. 服务端经有限入站额度、身份映射和连接围栏进入receiver；receiver解码后调用issuer专属资格与幂等流程。
6. 响应失败只结束当前交换，不撤销签发事实。重新执行时复用相同external commit及原TargetPrepared／TargetCommitted结果。
7. 接收方验证、保存恢复包并推进既有转换；仅原owner确认业务完成。会话停止后停止新工作，已开始本地提交实际结算后归还资源。

观测沿用[业务记录组织标准](../../design-docs/observability.md#业务记录组织标准)：恢复动作由完整负责人结算，连接与读写作为诊断；不为两次网络往返各建业务root。跨层持续计时仅在Engine装饰已有完整port；不增加内部阶段查询或观测业务接口。本片不扩展wire传播字段。

## 6. 实施步骤（Implementation Plan）

| 步骤 | 位置与修改 | 风险及完成证据 |
| --- | --- | --- |
| 0：固定基线 | 原恢复use_case／issuer、准入、真实宿主及协议fixture | 先证明分叉双方可恢复、原拒绝／幂等／断点行为，核对冲突下准入及两阶段30秒预算；记录当前与兼容端SHA，不以mock代网络 |
| 1：最小纵向 | recover_conflict transport、专属wire、backend profile、handler与Engine接线 | 同一交付完成真实GroupInfo到恢复包和原持久转换；不得仅交付trait或孤立包装 |
| 2：完整生命周期 | channel／receiver、原owner scope、会话参与者及撤销登记 | 故障gate阻塞拨号、身份读取、响应和提交，证明取消隔离、切Space、移除与实际关闭 |
| 3：兼容与资源 | 协议fixture、第二backend、现有网络与成员harness | 新旧双角色Direct／Relay，畸形帧、容量、重复commit及响应丢失／进程重启；不改旧端以促成通过 |
| 4：删除与结算 | 删除原生adapter路径、架构检查及稳定文档 | 添加有负fixture的旁路检查；回写实际合同、逐项证据和跳过项，完成后才归档，不关闭整个#34 |

所有步骤复用现有共享target，由一个验证负责人串行运行Cargo。此表是实施顺序，不是已完成进度。

## 7. 边界情况（Edge Cases）

| 场景（Scenario） | 预期行为（Expected behavior） | 实现边界（Implementation） |
| --- | --- | --- |
| 无冲突或缺失恢复目标 | 无工作按原合同完成；结构损坏按原Corrupt，不凭空选peer | 原owner |
| 分叉导致普通同步被阻断 | 仍按恢复资格核验；不能全放行也不能误用普通同步规则全部拒绝 | 专属准入＋issuer |
| 空GroupInfo／空commit／零长度帧 | 拒绝，不创建有效恢复session | wire＋原owner校验 |
| 超限、截断、额外字节、未知版本、响应冒充请求 | 有限读取并拒绝，不调用issuer，不输出负载 | wire |
| 同peer并发、容量耗尽 | 有界等待或Busy，无影子原生拨号／无无限任务 | manager |
| 本次取消、另一请求仍活动 | 只结束本交换；其余请求正常，资源最终归还 | 租约及owner监督 |
| 网络失败或响应丢失 | 不声称远端未提交，返回原延期／失败分类；下一轮从持久事实恢复 | 原业务owner |
| 同commit重放／不同commit复用同transition | 前者保持幂等；后者拒绝，不能覆盖原签发 | issuer及持久session |
| 移除后重加、迟到连接、切Space | 原请求失效；不能使用新资格复活旧请求 | 冻结围栏＋提交scope |
| 本地提交开始后取消或期限耗尽 | 监督实际提交结果，不中断制造半状态，不把网络结束当业务回滚 | 原本地工作边界 |
| 旧端只处理一个流 | SingleExchange，一轮一连接，无新ALPN或额外首帧 | backend＋wire |
| 进程在四类持久阶段中退出 | 沿既有恢复状态继续；不增加新格式，不从头重签覆盖 | 原恢复owner |

## 8. 测试策略（Testing Strategy）

### 单元测试（Unit Test）

- 冻结旧请求后切会话／撤销再恢复：旧请求拒绝，新动作可取得新资格；准入来源失败保留完整source。
- wire输入为五种消息、边界长度及错误方向：合法编码与基线fixture逐字节相同；非法输入不进入业务dispatch。
- 可控时间依次阻塞排队、拨号、读取前缀与body：总期限只消耗不续期；旧请求不能另获30秒。
- 内存第二backend运行两阶段与取消场景：业务输入／输出与iroh无关；不把此结果称为真实网络通过。

### 集成测试（Integration Test）

- 使用真实签名成员历史、SQLite及MLS构造分叉，完成GroupInfo、commit、恢复包和持久转换，核验双方最终事实。
- 各网络／身份读取gate阻塞后移除成员或切Space，再释放：迟到网络结果不能进入新scope；已开始旧scope提交保留真实结算。
- 签发成功后丢响应／强杀进程，重启提交原commit：返回同一有效结果，不重复消耗nonce或覆盖session；不同commit拒绝。
- 填满并发与队列后取消、关闭：任务、连接及额度回到基线；关闭不留下未监督拨号或本地签发。

### 回归测试（Regression Test）

- 复用`tests/hosts/connectivity`、原成员拓扑测试与既有Relay host，在独立OS进程执行当前／当前、旧／新、新／旧的Direct与Relay恢复矩阵。需要补场景时扩展原harness，不创建合成地址替代网络证明。
- 协议字节兼容、身份错配、成员移除、原恢复四类持久阶段、成员历史与普通组更新回归；完整workspace及既有必需CI。历史F2失败单独保留，不用隔离绿覆盖全集失败。
- 编译平台使用现有Linux／macOS／Windows、iOS／Android／HarmonyOS门禁；真机、后台和产品宿主未执行只能记跳过。第八片的物理跳过不等于本片设备已验证。
- 根交付六项：`just cargo metadata --locked --format-version 1`、`just cargo check --workspace --all-targets --locked`、`just cargo fmt --all -- --check`、Rust风格检查、仓库架构检查、`git diff --check`。

每项真实网络记录源码／二进制SHA、角色、实际路径、恢复断言、通过／失败／跳过和重放入口。Artifacts不含真实用户资料或凭据，不把本地工件当正式文档依赖。

## 9. 验收条件（Acceptance Criteria）

* [x] AC01：原维护入口完成真实分支恢复，Engine与宿主没有新增步骤编排或公共API。
* [x] AC02：出入站均走专属Uni profile和受管租约；旁路静态检查及故意加入原生拨号的负fixture生效。
* [x] AC03：合法分叉恢复不被普通同步准入误挡；陌生、歧义、错peer、已撤销及不合法recipient全部拒绝。
* [x] AC04：原generation冻结覆盖排队、身份读取、拨号、流和业务提交；移除后重加、切Space、迟到结果不能复活旧请求。
* [x] AC05：一次取消不影响其他交换；30秒绝对期限及10秒首流上限不因步骤切换续期，本地提交实际监督结束。
* [x] AC06：连接、拨号、队列和入站并发遵守本片预算；过载有界，关闭后资源回到基线。
* [x] AC07：现有ALPN、五类消息、长度前缀、FIN及4 MiB限制保持；新旧双角色Direct／Relay真实矩阵通过。
* [x] AC08：响应丢失、相同commit重试、不同commit冲突及各持久阶段重启保持原幂等／恢复合同，无新格式或明文写入。
* [x] AC09：第二测试backend完成同一业务通信；Application／Core／协议层没有新增iroh类型依赖。
* [x] AC10：来源错误可追溯，公开错误与日志脱敏；网络成功不冒充恢复完成，无额外网络业务root。
* [x] AC11：原生旧实现删除，生命周期参与者完整，相关测试、根门禁及最终候选必需CI通过；未执行平台逐项跳过。
* [x] AC12：稳定合同回写，证据逐项关联；旧技术债保留，按实际完成状态归档，不据本片关闭整个#34。

## 10. 风险与取舍（Risks and Trade-offs）

- 普通成员准入可能与恢复资格不同，这是首要风险；选择复用身份／围栏能力并由恢复owner维护业务资格，避免复制成员规则或直接放行。
- 30秒总期限比逐步骤10秒更可预测，但可能收紧慢链路容忍度；必须先测原Direct／Relay实际恢复，不以增加重试掩盖预算问题。预算不足时在本规格记录证据后调整，不能只改实现。
- SingleExchange放弃两阶段连接复用，换取现有旧端兼容和较小迁移范围；以后需要多流须另有协商与测量，本片不预设新版本。
- 网络取消与本地持久提交不等价；监督提交可能使资源释放晚于响应期限，这是保证状态完整的必要成本，不得报告虚假的即时关闭。
- 不把整个恢复用例或签发、port、wire与测试塞进一个`mod.rs`；模块入口保持声明与必要导出，防止统一负责人演变为单文件负担。
- peer选择沿用现有首个证据peer规则，不新增地址评分、在线优先或自动切换heuristic；若发现其可用性问题，另记证据和范围。

## 11. 实施前问题的核验结论

1. **普通准入不能直接复用。** 基线规则明确拒绝 `Diverged`；本片通过真实分叉恢复验证后，使用恢复专属成员事实port及认证身份唯一映射，保留消息级recipient与签发资格核验，不放宽普通同步规则。
2. **保留30秒交换预算。** 出入站交换共用绝对期限、首流10秒与并发预算均有专项验证；当前交付候选三角色Direct／Relay六格通过。矩阵耗时包含配对、分叉和进程操作，不能拿整格耗时当作单次交换耗时，也不据本地拓扑宣称所有公网慢链路均已验证。
3. **兼容端已固定。** 恢复矩阵旧端源码为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`，真实新旧双角色互通及五类消息向量已有证据；实际二进制SHA和当前候选来源见执行计划，不凭版本名或另一协议的旧端替代。
4. **提交继续由原owner监督。** 原作用域覆盖网络返回、准备和条件提交；真实迟到响应下切Space、同设备移除重加及各持久阶段重启有专项验证。已开始的不可取消提交监督到实际结束，不把超时或取消等同回滚。

稳定结论由[成员分支恢复通信边界](../../design-docs/membership-history-ownership.md#成员分支恢复通信边界)维护；
验证层次、失败、跳过项及最终候选范围见[逐项证据表](2026-10-28-membership-branch-recovery-connectivity-plan.md#验收证据核对最终结算)。
以上问题及AC01–12已按最终代码候选 `33dc6f03` 的证据结算；首轮失败、技术债和设备等跳过项见执行计划，不外推为#34整体完成。
