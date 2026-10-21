# 进度

- 2026-10-21：读取 implement、uni-rust-development、planning-with-files 与仓库约束；fetch 并新开分支，保留已有改动。
- 尚未实现生产代码或执行阶段 0 技术验收。

- SDK 源码：Remote 私有 get_blob_ranges_impl 在网络 write 错误时 try_join 提前退出，丢弃 import rx；store import 在独立运行时执行。wait_idle 是整个 store 的全局等待，不能视作单项停止。
- 新增独立集成探针：真实 QUIC + FsStore，通过测试 subscriber 暂停 import worker，再注入网络重置，检查 SDK 错误返回是否等待 worker。
- 首轮编译失败：let_underscore_lock 拒绝未绑定 MutexGuard；改用具名 guard，首轮日志保留 probe.log。
- 第二轮探针已到退出清理，但重复关闭 provider store 返回 Sender closed；Router 的 BlobsProtocol 已关闭同一 store。删除重复 shutdown，失败日志保留 probe-fixed.log。
- 停止探针修正后通过（1 项，0.13 秒）；扩展同连接 sibling 内容读回时出现 SDK error 3，尚在定位测试是否混入了额外完成语义问题，日志 probe-sibling.log 保留。
- sibling 扩展固定失败在 SDK get 的 stream reset error 3，原因未证明。该扩展不属于最小停止复现，撤回扩展并保留失败记录；不能宣称同连接双流或内容读回通过，相关验收保持未执行。

- 最终最小探针通过：1 passed（0.14 秒）。根 metadata、workspace all-targets check、fmt、Rust style、Engine repository preflight、文档相对链接和差异检查通过。
- workspace 分组完整回归运行中（4306 项）；成员全集、升级矩阵、设备和线上 CI 未执行。
- 手工审查确认无生产接口、协议、持久格式和依赖来源变化，测试全局 subscriber 仅在独立探针进程使用；code-review 技能未安装，不假称已调用。

- 完整 workspace 分组结束：555.896 秒，4305 passed、1 failed、27 skipped，退出码 100；新增探针 0.145 秒通过。
- 唯一失败 peer_address_read_diagnostics.rs:270 的符号状态 unresolved／captured；对应生产与测试来源均未改动。独立运行既有二进制仍失败（退出 101），保留 diagnostic-existing-binary.log，不放宽断言，不声称整轮通过。
- 一次用 workspace Cargo 命令复核诊断项触发无必要的全目标重建，已终止该命令组（143），确认其无存活进程，改用既有二进制复核；原日志 diagnostic-rerun.log 保留。
- 本次未创建外置 target、临时旧库或真实资料副本；只保留共享编译缓存。阶段 0 仍阻塞，后续生产阶段未启动。

- 续接：已重新核对分支／工作区／goal，上一 goal 相关工作留下可复现证据；本次继续验证底层 SDK 的完成等待权，不把高层 API 失败扩大为全部 SDK 不可用。

- 已验证固定 SDK 底层 get/import 与 provider/export 原型（managed-provider-probe.log，1 passed，0.44 秒）：暂停同次本地工作时取消不提前完成，兄弟流内容正确；未新增 SDK 依赖。
- 已定位旧 sibling error 3：Raw blob 使用 GetRequest::all 错当 HashSeq；改为 blob 后同连接读回通过，原失败保留。
- Uni 原型首轮编译发现 SDK pair.get_request 的 &self future 要求半流 Sync，改用当前生产未开启 SDK 进度追踪的 RequestTracker::NONE，避免将 Sync 限制扩大到 Application；Blob 新协议分类须在组更新 wire 中显式拒绝，已补齐。

