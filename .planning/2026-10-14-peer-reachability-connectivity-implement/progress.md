# 执行记录
- 读取 implement、Rust、TDD、planning 技能和仓库根/相关局部约束、规格与计划。
- git fetch origin main 成功，冻结最新 main。
- 检查当前环境：sudo 需要密码；netns 矩阵暂不可运行，未标通过。
- a07 宿主成功构建（96 秒），旧生产库无补丁；当前宿主构建成功（39 秒）；D2 a07_to_head 真实旧旧配对、互传、单端升级测试通过（20.22 秒）。
- user/mount/net namespace 探测成功，可以用 unshare -Urnm 不依赖 sudo 运行真实隔离网络；不在 namespace 编译。
- 补充既有恢复驱动 P00-prepaired-upgrade：真实旧旧配对、关闭、保存原始加密目录与进程内安全存储、复制角色资料后只升级一端；比对身份、成员与真实内容，检查实际 selected path，自动回收所有资料。
- rc.15 两个升级位置的 Direct 与强制 Relay 前置均通过，旧旧及升级后双向文本精确读回；未使用人工资格。首个 Direct 驱动未记录独立旧旧结果，已修正记录名称并需补跑。
- 第一版新 owner Engine 宿主编译通过；真实 Direct E01 配对通过（5.534 秒）；codec 固定向量测试通过。
- 新第二 backend 测试证明 per-peer pending 共享且 promotion 后释放、长期租约不持该额度，测试通过。修复了仅持 semaphore permit 而丢掉 PeerState 强引用会导致同端额度被重建的缺陷。
- 第一次全 workspace check 失败：Backend 取消信号新增依赖与测试调用未更新；已补 tokio-util 依赖、完整调用迁移和锁文件。第二次全 workspace 全目标 check 通过（64 秒，有既有警告）。
- 初稿增加源链、actor-owned task、入口未知连接取消、scope 原 20 秒期限内部传播、v1 显式保留能力、v2 同轮并行检查；尚在迁移原生旧测试、删除旧路径及审查竞态，不能交付或归档。

- 已删除原 native adapter/liveness/IO codec 和业务 builder，替换为单一 Application transport 与薄 bridge；原生、外部身份准入 tests 迁移完成。
- 真实 SQLite + Iroh owner source 故障回归通过；原生并行拒绝最终通过；46 个 coordinator/transport 回归及 Uni 取消/额度三项通过。
- 首个必需 20 轮 Relay 在第六轮失败，首次工件保留。发现旧 actor 在 coordinator 取消后仍占 per-peer check；新增 RAII 请求取消与 Uni 可结算取消。新网络取消回归证明 200ms 内后继核验复用健康连接，实际本地读取继续持资源许可。
- Scope 真实工作已由 coordinator JoinSet 等待，原四尝试许可通过私有上下文延续到 owner。scope 本机故障现在保留显示、停新工作；恢复后重读继续。
- 隐私 red 证明通用 error 日志泄露 source 中私有哨兵；新日志仅固定 class/IO kind，完整 typed source 留在返回错误，修复回归通过。
- 默认 PR 添加完整强制矩阵与移动编译；尚未推送、未检查线上结果、未归档。必需矩阵修复后运行中。

- 新新 Relay 修复后 20 轮通过：Online 最慢 372ms、双向读回最慢 535ms（mandatory-fixed），IPv4/IPv6 UDP 均有真实计数、fixture 清理和明文扫描通过。
- mandatory-final 再次 20 轮通过，但冻结 main 接收端的持续单向限制恢复第三轮失败。限制只用于初始角色证明，故障前恢复默认双向主动；对应 side1 接收三轮重新通过，首次过约束失败保留，不冒称它通过。
- main 主动 Direct 与 rc.15 主动 Direct 的实际 outgoing 方向通过。矩阵 now 使用 upgraded side0 主动、side1 接收，各自独立复制旧旧配对资料。
- 替换入站原按 native ID 排序，乱序 ID 行为 red 确认缺陷；改按完成准入顺序 FIFO 后 green。
- 增加原 presence.connection.closed 固定事件（native wrapper 只报一次、无地址/原始 reason）；长期 watcher 使用独立观测上下文，避免关联到已结束短动作。
- 入站活性工作独立冻结其成功修订/显示，避免复用最初 admission attempt 导致本机读取故障标记失效。最终 native 回归运行中。
- 全 workspace、平台、其余旧新矩阵、提交/线上最终检查仍未完成；计划保持 active。