- 真实 Uni 原型首次已通过（0.17 秒）；补充无效 reset／stop code 的稳定分类、source 与连接保留后再通过（0.09 秒）。固定 SDK 接收角色的普通与空 blob 对替代 provider 也通过（0.40 秒）。
- 续接相关回归全部通过：Uni 29、原生 connectivity 3、替代接收生命周期 6、Application transport 20；根 metadata、workspace all-targets check、fmt、Rust style、repository preflight、相对文档链接与差异检查通过。日志见 resume-validation.log 与对应命令日志。
- 本轮未重跑整套 workspace／成员全集／旧端矩阵和平台 CI，不沿用上一检查点的完整回归结果冒充当前 head。完整 spec 完成前仍须完成其全部必需门禁。
- 手工差异审查：通用半流变更仅扩展传输无关能力，稳定 Engine／绑定未扩大；I/O 分离后的连接与许可保留至实际本地工作退出。所有 SDK 替代流程仍为测试原型，生产 blob 路径未切换。
- 下一步：原始接收项材料与身份重建、旧端当前授权及资料角色、协议／资源基线前置；原型采用时迁入唯一 Infra 实现并删除测试副本，再推进调度和生产接线。

- 续接已重新核对工作区；上一 goal turn 为 progress（cfba06c4 提交了真实 SDK／Uni 原型和通用工作租约）。当前继续阶段0，不将局部证明冒充全 spec。
- 新增材料原型首轮编译误用 FetchTransferContext.from_device_id，真实字段是 peer_id；已更正，首轮日志保留 retry-material-probe.log。
- 材料测试两次后续编译暴露 peer_id 是 String、DeviceId 的实际访问器为 as_str（不是 as_ref）；已按真实定义修正，日志分别保留 retry-material-fixed.log 与 retry-material-corrected.log。
- 原始材料两项探针通过：首次真实物化失败后，新 attempt 从保留 V3 恢复相同 ticket／来源并写回正确文件；重复名称与重复内容按原位置区分。此为材料可行性，不冒充稳定 RetryEntryReceive API 已完成。
- 开工 main a461b894 冻结宿主构建成功，复用原冻结构建脚本并支持显式 SHA／输出目录，未修改旧 Engine。临时源码由原 trap 清理。
- 1GiB 首跑完成内部传输／校验／资源断言后，在写计数工件时失败：Cargo 测试 cwd 位于 crate，驱动误传相对工件路径。保留 resource-baseline.log，使用绝对工件路径重跑；不修改测试断言。

- 1GiB 资源基线重跑通过（16.50 秒）：逐块精确校验，RSS 峰值 47024 KiB，释放 tag 且无 SDK 临时文件；生产仍是旧适配器，连接总数／峰值均为 2，此为迁移前基线，不是新路径验收。
- rc.15 Direct 单边升级首组通过：旧旧双向文件、升级后原文件可读、新旧双向文件均精确校验；成员／历史／身份保留。完整升级矩阵仍待补齐。
- 真实 Uni 当前准入及同内容探针通过：原生 connectivity 4 项，0.23 秒。使用生产 IrohConnectionAdmission，成员读取为可变 fixture；撤权、重新加入后的旧 epoch、会话关闭均拒绝旧许可，不能把有效 ticket 视为当前授权。不是实际 SQLite 成员与完整 Engine 新流程的验收。
- 正在未修改的 rc.15 库中构建独立协议向量 example，使用共享 target；旧库与锁文件不打补丁，仅添加合成向量入口。

- rc.15 协议向量生成完成（2m21s），临时源码退出后回收；当前 SDK 对照 Raw／空负载的 ALPN、ticket、Get 请求、BAO 响应完全一致，1 项通过。旧 SDK 为 0.102.0 的 b33af91 固定 fork，不是 0.100。
- 正在串行构建当前宿主与 Relay，用于补齐两个冻结旧端 × Direct／Relay × 单边升级方向的迁移前资料矩阵。

- 8 组迁移前旧资料矩阵全部通过，汇总逐项核对内容／旧文件／保留身份成员历史／副本／清理／明文探针；摘要落在 target，两个临时冻结源码目录均不存在。汇总首轮误按固定 presence 后缀找第 1 侧工件，修正实际后缀后完整核验，不重跑网络。
- metadata、workspace all-targets check、fmt、Rust style、repository preflight 通过；preflight 首轮触发内部 Cargo 等待，已中止，构建结束后串行重跑通过。完整 workspace 首轮失败历史不变。
- 阶段 0 三项技术证明完成，文档状态统一进入阶段 1；不把原型或迁移前矩阵当作正式受管路径硬验收。下一步实施公平有界调度及精确预算。
- 阶段 0 已提交 dc0c3b22。阶段 1 最小切片：transfer 内部 DownloadScheduler 原子接收一个逻辑项，返回持有至实际结算的非克隆许可；排队满／到期／取消／关闭明确拒绝。调度不掌握 SDK，运行中任务停止与重试由后续 transport 完整负责人监督，重启不恢复队列。先用可控 Tokio 单调时钟验证 4／2、64／16、60 秒、跨端轮转和 drop/关闭竞争，再接实际交换。
- 已实现 DownloadScheduler 初版：原子容量检查、同端 FIFO／跨端轮转、60 秒单调排队期限、future 被丢弃或取消时精确撤销自身预留；实际许可记录真正授予时刻，迟到 poll 不重置首数据计时。关闭只封口等待项，运行工作保留许可。定向测试开始编译。

- 调度首轮编译失败为测试使用 DeviceId::from(&str)，实际显式构造器是 new；已修正并保留 scheduler-first.log，生产调度无同类调用。
- 调度 7 项通过（0.01 秒）：4／2 活跃、64／16 排队、跨端轮转与同端顺序、恰好 60 秒到期、未 poll 的到期项释放队列、取消／丢弃交接、自身预留回收、真正授予时刻与关闭封口。
- 已新增单项 ReceiveTiming：首数据／停滞期限、重复已验证前缀不续期、累计恢复 30 秒暂停而不归零、4 次真实尝试与固定退避；定向 receive 分组开始。尚未接生产 fetch，不能宣称正式网络预算生效。

- transfer 分组 38 项通过，包含并发 80 个任务只接受 64 个等待者且取消无残留。首次 style 检查拒绝受限可见性的 crate:: 路径和原因枚举上的纯转调分类；改为祖先相对可见性，并只给主错误提供固定原因分类，修复后 style 通过。
- 正在对最终规则模块串行跑 transfer 定向与根门禁。后续直接实现专属 SDK 交换 port 与完整 transport；当前生产路径仍旧，不用旧测试替代正式迁移验收。

- 最终规则分组 38 项通过（0.02 秒），workspace check 通过。根 preflight 发现已提交的向量脚本 -p uc-infra 被当前包选择器门禁拒绝（旧 monolith 已拆分）；改为直接指定冻结旧 crate 的 manifest，不增加门禁例外或放宽当前工作区检查。重跑向量生成和根门禁。

- 最终检查完成：transfer 38 passed（0.02 秒）、metadata、workspace all-targets check、fmt、Rust style、repository preflight 与差异检查通过。旧向量脚本改为直接使用冻结 crate manifest，重跑 5.76 秒通过，fixture 字节未变化；首轮包选择器门禁失败保留。
- 手工审查调度登记／预留 Drop／到期清除／关闭、授予时刻与恢复累计路径，均不扩大稳定入口或引入 schema。未安装 code-review skill，仍为手工审查。临时旧源码和矩阵进程均已结束回收；用户原有四个规划目录保留。
- 当前仍未完成完整 spec：规则模块尚未接生产，旧 SDK Downloader、warmup、对端级取消与旧 provider 仍在；后续必须完成 phase 1 专属 port/资源策略、phase 2 实际迁移与监督、phase 3 部分结果和稳定重试/绑定、phase 4 删除旁路，以及 phase 5 全部必需验收。完整回归未在新规则 head 重跑，原符号化失败和平台/设备跳过记录保留。