### 2026-10-15 续接
- 确认上一轮 workspace-final-fixed：4227 passed、25 skipped（525 秒）；无活动遗留构建或网络进程。
- rc.15 receive/升级 Joiner 首次失败源于旧 Sponsor 原资料只有启动直连地址，重启后无 relay 路由，旧端握手超时；保留首次及续接复现，不修改旧库／原资料。receive 改为升级 Sponsor，initiate 升级 Joiner，符合双角色独立资料合同；receive Relay 准入及精确双向内容已通过，关闭恢复运行中。
- 原资料额外断言升级前后 transport endpoint 身份完全一致。修复续接时尚未格式化的 native 回归代码。
- 原生七项 nextest 最终通过；根 metadata、workspace 全目标 check（78 秒）、fmt、Rust style、repository architecture、diff check 均通过。
- rc.15 接收角色完整恢复通过：关闭识别 64741ms、Relay 重连 13050ms、就绪后 Online 347ms／双向读回 588ms；cleaned 与 plaintext_clean 为 true。v1 驱动允许旧库自身退避，v2 门限不变；20 秒重连观测首次失败保留。
- 完整默认矩阵重新运行，工件 root 为 resume-required-matrix；先串行构建当前／冻结 main／a07，再执行各格。

### 2026-10-20 续接
- 上轮默认矩阵第二轮因双方底层 Relay 未在驱动额外设置的 20 秒内就绪失败；失败发生在三秒门起点之前。原故障及工件保留。核对新旧 Iroh 均有最长 16 秒退避和 10 秒连接期限，统一给底层就绪 90 秒观测预算并独立记录耗时；失联 20/70 秒及就绪后三秒门保持不变。
- 同源新新 Relay 三轮复现通过：Online 58/364/209ms，双向读回 128/534/297ms，均无整会话恢复，清理及明文检查通过。修正观测预算后 20 轮运行中。
- 上轮成员全集 56 项均在签发邀请错误 1227（无可发布地址）失败。当前单项 fresh_device_join 通过（4.455 秒），成员全集重跑中，暂不认定根因或旧失败已经修复。
- 无遗留构建／网络进程；仍复用原 planning 目录。
- 新新 Relay 修正观测预算后的 20 轮全通过：Online 最慢 354ms、双向读回最慢 569ms，底层重连观测最慢 18924ms，清理／明文扫描通过。
- 成员 G1–G4、自动连接和切 Space 已通过；本地六位码 mDNS 在宿主网络单独三轮为通过／通过／失败，在独立 user/net namespace 同样三轮全部通过。保持实际 5 秒解析期限，不改配对生产行为，继续用既有 nextest runner 在独立网络核验。
- 同版本 Jaeger 2.20.0／Collector 0.160.0 官方独立二进制校验 SHA256 后启动，使用仓库原隐私配置，仅本机端口和内部指标配置作临时调整。Docker 无权限未绕过系统权限。
- 真实 clipboard trace 验收 red：入站成员历史完整能力 span 显示 membership_history_sync，旧接收外层命名发生在 decorator 创建 span 之前。将已有用途描述移到 HandleMembershipHistoryMessageUseCase 的完整 endpoint 入口，删掉接收外层命名；不增加 Engine 查询／port／持久格式。等待当前成员全集结算后串行构建回归。
- 为确保最终二进制对应最终观测实现，oct20-required-matrix 在新新 Relay 第七轮后主动中断并清理；原失败／中断工件保留，后续完整重跑。
- 主线在暂停期间前进 32 个提交至 f0627a8b0bcc274707035a86ab2df47154d1e536，涉及 Iroh git pin、独立网络日志、配对错误及构建缓存。最终交付前保存本片检查点并合入主线；开工互通锚点 b6fa0ce6 不变。已停止等待中的旧基线 trace 构建，避免编译与合并竞态。
- 隔离网络完整成员 nextest：56 passed、14 skipped（670 秒），包括原宿主失败的六位码场景。原宿主失败和隔离证明分别保留。
- 检查点和最新 main 已提交合并，无文本冲突。核对新 ADR-034 与 Rust 固定词表规范；presence 协商协议枚举改用同一 wire_enum! 列表，保持线上取值；主线 Iroh git pin、网络日志与资源普查全部保留。
- 整合最新 main 后，真实 clipboard trace 的原用途命名 red 已转绿：父子树、完成日志、时长及内容隐私断言通过，并经原 Collector 隐私规则导出到 Jaeger。第一次本机重放收到 404，完整重跑通过；失败工件保留。
- 浏览器实际打开默认 business/test 列表，点击 clipboard.copy_and_sync 进入详情；截图及 DOM 证据确认两端 persist、dispatch、receive、write_system 完整，正常 presence 检查／清理不出现在业务列表。
- 整合后的 metadata、workspace 全目标 check、fmt、相对最新 main 的 Rust style、架构门禁与 cargo audit --deny unsound 通过；audit 有既有 yanked/维护状态警告，没有阻断项。开始完整 workspace 构建下的直接相关回归，含 5 项显式真实观测场景。
- 整合后的直接相关回归 108 项：107 passed、1 failed；失败仅为复制 trace 的 Collector 回放 404。连续真实复现捕获到根路径 `/` 被当成 OTLP 回放，改为仅回放已解码／断言的 POST traces/logs；不改变业务或 Collector 隐私过滤。
- 完整 owner 第二 backend 新回归 red：资格读取等待期间最后连接关闭已发布 Offline，迟到读取错误却静默恢复旧 Online。删除尝试显示快照及恢复写入，只标记本机故障，保持当时最新有效显示；等待 green 与完整验证。
- 新竞态回归同一完整 owner 二进制重建后 green（1 passed）；完整相关 109 项仍在链接／执行中。
- `oct20-final-related.log`：109 passed（41.875 秒），包括新增竞态、7 项原生 presence、Uni／coordinator／codec／隐私以及 5 项真实 clipboard trace；Collector 回放已通过。最终 workspace 已开始，预计 4302 项。
- 已提交 `130f7118`（最新显示竞态修复）和 `bfe3484e`（验收适配）。最终 Jaeger 浏览器展开 root 标签确认 `uc.outcome=ok`，partial 导出存在；临时 Jaeger／Collector／浏览器已停止，校验官方下载 SHA256、原隐私过滤正文一致后回收其下载与 profile，仅保留后续隔离测试 runner。
- 最新 main 整合后的 workspace 首轮出现两项失败：reset_space 完整宿主测试超过原 20 秒门；repeated_leaves 任务样本为 66/64/66/66/66/64（句柄 57/56/56/56/57/55），极值窗口断言失败。其余项继续结算，默认矩阵因 workspace 未全通过暂不启动；后续先以同二进制单项及隔离网络最小复现，不放宽产品门限。
- workspace 首轮最终为 4300 passed、1 failed、1 timed out、27 skipped（627 秒）。reset 同二进制宿主单项三轮通过（6.7／15.3／10.2 秒）；禁外网命名空间三轮超过原 20 秒并最终在改密返回 1475。leave 宿主失败／失败／通过（33–42 秒）；隔离网络通过／失败／失败（约 9 秒），任务固定 60 但公网连接重试使句柄在 124–135 波动。已检查探测进程全部退出。
- 两项测试检查本地资料／生命周期却启用了默认公网 Relay。用既有 dev-tools 配置关闭这两项本地测试的公网中转，保留原 20／60 秒和资源断言；真实 Relay 仍由完整网络矩阵验收。开始定点 red／green，未修改生产行为或添加 API。
- 本地生命周期两项定点 green：reset 4.052 秒、leave 9.280 秒；相同新二进制再各跑三轮均通过，reset 3.98–4.59 秒、leave 9.06–9.26 秒。完整 workspace 重跑为 `oct20-final-workspace-fixed.log`；默认矩阵只在其 4302 项全部通过后启动，日志为 `oct20-final-matrix-fixed.log`。
- `oct20-final-workspace-fixed.log`：4302 passed、27 skipped（585.950 秒），包括原两项失败测试。完整默认矩阵已开始串行构建当前／冻结 main／a07 宿主，证据 root 为 `oct20-final-required-matrix`。
- 三个宿主构建通过，冻结 main 的锁适配实际 `--locked` 构建通过（53 秒），a07 构建通过（41 秒）。新新 Direct 文本／文件通过，cleaned／plaintext_clean 为 true。
- 最终默认矩阵第 10 轮新新 Relay 失败：前 9 轮通过，第 10 轮失联 14813ms、底层就绪 10709ms，双方就绪后三秒内未自动 Online；cleanup 和明文扫描通过。首次工件保留在 `oct20-final-required-matrix/new-new-relay/relay.json`，旧新格尚未执行。
- 不改三秒门，给原驱动增加固定分类字段与时间戳的脱敏诊断（无 payload／身份／原始错误正文），用同二进制、同拓扑、20 轮加只查询的延后收敛观察复现；仍按原三秒失败。日志 `oct20-relay-diagnosis.log`，目录 `oct20-relay-diagnosis`。
- 诊断 20 轮全部通过，首次失败 A 的最后原生 Relay 状态为 0、B 为 1，TCP 判据却已把双方计为就绪。核对固定 Iroh 源码：net_report 通过独立 reqwest HTTP 请求访问同一 Relay URL 的探测路径，TCP socket 不能证明 Relay 数据协议已建立。
- 原 TCP-only 判据的回归测试 red（1 failed／5 passed）；改为 TCP 与本轮已有 relay.status.observed 协议连接成功事件同时成立，6 项全部 green。三秒从双方较晚的原生事件发生时间计，日志刷新／读取观测耗时计入三秒，不等待 Online，不额外发内容或恢复请求。新新和旧新共享同一判据；旧 a07 工件也确认原生事件可用。
- 全默认矩阵按此测量修复重跑：`oct20-final-required-matrix-readiness`，总日志 `oct20-final-matrix-readiness.log`。当前生产二进制未改，4302 项 workspace／109 项相关回归仍对应同一生产实现；首次误判和所有诊断保留。
- 修正原生就绪判据后的新新 Relay 20 轮全部通过：失联最慢 17406ms、底层就绪前 19743ms、原生就绪到 Online 最慢 356ms／双向精确读回 577ms，cleaned／plaintext_clean 为 true。冻结 main Direct 接收角色也已通过，继续旧新全矩阵。
- 冻结 main 的 Direct 双角色及 Relay 双角色各三轮全部通过。完整默认脚本随后在 rc.15 旧旧配对的双向内容基线失败（升级前），B→A 被 apply_failed 拒收；全新原资料三次为失败／通过／失败，均发生在两端 rc.15，库包／版本比对保持冻结，仅新增宿主包。
- 第一轮正常关闭并以同一旧库重开原资料的对照通过：旧 Space／传输身份保持，旧旧内容与单端升级 Direct 接收均通过。不改旧库、不补地址／成员资格；继续三轮对照以判断旧库刚配对运行态与闭合资料前置的差异。首次工件和两种结果分别保留。
- 旧旧正常关闭重开后的内容基线又三次通过；主动角色的一次升级阶段因旧端抢先拨入而只有 incoming=1/outgoing=0，不能登记主动方向通过。固定监听端口／外部退避布置反而导致旧端配对超时，已撤回，实验工件保留。
- 现有 IrohNetworkPartitionGate 测试故障边界增加按协议拒绝新入站的能力，负责人仍为原生 gate；Engine 仅经 dev-tools 完整故障动作设置并返回数量，不暴露 Application 状态或步骤。只限制新 Server-side 握手，出站与已有连接保留；清空后恢复，无持久化，进程重启由宿主重新布置，角色验证后、恢复计时前清除。
- 真实双向握手 hook 回归 red→green，出站保持允许、入站被拒、清除后允许；风格通过。宿主独立 inbound-network-faults 特性仅当前默认启用；冻结 main 显式使用原 current-engine 能力集，旧 SDK 库源码与依赖版本不变。当前／a07 宿主重建中，后续须补最终 workspace（预计新增 1 项）及全矩阵。
- 当前宿主构建通过（52 秒）、a07 旧库宿主构建通过（14 秒）；rc.15 Direct 主动角色真实验收通过，incoming=0/outgoing=1，旧旧关闭重开基线、升级前后身份／成员／内容及双向读回、清理／明文扫描均通过。故障限制在实际角色证明后清空，随后恢复仍只能查询，禁止手动唤醒。
- 全默认矩阵再次从头执行，证据目录／总日志为 `oct20-required-final-incoming-control`。生产业务路径未改；仅 dev-tools 原生故障边界扩展，最终仍须补 workspace／成员／升级等门禁、平台 CI 与清理。
- 会话切换为 managed 沙箱后，新工具进程列表只看到自身，旧 write_stdin 会话失效；旧 workspace 日志仍持续增长，不能据新 `ps` 无 PID 判定旧构建停止。预先排队的完整矩阵仍由原进程等待 workspace 全绿后启动；未见最终汇总前不新开 Cargo。后续缓存写入、Git 写入、外网及任务缓存清理须遵守新的沙箱权限。
- workspace 重跑中的原 reset 已通过（4.488 秒），当前尚无失败，等待资源回归及最终汇总。

- 续接核对原进程：完整矩阵 PID 1025423 与最终验证排队进程仍在运行，无重复 Cargo。新新 Direct 与 Relay 20 轮通过；冻结 main Direct 双角色及 Relay 接收三轮通过，主动三轮进行中。规格和稳定设计补充旧旧关闭重开基线及原生入站／出站故障控制的实际边界。用户新增 blob planning 目录同样保留不动。

- 最终完整默认矩阵 11 格全部通过，matrix-summary.json 已逐格核对；清理／明文扫描均 true。新新 20 轮失联最慢 17145ms，就绪到 Online／双向读回最慢 372／577ms；冻结 main 两角色各 3 轮最慢读回 522ms；rc.15 两角色失联 64802／64718ms，均低于 70 秒。最终验证脚本已串行开始 workspace，之后仍须完整回归及平台线上门禁，不归档。

### 2026-10-21 最终回归续接
- 完整矩阵已结算并通过；最终 workspace 共 4303 项，当前暂未报告失败。此前 reset_space 超时项在完整执行中 4.764 秒通过，repeated_leaves 资源项 9.681 秒通过，原时间／资源断言不变。最终脚本仍按计划串行执行，后续组及线上平台结果待结算。

- 最终 workspace：4303 passed、27 skipped（执行 633.977 秒，含构建 1108.291 秒），JUnit 保存为 oct20-final-workspace-with-incoming-junit.xml。完整成员全集已开始，使用 workspace 同构建范围与独立网络 runner；不得把后续未执行项目登记通过。