- 续接 ab3109fc 为 progress：已提交阶段 0 证据和阶段 1 调度／时间规则，当前工作区只保留用户原有四个规划目录。
- 正在迁入 Application 专属 BlobExchangePort 与 Infra 唯一 SDK 实现，固定 SDK parser/encoder、不使用 Endpoint。成功结果携带不透明 TempTag／StreamWorkLease 保护，引用／输出结算后才释放。取消不丢 import 准备／完成等待，提供端排空同次 export。
- SDK 源码盘点发现 into_byte_stream 的通道错误会映射为 EOF；提供端完成前补同一 store 状态核验与至少合法响应头的检查，具体故障覆盖仍须验证，不能仅据源码宣称异常路径完备。
- 交换首轮 check 报 TempTag 的真实路径为 api::TempTag、AtConnected::new 首参为 std::time::Instant；已按固定源码修正，保留 exchange-first-check.log，继续定向编译。
- 唯一 Infra 实现替代原型后，真实 Uni connectivity 4 项通过（0.25 秒），含当前生产准入与同内容取消隔离。SDK 暂停 import/export 的复合探针迁入同一 port 后通过（0.46 秒）；已删除 tests/support/blob_exchange.rs 重复算法，新增仅测试的预连接租约 fixture。
- 新增具体网络错误白名单、显式 QUIC 流停止不重试、结算复合错误保留双来源及 Debug 隐私测试。合法空 blob 沿 SDK EMPTY 始终完整语义，不再 add_bytes 创建自动永久标签；扩展真实空／已有本地内容、未完整请求头取消与恰好到期探针，正在验证。
- 二轮 check 暴露 SDK 的 n0_future::time::Instant 在当前平台实际为 Tokio Instant；已修正，保留 exchange-fixed-check.log。未修改固定 SDK 修订、ALPN 或线上编码。

- BAO 长度辅助首轮 compile 暴露 private rec 模块及 ChunkNum 类型；改用公开最小树 + SDK 分组层数。扩展尾部范围用例又发现 16KiB 预期多出 192 字节，归一化末块尾部为开放范围后全部通过，exchange-size-check.log／exchange-size-expanded.log 均保留。
- SDK worker 真实故障、来源不存在的空 FIN/SourceUnavailable、普通/空/本地已有内容、header 取消/期限都通过。SDK helper Drop 的隐式停止错误改由 IoCompletion 结算，header 已发送后注入 stop PermissionDenied，确认不能返回普通取消且原 IO source 可追溯。
- 最终 reviewed nextest 11 passed、312 按过滤跳过（0.527 秒），metadata、workspace 全目标 check、fmt、Rust style、repository preflight 与差异检查通过。锁文件结构比对仅当前 Infra 增加既有 bao-tree 依赖边，全部来源/版本不变。
- 已手工审查 SDK 完成等待、内容/工作保护、EOF 与响应长度、空数据、隐式停止失败、错误链和隐私；没有 code-review skill，不冒充技能审查。当前没有运行中的 Cargo 或网络矩阵工作；生产旧路径尚在，阶段 1/2 全部退出条件未满足，完整目标继续。

- 续接 0ca4ed1c 为 progress：唯一 SDK 交换实现、实际 Uni 探针及异常结算已提交，工作区保留用户四个规划目录。
- 本轮最小完整切片：Uni 的 ConnectionRequest 在 Blob 自有队列之前冻结准入／会话基线；同一对象用于重试，撤权后旧对象不能因新 epoch 复活。迁移现有消费者并删除原 connect/connect_with_cancel 入口，不保留仅加默认参数的包装。之后接完整 transfer 监督与独立资源策略，稳定 Engine 入口不增加内部阶段查询。

- 上一状态答复 turn 为 no progress（只确认 98 项结果并汇报）；本轮已继续实际修改和验证。增加共享等待者结果分类、最后等待者取消／到期后保留拨号容量且保留真实 backend PermissionDenied 的测试，定向 connectivity 最终 99 passed，0.22 秒。
- workspace 首轮失败为 group_update_adapter 新 import 放在 //! 之前，修复后两次全目标 check 通过（51.04／57.72 秒）；metadata、fmt、Rust style、diff check 已通过。正在执行 repository preflight 和差异复核，不把本步当作 production Blob 迁移完成。
- repository preflight 通过；手工复核了所有内部调用方、原 epoch 的 refresh、共享 flight 最后等待者、准入读取实际结算与复合来源。没有可用 code-review skill，仍如实记为手工审查。补跑迁移后的真实 Uni／SDK 定向 nextest，完成前不提交。
- 迁移后的真实 Uni／SDK nextest 5 passed、317 按过滤跳过（0.513 秒）：包括原生准入、同／不同内容的双流独立停止及 SDK 实际工作结算。根必需检查与文档相对链接、差异检查均通过。完整 workspace 测试和最终生产矩阵本轮未重跑，原已有 profile 诊断失败记录保留。
- 当前小步只完成请求围栏与 Uni 本地工作监督，未达到阶段 1 全部退出条件；下一步仍是独立 blob manager 的资源策略与完整生产 owner，不修改完整 spec 范围。

- 续接 a714953a 为 progress；工作区仅用户四个原有规划目录。当前最小切片：Uni 完整拥有入站准入，入口同步冻结会话／撤销并申请未知额度；Infra 识别后通过准入预留回调原子交接每端额度，之后才读成员权威。成功返回同一完整连接租约，失败保留来源；已开始的本地读取由原准入 future 结算，不新增持久恢复。blob 最终配置 16／2／60 秒；本步以真实消费者编译及门控读取／可控时间测试验证，不冒充方向连接限制或生产 owner 已完成。
- 入站首轮 workspace check 仅报 checked 泛型化后纯 Err 测试缺少成功类型；已明确标注 Result<(), _>。实际生产与全部 trait 消费者已通过类型检查到这一处。为防接受方丢弃等待后遗弃本地读取，启用监督的入站准入也交由 manager 的 JoinSet 持有；取消会话后关闭原连接，但实际读取未结束仍保留准入额度。
- 新入站定向 105 项通过（0.23 秒）：未知 16、识别后每端 2、交接后未知名额复用、先撤销后重加、60 秒慢读结算保留 PermissionDenied、会话关闭等待与 panic 来源。继续实现方向名额覆盖建立／本地结算／真实关闭，避免只限交换数仍积累连接；现有 105 项在首轮 lifetime 变更后再次通过（0.22 秒）。
- 方向名额测试首轮 107 passed（0.26 秒），补齐入站方向隔离及关闭／本地结算两种先后顺序后 108 passed（0.23 秒）。复核另修复两处责任边界：过载立即返回，不为拒绝再 spawn 监督任务；连接监视持冻结会话 token，关闭收集包含晚返回拨号登记的结束任务。后续检查先暴露嵌套 Either 的括号错误，修复后又暴露 PendingInboundLease 的 I/O 非 Sync；已将预留上下文缩至可共享的围栏／会话／预期身份，不要求 I/O 具备 Sync。首轮日志均保留。
- Application 全 lib nextest 1148 passed／1 skipped（3.803 秒）；新增晚返回拨号关闭收集用例另 1 passed。最终 Native Blob profile 移除内部阶梯重试，每一轮只调用一次 Endpoint.connect；现有成员 grant 仅对 Blob 开放经当前准入检查的空闲保留。真实 Uni 双流探针启用 16／2 准入、每端每方向 1 连接及 30 秒空闲保留，并调用真实 retain 以验证当前授权接线。生产 Downloader/provider 尚未替换。
- P2P 全 lib＋blob_connectivity_feasibility nextest 319 passed／3 skipped（38.747 秒），包含启用新 16／2、方向 1 和保留 30 秒策略的真实 Blob 双流、当前授权与 SDK 结算；原普通剪贴板拒绝 ACK、成员历史和前六片相关测试均在本次范围。已手工复核同步过载拒绝不 spawn、未知／识别交接、两种结束顺序、旧 session、不扩大稳定 Engine/API、Blob 单轮原生建连及来源保留。完整 workspace 测试、正式 owner 矩阵与原 profile 诊断失败仍未结案。
- 最终 metadata 输出已解析核验，workspace all-targets check 通过（1m14s）、fmt、Rust style、repository preflight、文档相对链接及 staged diff 检查通过。code-review skill 不在可用清单，采用手工差异审查，不冒充技能审查。用户四个原有规划目录保留；本轮不修改 SDK／ALPN／持久格式，不把过渡资源能力算作正式 Blob owner 完成。