- 最终成员全集出现首个失败：group_update_delivery::missing_predecessor_recovers_from_another_member_after_receiver_restart，150.623 秒，B 已追上 epoch 5 后 C→B 未确认投影仍为 ReceiverUnavailable，日志 presence／group_update／membership_history 握手超时。首次完整日志保持 oct20-final-membership.log；其余项目继续结算，最终验证脚本会在本组非零后停止。先用同一二进制单项及隔离／宿主对照确定复现率，不延长原 120 秒恢复期限，不据此前通过认定问题已修复。

- 首次最终成员结果：55 passed、1 failed、14 skipped，746.439 秒（日志／JUnit 保留）。同一二进制隔离／宿主／隔离三次目标单项全部通过（36.491／35.927／39.914 秒）；同一 ci 二线程并发 g4 与目标单项两项也通过（152.921 秒，目标单项 37.609 秒）。未改源码、期限、并发或测试配置，当前根因尚未确认，不能声称修复或既有失败。完整原条件重跑 oct21-final-membership-repeat，成功才继续剩余串行验证；再次失败则先增强既有观测缩小复现。

- 同条件完整成员重跑再次为 55 passed、1 failed、14 skipped（735.244 秒），同一目标在 143.62 秒失败，三次尝试后的下一 deadline 为 16:38:43.094，测试于 16:38:43.829 结束，接近已安排的 120 秒退避边界。握手失败日志未按节点关联，可能含刻意离线的 A，不把它们直接归因为 C→B 路由失败。
- 发现已完成组更新计划的 CI 缺口场景明确采用禁 Relay 下固定重启端口，避免把地址重新发现混入历史恢复；当前内进程同类 fixture 未沿用。仅目标场景 B 通过既有 with_test_iroh_bind_port 固定重启端口，其他场景默认随机，保留原 120 秒、真实历史／epoch／ACK 和内容断言。原二进制另存 oct21-member-before-fixture，先定点再完整成员对照，不提前声称生产根因修复。
- 新故障控制还存在 Rust 格式差异，已 just cargo fmt --all；未改变其行为。受影响 workspace 测试产物重建中，后续根 fmt 检查仍须通过。