- 续接 4157ed46 为 progress。当前切片将生产 Router 提供端交给 Application BlobTransport：调用方只交接原生连接，内部完成现有准入、4／2 即时交换限额、整体请求截止时间、SDK 导出结算和关闭。完整接收／手动重试仍是同一最终 transfer 目标；本步先替换旧生产 provider，不改变协议或持久格式。关闭协调者保留任务与结果，调用等待退出不丢工作；本地 store 在完整收尾后关闭。
- 首轮 App check 通过（19.45 秒）。全工作区首轮仅 Node 测试引用不存在的 NoopPeerReachability，已改为复用原 fixture 的 reachability；修复后的全工作区 all-targets check 通过（1m12s）。生产 Node 已删除 BlobsProtocol 注册，改为 IrohBlobHandler／Application BlobTransport，并用真实 store 生命周期 port 保留关闭结果。
- 未改造生产 Downloader 在额外空预热仍打开时经新 handler 精确读取 512KiB，真实 UDP／SDK 集成 1 passed（0.050 秒）。受管负责人 4 项 nextest passed（0.028 秒）：4／2 超额只拒绝本交换、首请求整体 60 秒、5 秒未完成不丢收尾、导出与 store 关闭双来源保留及重复关闭。普通空预热无工作按 30 秒回收，不占已授权名额。
- 真实 Node.quiesce 测试通过：SessionProtocolDispatcher 已取消 handler 等待，但原生连接持有的 SessionLocalWork 持续覆盖 Application／SDK 实际工作，释放门后才排空；与真实 Downloader 用例一起 2 passed（0.090 秒）。增加 SDK 服务任务 panic 唤醒监听并保留 JoinError，避免等 30 秒空闲才发现失败；收集兄弟结果时保留同一个 accept future，不丢已开始的准入读取。
- Application 全 lib nextest 1154 passed／1 skipped（3.815 秒）。真实 Engine 使用当前加密资料完成 Direct、强制 Relay 双向文件精确读回，各 2 方向，Relay 工件确认 direct_paths_blocked；两组 cleaned／plaintext_clean 均为 true，工件在 target/blob-connectivity-evidence/provider-{direct,relay}。本机提供端与当前旧 Downloader 的互通证明，不代替最终双方完整 owner 矩阵。
- P2P 全 lib 与 SDK 生命周期／旧协议向量 nextest 322 passed／3 skipped（39.044 秒）。连接工作许可调整为保持到父监督任务发布结果结束，完成结果持续重用原 registry 并在新请求时回收；不让已完成但未收集的监督任务绕过名额。
- 复核发现额外全局连接工作 semaphore=16 会在缓存全空闲时拒绝第 17 个对端，与原缓存淘汰能力不一致；已移除这项非 spec 的服务总数门槛，改为父监督者持原 Uni 租约到结果交接。每端每方向一个授权连接继续覆盖全部任务寿命，未知候选16／服务4／2不变；新增 17 对端空闲缓存淘汰用例验证不收窄原需求。
- 缓存复核修正后的 6 项 runtime 测试通过（0.019 秒），Application 全 lib 最终 1155 passed／1 skipped（3.784 秒）；真实 Downloader、真实 Node quiesce 及旧协议向量复核 3 passed（0.091 秒）。移除额外服务门槛后第 17 个对端可通过淘汰空闲连接获得服务。
- 新增当前受管提供端真实 SDK 双流：接收窗64KiB，UDP统计确认正文开始后取消停读的 A；B 同一连接／一次获取精确4MiB，backend调用数仍为1；第三个正文中途断开对端后提供端 shutdown 实际成功。1 passed（0.167 秒），无需按“连接消失”吞停止错误或修改协议。最后执行全P2P／SDK／向量分组及根门禁；下载与稳定取消仍旧。

- 本轮最终 P2P／SDK／协议向量 323 passed／3 skipped（38.754 秒）；metadata JSON 解析、workspace 全目标 check（16.19 秒）、fmt、Rust style、repository preflight 全部通过。再次手工复核实际任务持有、关闭竞态、准入首流时钟与逐流停止；code-review skill 不可用，如实采用手工审查。上一状态答复只汇报状态，记为 no progress，本轮继续提交生产提供端并推进下载。

- 提供端已提交 6d0c13c1；开始修复下载恢复所需的旧连接寿命等待。refresh 改为消费旧 lease，保存原资格／会话，退役旧缓存，在方向名额实际释放后重拨；关闭和本地工作任一未完都不能提前拨号。首轮编译发现 membership history 也调用该入口，已同步迁移；会话测试改为监督异步 disconnect_all，未丢弃未 poll 的 future。日志 refresh-lifetime-tests.log 保留。

- 重连修复的 Application 全库 1157 passed／1 skipped（3.863 秒）、workspace all-targets check（1m18s）、metadata JSON、fmt、Rust style、repository preflight 和 diff check 均通过。手工审查消费旧租约、缓存强引用、寿命通知先归还许可再唤醒、重复会话及全部调用方；本轮没有启动全 workspace 测试或重跑最终网络矩阵，不把原 profile 诊断失败结案。

- 续接 61423f3a 为 progress，工作区仅用户四个原有规划目录。当前迁移不再扩展提供端测试：实际 FetchBlobUseCase 必须改用受管获取，并持下载许可及内容保护覆盖输出／解密、引用和 tag；外层 select 不能丢弃实际任务。ticket 来源需以真实身份目录解析，开始解析前冻结原会话／撤销围栏，不能在外部队列之后新授资格。旧 fetch 还用 ticket 完整地址预热；新路径必须把该地址作为本请求的不透明拨号提示传给 Uni，不建立第二套原生连接缓存或为地址再拨一次。
- 已开始把票据地址改为本请求不透明提示，沿共同 flight、失活刷新和唯一 backend 传递；不持久化、不增加地址缓存。首轮 all-targets check 仅发现四个测试直调 backend 缺少新参数，已迁移；download-hint-check.log 保留。
- 实际下载代码已开始取代 FetchBlobUseCase 中 Core fetch/fetch_to_path 调用：新增纯本地 read/protect/export port 与票据来源能力，Engine 组装同一 BlobTransport/FsStore；下载许可和 TempTag 持到输出、解密、引用及标签完成。正在编译及迁移 fixture，尚未宣称本轮接线可交付。首轮 owner 编译暴露新增取消分类的穷举分支，已补齐；接线编译暴露批量替换导致结果构造括号多一层，已修正，原日志均保留。
- 为避免异步身份查询在队列前形成无界隐藏等待，调度键改用票据的认证公钥（不作为业务 DeviceId），同步 inspect + reserve 后才监督工作；活跃许可起算覆盖权威 DeviceId 查询、当前准入和网络。身份查询前的原会话／撤销围栏持续由 ScopedAttempt 保持，地址提示只经过唯一 Uni backend，不落库。
- 新生产组装已通过一次 workspace all-targets check（1m17s），当前继续修正取消实际结算，不能将该中间编译作为最终交付。旧 facade 对端级 shutdown 调用已删除，获取 future 的外层 select 已删除；唯一监督任务返回不透明完成通知，取消协调者先触发本流取消，再等待真实获取／本地工作及本次会话结算，五秒超时只结束调用等待。反向通知由同一 transport 监督一个有界、按任务合并的 actor，不再直接 await reporter。实际测试及最终检查待完成。
- 下载获取／同步调度／原连接围栏／提供端定向 nextest 135 passed（0.122 秒），含原文件引用及内存解密回归、取消慢本地工作五秒返回未完成并重复等待真实结束、不调用旧对端级 shutdown。开始当前生产 Engine Direct 文件端到端验收，实际获取应全部经过新 Uni；尚未把进度 actor、重试分类或完整接收提交的余项标完成。
- 当前生产获取 Direct E02-file-transfer（275ms）与强制 Relay E02-relay-only-file-transfer（147ms）均双向精确通过，工件在 target/blob-connectivity-evidence/download-{direct,relay}；这两组是提供端和下载端都已接线的当前资料验收，不替代最终旧新／资源矩阵。
- 永久阻塞 reporter＋本地慢读用例在调用方保留和丢弃两种情况下均通过：首个取消五秒明确未完成，重复调用接同一次实际收尾，释放慢读后取消完成；不调用对端级 shutdown，transport 关闭收集通知 actor。定向 135 passed（0.122秒）。Rust style 首轮可见性路径误报已用相对祖先范围修正。重试分类正在接入 native 错误白名单；首轮显式 Option 类型推断失败已修正，日志保留。