- 固定 B 端口目标单项通过（36.447 秒）；同一 ci 二线程完整成员组中，该原失败目标通过（36.925 秒），其余场景仍在结算。其他自动连接／重启 fixture 保持原随机端口，生产无变化；首次两轮失败与保留的原二进制仍可复现对照，不把 fixture 收敛称为生产根因修复。既有已完成组更新计划的 CI 固定地址前置提供来源，追加 f062 主线单项脚本仅准备、尚未执行，不能登记通过。
- 重建来源已确认：uc-observability-runtime 默认来源 build script 跟踪整个 crates／tests 树及 Git index；成员 fixture 变化可触发依赖重编。保持 MBX 与共享 target，不在活动构建中切换设置。

- 固定 B 端口的最终完整成员组：56 passed、14 skipped，执行 680.799 秒、含重建 1104.801 秒，56 个非空 result.json 与独立 JUnit 保存。原失败目标 36.925 秒通过，完整组同一默认 ci 二线程配置全通过。升级 smoke 已串行开始；后续未执行项目仍不标通过。

- 后续回归：升级 smoke 五项按原分类通过（3 pass／1 rejected／1 known-incompatible），testkit 18 项通过；Direct／强制 Relay 四类剪贴板各 4 项通过；三进程成员历史 Relay 复用通过（16.58 秒）；固定旧主线组更新 v1／v2 双向两项通过（26.215 秒）；固定旧主线剪贴板互通通过。原网络全集在配对／内容／并发剪贴板通过后遇到驱动 counters 未定义。
- counters 错误由本片扩展双栈工件时误将两个专属字段加到通用 partition 和 IPv4 blockPeerPair 造成；两处无该数组且规则不能代表独立 IPv4／IPv6 计数，已仅移除误加字段。blockDirect 的双栈实际计数与断言保留。首次工件 oct20-final-original-network-artifacts 保留；语法检查通过，oct21-final-original-network-fixed 正从全场景重跑，随后额外 known-peer 三轮，再根门禁。此修正不改变任何 Engine 或测试门限。

- 最终原网络四模式 22 场景全通过（325.316 秒），各模式 cleaned／plaintext_clean 均 true；额外已知设备换址三轮通过（3597／3587／3758ms）。全部根门禁通过，25 项 CI 脚本测试通过，audit 无阻断项、保留既有 9 条允许警告。20 个本地阶段汇总校验通过。
- 无活动本任务网络／Cargo 进程，已回收本次 a07／a13 源码 src 与观测缓存；共享 target／MBX、旧端二进制和证据保留。隔离 runner 复制到工件目录用于复现，未执行的追加主线对照不记通过。开始最终差异审查及提交／PR，平台线上结果尚未执行，不归档。