- 当前迁移最终 P2P／SDK／向量 324 passed／3 skipped（39.232秒）。全库 App 第一轮新增身份慢读测试发现取消来源被宽泛 Connectivity 包装成了 Settlement，已统一调度／Uni 语义分类并修正，定向1 passed（0.029秒）后全库1159 passed／1 skipped。原失败输出、缺少调度错误导出及异步结果类型标注的编译日志保留。
- 复核发现新五秒未完成不能落回旧条目取消清理：已让条目取消保留 Cancelling 并返回 CancellationIncomplete，不做 projection/staging/terminal；旧扫描也跳过仍由受管负责人持有的任务。Engine 显式同名结果走固定 wire_enum，未改持久状态。首轮宏 serde 分支误写类型冒号，已按实际宏语法修正；完整后台条目取消／薄绑定仍未完成。
- 一次登记回收脚本未找到预期替换，未写入文件；已改用明确定位完成通知的登记块，工作区实际 helper 负责回收已完成但等待方退出的登记，并拒绝覆盖同一活跃身份。误触重复 App 验证结果仍保留为证据，不冒充新增覆盖。

- 取消与到期竞争采用红绿验证：30秒先取消、60秒仍有慢身份读取时原实现错误返回 RequestExpired（download-cancel-before-deadline-red.log）；修正后取消分支持有真实工作，抑制后续期限改写结果。未取消项仍按60秒到期，两者真实读取结束前都占名额；新输出启动前再次检查停止意图。
- 最新源码最终 App 全库 1160 passed／1 skipped（3.833秒），Engine公开契约50 passed（0.021秒），workspace all-targets check（1m15s）、metadata JSON、fmt、Rust style、repository preflight 与 diff check 全部通过。真实Engine双向文件最终 Direct277ms／强制Relay212ms，各精确2方向，Relay直连阻断，cleaned/plaintext_clean均true。最终工件为 target/blob-connectivity-evidence/download-settlement-final-{direct,relay}，先前候选证据不冒充此轮最终候选。
- 本轮手工差异审查已覆盖同步容量／原围栏、唯一地址提示、下载实际工作持有、取消早于期限的先因、永久通知阻塞、五秒未完成与 staging／扫描隔离、类型来源及无持久格式变化；code-review skill 不可用，不冒充技能审查。完整Receiver计划与自动取消全流程、单项重试／绑定、删除旧入口及最终全矩阵仍未完成，目标保持 active。
