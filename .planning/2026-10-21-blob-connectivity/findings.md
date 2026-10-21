# 发现

- 起始分支 feat/active-clipboard-connectivity，HEAD 2154b959；保留原有规划目录和未提交文档。
- 已 fetch origin/main，冻结基线 a461b894；新分支 feat/blob-connectivity 从该基线创建。
- spec 明确阶段 0 任一技术证明失败／无法证明即设计阻塞，不允许提前替换生产路径。
- implement 要求最后 code-review；当前技能目录未发现该技能，继续查找；如缺失执行等效差异审查并如实记录。

- 停止前置失败已复现：固定 SDK 网络错误返回时，受测试门阻塞的同次 import 仍活跃。源码对应 remote.rs get_blob_ranges_impl 的 try_join 提前丢弃私有 handle.rx。
- 全局 Store::wait_idle 不是单项 join，SDK 文档明确并发时可能永不空闲。
- 不宣称所有低层 SDK 组合不可行；可选 fork 完整 handle 或 Infra 持有 get::fsm／ImportBaoHandle 两份完成权，provider export 也须证明。
- /code-review 在可访问技能目录不存在；本轮执行手工调用图与差异审查，不假称调用该技能。

- 续接已证明原阻塞仅限高层 Remote::execute_get：公开 get::fsm + ImportBaoHandle 可保留同次完成权；SDK provider parser + export byte stream 可在取消后排空本次 export。
- SDK Raw blob 应用 GetRequest::blob；旧 sibling error 3 来自误用 all（HashSeq），已纠正并成功读回。
- 本机真实 Uni ALPN、共用连接、并行暂停 A/B 后取消 A、B 精确读回4MiB通过，backend 仅一次获取；实际原生半流显式 stop/reset，StreamWorkLease 在 I/O 释放后仍保留额度。
- 当前阶段0不再是 SDK 技术无路可走；仍需原始重试材料／历史端当前成员授权与资源等前置证明，未切生产路径。

- 阶段 0 前置已完成并提交 dc0c3b22：原材料两项、真实 Uni 当前准入和同内容隔离、固定旧 SDK Raw/空向量、两旧锚点 × Direct/Relay × 升级两侧共 8 组资料基线、1GiB 迁移前资源均已核对。正式新 owner 的验收不能沿用旧适配器结果。
- 阶段 1 已实现但尚未接生产的规则在 transfer/receive/scheduler 与 timing；单调授予时刻保存在 DownloadPermit.started_at，不能从 acquire 返回时重计首数据预算。调度器不 spawn 网络工作，后续完整 owner 必须把许可移入受监督任务，不能在取消调用或外层 future drop 时直接释放。
- 生产取消仍有已知待迁移点：facade 先 remove 注册、await reporter、cancel select 丢弃 fetch、shutdown 对端连接、吞 session 清理错误；不能简单把新队列套在这条路径上假报独立停止。实际改造须让原任务持续被监督，5 秒仅结束调用等待，重复取消等同一结算。
- 现有 SDK fetch/fetch_to_path 的 TempTag 只覆盖 adapter 方法，到永久 tag 还有微小窗口；新交换结果须保留 store 保护，直到引用/输出/tag 必要结算，不复制“理论窗口可忽略”的旧理由。
- 现有 Uni policy 的 incoming 非 presence 默认取 waiters_per_session，accept 以连接为单位占该许可；accept_bi 默认先等许可再取流。blob 的未知入站 16、交换 4/2、每方向一条连接且退出未完成仍占名额须独立落实，不能仅设置现有几个数字就宣称满足。也不能保留第二份原生连接缓存。
- 下一步：Application transfer/blob/transport 的专属完整交换 port；仅接收已准入 Uni StreamLease，SDK/BAO/store 留在 Infra。下载及提供都必须监督准备、import/export 与输出，取消/超时发送停止意图后继续 join，不丢弃 SDK future。采用时迁走并删除 tests/support/blob_exchange.rs 原型副本；生产仍旧，阶段 2–4 全部待完成。

- 本轮已实现 Application BlobExchangePort 与 Infra IrohBlobExchangeAdapter，前者只有 Uni StreamLease/内容身份/取消上下文，后者只有 FsStore。接收保留独立 import rx，提供取消后排空 export，成功 receipt 保留 TempTag + StreamWorkLease；tests/support/blob_exchange.rs 已删除，旧探针全部调用同一实现。
- SDK into_byte_stream 把通道异常隐藏为 EOF；不能只查 store 状态／至少 8 字节。现在用 SDK 同源 bao-tree 遍历器的预期长度 + 实际长度 + store 状态判定完整响应，编码仍是 SDK。直接 bao-tree 仅新增已有锁版本依赖边。公共遍历须按 SDK 组层数压缩，并把触及／超过末块的尾部归一化为开放范围；首轮 16KiB 稀疏用例曾差 192 字节，修复后全部比较通过。
- 停止错误不能只在显式 cancel 分支捕获：SDK 准备／请求头 future 被取消时可能通过 Drop 停止半流。IoCompletion 最多收集两半流各一项兜底失败，真实 header 等待时注入 stop 权限失败已证明保留 IO source，不能返回普通 Cancelled。
- SDK helper 成功不等于业务完成；当前 production facade/Router 仍旧。下一步必须把全局 DownloadPermit 移入受监督的完整工作任务，覆盖网络、输出、引用与所需结算；调用方取消等待不丢任务，5 秒只结束等待，通知非阻塞。还需 ticket 来源解析／当前身份映射、独立 manager 方向名额／入站资源与 30 秒空闲、生命周期及实际 Node/Engine 接线。
- 后续调度接线要特别处理撤销基线：现有 ConnectivityManager.connect_with_cancel 在自身排队前 begin(target)，但 blob 的 DownloadScheduler 在它外部。必须在逻辑请求接受／外部排队之前冻结同一准入基线，并贯穿复用、开流和提交；不能等拿到下载名额后才取新 epoch，导致排队旧任务在撤销／重新加入后复活。

- 外部队列围栏已改为同步 ConnectionRequest；manager 的旧 connect/connect_with_cancel 已删除并统一迁移内部消费者，refresh 复用原请求快照。新请求跨 disconnect_all 会话重启后仍拒绝旧 token。
- Uni 的 supervise_local_work 与 presence 保留分离；checked helper 不补启动已取消／到期且尚未 poll 的本地读，已开始读结算后复合保留主因与真实失败。共同拨号取消／到期只影响自己，最后等待者才 join；flight 结果不再以 Stopped 覆盖真实 backend failure。
- 该接口改造不等于完整 Blob owner 已接入。manager.accept 仍在 await 内取准入，未知入站与识别后额度尚未分离，方向名额及 closing 资源需后续实现；不能直接把 supervise flag 配上就宣称完成生命周期。

- Uni 现有可配置 InboundAdmissionPolicy，未知与识别后额度在真实身份解析／成员读取之间交接；受管 accept 在入口申请未知额度并冻结 token/fence，超额立即返回，不 spawn 额外拒绝任务。准入本地工作由 JoinSet 持有，等待方 drop 后仍完成；到期先关闭原连接，再等待真实读并保留附带错误。
- 方向名额必须同时被 record（覆盖本地工作）和 native closed 监督持有。ConnectionLifetime 保持唯一 PeerState 及 semaphore，不缓存原生连接；建立中、取消后迟到连接和关闭未完成均占名额。关闭 JoinSet 须收集旧拨号晚登记的新结束任务，watch 必须持原 session token。
- Native Blob profile 已取消内部 staggered retry，每轮只有一次 Endpoint.connect；不能只数 Uni backend.connect 调用而漏算底层自重试。Blob grant 的 can_retain 读取当前权威，其他 profile 不因此开放保留。
- 尚未实现：生产 Blob 提供端 4／2 即时拒绝、准入和首请求头共用 60 秒（不能在 SDK provide 重建期限）、独立完整 owner／输出和引用保护、非阻塞进度与 5 秒未完成、稳定重试／绑定和最终矩阵。生产旧 Downloader、warmup/provider/cancel 路径仍存在，资源策略的真实 Uni 探针通过也不能冒充生产接线已完成。

- 生产提供端迁移必须兼容旧客户端先 warmup 再由 Downloader 自拨的两条 native 连接。每方向一个“已授权连接”的限制不能在空预热握手后立即占满：Blob policy 在首 QUIC 流出现后才识别并做成员准入，空预热仍属全局 16 未绑定候选；不读取自定义业务头、不发送数据，首流继续使用原 SDK parser。准入和已缓存首流共用入口起 60 秒。此为旧 SDK 兼容的必要业务顺序，不放宽已授权名额。

- 生产提供端已经换为 BlobTransport/IrohBlobHandler；Node.install_blobs 通过资源 factory 由 Engine 组装 owner，SDK provider handler 不再注册。SDK stream port 与独立 store lifecycle port 复用同一 FsStore，关闭 source 不再仅依赖 SDK 的日志式 shutdown。当前旧 IrohBlobTransferAdapter 仍负责下载／warmup，下一步必须迁走并删除，不能把本步当作完整第七片。
- 服务连接监督持原 Uni 租约到父任务结果交接结束，每端每方向名额同时约束监督任务。缓存容量 16 只控制保留集合，满时淘汰空闲连接，不作为新对端服务总数门槛；未知候选 16 和服务流 4／2 仍各自独立。SDK 兄弟结果在同一个已 pin 的 accept future 旁收集，不能每次 select 都重建并丢弃准入读。
- SessionProtocolDispatcher 会在会话取消时直接丢弃 handler future；必须把 SessionLocalWork 附着在同一个 IrohPeerConnection 上，由真实准入／流／SDK 工作引用延长寿命，不能只在 handler 栈上持 guard。真实 Node quiesce 门控用例已证明等待方丢弃后仍保持保护。
- provider-direct/direct.json 和 provider-relay/relay.json 已核对 outcome=passed、exact_bytes_verified=true、direction_count=2、cleaned/plaintext_clean=true；Relay 另为 direct_paths_blocked=true。应用库 1154／1、P2P 库及两个集成 322／3 通过／跳过，Root全workspace旧profile诊断失败尚未重跑结案。
- 当前 BlobTransport 的真实 SDK A/B 用例通过：32KiB以上实际 UDP 正文到达后取消A，B同连接4MiB精确读取且只获取一次；第三次正文中断后source owner正常关闭。真实 Noq 停止与 SDK 导出结算在该路径未产生“已关闭误报清理失败”，不额外放宽停止错误。
- 下一步接下载时需核对：当前 manager.refresh 借用旧 lease，而方向1的名额由该 record 生命周期持有；不能带着旧 lease 自身的名额去重获同方向连接。还须在外部队列及异步 ticket 身份解析之前保持旧会话／撤销围栏，不能拿到名额后取新 epoch；阅读全部真实 fetch 消费者后统一迁移旧端口，不保留转调兼容层。

- refresh 的实际消费者还包括 membership/history_transport，不能只搜索 connectivity 子目录。旧实现借用 lease 在方向限额1时会被自身挡住；新增寿命完成 watch 只携带完成事实，不持有旧 record。下载自动恢复仍须使用同一冻结 ConnectionRequest，且持原下载许可等待实际结算，不重新入队。

- 当前真实获取已经调用 Uni／唯一 SDK port，内存解密与 Raw 流式输出、引用和 tag 在受监督 UC 内，许可持至这些工作结束；正常 Engine Direct／Relay 双向精确通过。旧 Core fetch／Downloader 代码仍在，主要测试引用它们，需要后续整体删除并迁移旧客户端 fixture。
- 源身份异步读取必须位于已取得活跃许可之后；同步票据公钥可作内部调度键，不能派生业务 DeviceId。Scope 冻结当前会话和原撤销围栏，后续 bind／每次准入继续核对。票据地址用不透明本请求提示传给唯一 backend，不能通过 upsert 或第二份缓存偷偷持久化。
- 不能让两个等待者消费同一个 Notify 维护时钟；获取外围唯一时间监督者覆盖身份读取、准备及真实 SDK 工作，恢复开始也显式唤醒重算期限。身份60秒慢读用例已证明到期发停止但不虚放名额。
- 当前仍有完整性缺口需继续：文件会话最终事件／条目提交在 Fetch UC 外，完整 ReceiveWorkPlan 与后台条目取消还需统一负责人；CancellationIncomplete 已阻止旧清理，但后续自动完成 CAS／产物结算不能依赖用户再次调用。稳定绑定尚无这些完整接收动作映射，不假称只有 serde 就满足薄绑定。

- 当前普通下载已从生产调用点移走 SDK Downloader，但 Core trait 与旧实现仍供测试及兼容探针使用，尚须整体删除生产入口并保留明确的旧客户端测试 fixture。不要把生产调用点切换等同于旧代码已删除。
- 完整接收负责人迁移还需校正取消通知的终态位置：当前队列在接受停止意图后就提交旧 Cancelled 帧，实际 TooLate／清理失败应由最终聚合结果决定。后台完整动作的观测上下文也需检查：同步 execute 的 span 不能仅靠 with_current_subscriber 就假定已传播到 detached worker；不为此扩大 Engine 内部查询接口。

- a829f384 后的条目取消用例原在 facade 内；实际只返回 CancellationIncomplete 而没有继续条目投影、staging 与 CAS。现已迁入 transfer/receive/cancellation，完整动作以 entry+attempt 去重、独立于下载许可的有界工作表持有，transport 监督完整协程。Complete 子停止能力不再返回五秒局部结果；五秒仅在完整动作入口等待处计算。
- 取消后端仍需与完整 ReceiveWorkPlan 的实际生命周期统一。当前 materializer 的平面文件循环与目录流程在 clipboard 模块；目录路径有 ensure_attempt_active／committing 核对，平面路径还需按真实调用图核对，不可仅凭子 fetch 已停止就证明整个接收计划所有输出均已静止。下一步必须接完整计划、逐项结果与重试，不继续只补网络小测试。

- 当前 ReceiveWorkPlan 默认入口只是把请求拆成七参数后调用旧 materialize；原实现仍在 clipboard，无法将请求级生命周期知识集中 transfer。已迁至 transfer/receive/materialization 并删除旧入口与 clipboard 二次导出，模型／port／流程分开；本次不把结构迁移算作取消围栏或完整结果聚合完成。

- 仅等待 cancel_inbound_attempt 快照不足以证明计划静止；慢 reserve_target 可在快照后才发起获取。本轮用登记于 attempt 可见之前的 ReceiveAction 贯穿整个已有入站监督流程，计划／子获取继承 token，取消先停止计划，再等真实完整流程。
- 还存在“原计划在取消查表前已经结束”的竞争，内存查无工作不能推出应做 NoEntry：必须在清理前重读账本，已部分取消只能补投影。控制时序回归同时覆盖查表前／后两种终结。
- 正常错误共享不能直接包完整 ApplyInboundError，否则改变原 Display；thiserror transparent 对 anyhow 来源还会跳过其根 IO。共享 lower SourceError 的显式 source 包装保留 root，并只用于已登记计划，无计划的原处理错误保持原对象。完整动作失败继续由取消等待结果传播，不能将 watch 结束当作成功。

- 下一步所需生产重试资料尚未接线：当前 original_blob_indices 只作长度断言，RetryEntryReceive / item_ids 在 crates 下仍未实现。Core 现有 attempt／artifact ports 没有 recipe／ticket 字段；阶段0“从原始V3可重建”是可行性证据，不能当作生产已保留资料。后续须按不新增持久格式的既定约束把普通同进程原始资料保留与稳定项结果一起接入，重启不可重建明确不可用。

- 本轮从实际代码修正：平面循环原失败分支把当前和所有剩余文件都标缺失并break；单项取消也break；无attempt调用还复用同一会话，若继续便会接到已经失败的会话。现所有文件各自按原位置区分，失败／单项取消继续，整计划token仍封剩余项。
- 文件获取原最后项直接向来源条目发Completed，会覆盖前面的失败且早于条目保存；ReceiveBatch只记逐项事实，由原ReceiveAction在持久结果／实际流程结束后发一次终态，进度按整批200ms节流。必要session.complete错误现在保留source并返回失败。
- 后续需核对：旧超时扫描目前只查BlobFacade在途登记，仍可能漏掉完整计划在子UC已结束但会话／条目结算未完的寿命；必须接现有ReceiveWorkRegistry，不能仅靠新测试统计宣布许可／生命周期全覆盖。目录表示获取失败的既有generic错误仍待改为完整source；原资料生产保留和稳定逐项查询仍未实现。

- 当前EntryReceiveProgress只有聚合计数；file_transfer已有receive_item_id、item_role、failure_code和密文metadata，directory_publish_log能在不读root_map的情况下识别目录整体单元。正常blob获取此前不填item_role；本轮按真实表示／文件上下文填已有字段，避免让Infra解析Application任务ID编码。历史缺失类型显式unknown。
- 现有汇总items_total/items_completed是成员计数，目录可能多成员；新增items是可选择的完整业务单元，目录只返回一项，原计数不改语义。完整原始清单／未启动项及同进程重试资料仍需在receive owner保留并合并，不能仅靠现有投影行推断。

- 持久投影的completed只证明成员层完成，不能等价为业务项保存成功。Core现在在attempt非终态时把此类项保持transferring；原成员字节计数不改。完整终态查询仍须合并receive owner真实结果／已落地产物证据，尤其是部分接收和会话发布失败；本轮不标记整个逐项业务查询／重试要求完成。新增绑定仅是现有持久查询入口，不声称已有RetryEntryReceive或取消绑定。

- 下一完整查询／重试切片应在ReceiveProgressQuery合并同一receive owner的完整资料与结果，不能让SQL投影被误当作最终业务项权威；当前只限制非终态成员不能提前完成，部分结果仍需逐项产物／完成事实证明。保留原始V3（Bytes可共享、不复制文件体）、来源／预期身份和实际部分快照／产物，才可把失败项选择重建成新attempt，同时保留成功项。
- 手动重试必须用expected_attempt CAS，并先确认原ReceiveWork与取消协调者实际结束；不能复用正常重投的“另一份partial保留旧占位不替换”分支，否则已重试成功的选择无法保存。原资料应按条目／尝试归receive owner，删除／关闭必须清理；加入敏感原资料前需要明确封口世代，防旧任务在锁定／切Space后重新登记，不能仅无限保留Raw输入。

- 资料保留的内存边界需要按最小必要材料实现：优先保留原始V3BlobRef、manifest、预期身份及成功项的原位置／路径所有权／已验证摘要；重试保存时复用既有已存快照内容，避免为了recipe长期保留整份含inline负载的原始envelope。原blob_indices必须实际参与选项过滤，不能只做长度断言或把过滤后的索引重新从0编号。

- 目录表示阶段此前把失败保存在batch内部后返回新的anyhow字面量，取消也重构Cancelled，造成调用方无法追溯原始来源。现在目录路径在获取失败／取消处记录状态后直接携带原始错误返回，尚无目录输出；非目录仍按原部分结果处理。新真实目录回归同时验证原始context类型和IO来源／取消类型，且成员获取为零、目标目录为空。
- 两套取消绑定只交接现有Engine完整操作，未增加清理步骤或内部状态对象；固定结果用Engine同一wire_enum派生Display，保留五秒未完成。UniFFI工作线程的生命周期命令仍走原独立通道；本轮不改worker排队模型。

- 当前生产assembly此前独立构造ReceiveWorkRegistry，readiness关闭只改AtomicBool。接收在异步身份／权威读取后才登记或开始attempt，会跨门禁关闭／重开继续。readiness现持有同一registry运行世代，关闭取消旧世代，旧action注册失败，权威读取后再次核对；恢复只可打开原世代。已将就绪状态统一到同一锁内事实，不保留与世代分离的AtomicBool。
- 旧超时扫描此前只看BlobFacade的子获取／取消；完整ReceiveAction在慢准备／保存阶段可能被抢写或清理。现在扫描同时查共享registry中尚未交接真实完成的条目；关闭不移除登记。来源保留尚未实现，本轮只完成其不可跨关闭重新登记的前置边界。

- 仅用owns_entry初筛并不足以证明清理安全，检查与清理间仍可开始新动作；增加按条目RAII清理保护，并让同锁内的新接收登记返回typed CleanupInProgress。保护不占下载许可、不引入队列，跨异步操作存续；正常完成、失败或退出释放，其他条目不受影响。当前仅保护完整在途动作和清理互斥；对无运行owner的异常／历史行仍须由同一receive负责人核验结算，不把内存查无工作当作可删除任意产物的证明。后续还要审查旧attempt行与新权威attempt／实际产物的对应，避免旧行删除更新尝试已使用的路径。

- 来源目录现归ReceiveWorkRegistry，仅在attempt接受后保留源设备／预期身份／V3 refs／原始位置与表示描述，未保存时共享原V3 Bytes，部分保存后释放整包正文，全部成功移除来源。query用catalog原清单与实际persisted事实合并：同进程NoEntry／保存失败不能把download completed显示为交付完成；缺少runtime资料的持久partial投影仍需进一步证据重建，不能据此宣布完整查询完成。
- 完成后receipt只持不含源错误／actor引用的ReceiveFacts和原始资料；活跃Batch用Weak关联。首版在finish中drain routes导致最后actor引用过早释放，两个通知测试真实超时（6通过2超时），改为保留原交接寿命、receipt只持弱引用及最终快照后8项全部通过。不能仅为收窄内存而悄悄丢弃已有正常终态交接。
- 删除最终由既有DeleteClipboardEntryUseCase完整负责，在获取entry／event／文件引用之前取得receive删除范围，封住旧birth弱围栏并对相关Progress同时发cancel，等实际结果后再读最终event及引用，避免使用旧快照漏清理新受管输出。clear／cleanup／reconcile／retention复用同一Arc<DeleteUC>，移除各自重建的管线。原底层ReceiveEntryDeletion装饰器已删除，ApplicationDeps不再被改写。真实目录测试同时证明最新event引用被读、新受管文件删除、用户文件保留及停止来源不被吞掉；清空历史未枚举pending、无记录遗留ledger和来源容量／寿命仍需最终核验。
- 下一步应实际消费source的expected_hash／from_device／表示描述和planned.blob_index，实现选择重试，不能靠未使用字段或stage0探针声称生产重试完成；保存后重建基底需用已存密文快照及原表示描述，成功项路径／所有权／验证摘要从真实保存事实恢复。NoEntry原包不能无界持有，须在不破坏普通同进程原始重试的前提下解决生命周期和容量，而非为通过资源检查任意缩减可用性。

- 选择不能过滤blob数组后重新编号：file ordinal用于既有member身份，blob原始位置用于source选项，两者遇到representation混合时不同。现在保留完整数组，从original_blob_indices分别生成rep／file位置，selection与ReceivedItem均绑定原始位置；混合表示＋两个同名文件测试证明只选后一个仍使用原member:1、原blob:2。
- 旧成功路径不能作为新attempt的Preparing产物，否则取消会清理已经交付的受管文件。选择物化复用旧输出只加入completed事实，不加入receive_artifacts；used_names先登记旧文件名，避免新写覆盖成功文件。复用前检查实际大小／摘要／表示身份、路径归属及symlink，64KiB流式读取，不新增下载池或持久字段。
- 来源目录改为逐项landed证明；从真实MaterializeResult.completed在实际保存后记录。query不再用一个persisted布尔值把所有下载完成项都升级为交付成功。当前尚无生产选择重试调用者，with_selection及source部分字段的未使用警告仍是接线未完成的证据。
- 下一步应实际实现RetryEntryReceive负责人：从runtime资料和既有密文快照构建计划，验证当前成员／预期身份／旧工作和取消协调者已结算，使用expected_attempt CAS后监督实际处理；保留旧成功并允许替换partial。新attempt的复用成功项还需要真实业务投影，不能只靠内存集合，尤其不能在complete释放来源后漏掉原成功项。无runtime部分成功证明及原NoEntry遗留ledger删除仍未完成。

- 下一轮重试接线要点：SnapshotReconstructor保留rep.id但把paste rep放首位，且只恢复selection候选；不能直接把这个数组交给原representation_index。需按原描述恢复顺序，必要时从现有密文表示完整读取，而不是因重建器忽略可恢复材料就缩减为Unavailable。重建前后核对既有get_snapshot_hash与原expected_hash，绝不换用来源端当前内容。
- Retry必须同时确认ReceiveWorkRegistry旧动作及CancelEntryReceiveUseCase.jobs的settled完成，单看attempt终态不足以证明清理完成；可用Application内部完整业务前置能力，不向Engine加阶段查询。新attempt复用成功项需真正投影，不能在complete移除runtime来源后只剩选中项；未选失败／取消状态和原因也要继承，不能把所有缺失都当作新失败。
- 公开操作的重复点击应立即同一在途结果或AlreadyReceiving，不能先等待现有EntryIdentityCoordinator.lock到整次下载结束再返回Superseded；CAS之前需封住同条目的并发集合，受监督的动作不得因等待方退出而遗失。正常redelivery的partial保旧占位分支不能用于手动重试替换，成功项的真实ReceivedItem证明是本轮已提供的输入。


## 2026-10-22 实际手动重试接线

- 新完整入口：transfer/receive/retry/owner.rs负责原始选择、当前成员、源快照与已存输出预检、防重复和实际后台交接；acceptance.rs在共享内容身份锁内检查旧attempt／身份／旧取消实际收尾、登记完整动作、begin_redelivery CAS。GateClosed／EntryDeleted返回Unavailable，CleanupInProgress返回CancellationIncomplete；不写新attempt。
- apply_inbound复用原有唯一物化／原子capture／replace／回滚／停止监督；execute_owned接收内部PreparedReceiveRetry，源目录和继承保存证明登记后才发送Accepted；手动模式store-only，部分新增成果可替换旧部分占位。Engine仅公开一个RetryEntryReceive完整动作，两绑定只映射。
- RetrySnapshotReader按最初表示ID及位置从现有密文存储读取正文，检查格式／MIME，不使用粘贴优先重建器。丢失blob表示可保留空位置，丢失原非blob表示不能伪造；Lost／Orphaned携原始类型返回Unavailable。NoEntry原V3仍共享Bytes。错误Debug／Display只固定文字，Error::source保留原链。
- 未选失败／取消结果继承；旧成功输出复用不登记为新清理产物。全部补齐时source=None，只保留纯结果投影，避免本次没有下载行的继承成功项丢失；这个投影仍只在同进程存在，重启后的持久投影仍待实现。
- 本轮最终App1206＋1跳过，Engine51，两绑定47＋19；workspace全目标1m30、metadata JSON、fmt/style/repository/privacy1371、文档新增目标4与diff检查通过。最终文件冻结期间未修改Rust／文档／inventory，未清缓存、未建独立target。验证日志在同一planning目录，metadata JSON重命名为忽略的metadata.log，未提交机器路径清单。
- 实际GetReceiveArtifactsPort尚不存在，现有Core只有receive_artifact_log.rs的ListUnsettledReceiveArtifactsPort；不能按旧摘要把所需读取能力假当已实现。entry_receive_attempt_repo.rs的begin_redelivery保留旧传输／产物行，仅以Failed／Cancelled＋旧attempt作CAS。新attempt的实际继承项投影可以研究既有FileTransferFacade／ReceiverTransferRegistration和ReceiverTransferHandle，但必须先核对清理不能按新行cached_path误删旧输出。
- 仍需真实SQLite／Engine链路，而不是继续只用测试attempt／capture端口证明完整业务。正常redelivery保留旧部分成果时会覆盖原receipt且未继承旧证明，需要完整来源衔接；新retry预检还未登记active，局部校验与完整结算许可／期限、关闭／删除／撤权全覆盖仍未完成。NoEntry来源寿命、无runtime来源可重建性、目录原始整单元、旧fetch／Downloader／warmup与最终矩阵继续保留。
- 当前origin/main额外有1c0660d9（#224，暂停释放SQLite连接与network log lease），本分支仍基于开工a461b894；生命周期整合／最终交付前须同步并重验，不在未提交大改动期间重写分支。


## 2026-10-22 重试账本原子接受

- 新专属BeginReceiveRetryPort把旧attempt CAS、完整项初态及目录空Staging头放进一个事务。共享replace_terminal_attempt保持普通redelivery原语义，不复制两套CAS；无新增schema／密文版本。
- 红测先证明旧CAS重开后0项（预期3），写全行后再证明旧group_receive_items把实际继承完成降为Transferring。既有binding_state增加entry_bound保存已交付事实，所有初态cached_path=None；不写事件／产物清理记录，目录根映射仍使用既有加密能力。真实唯一键故障证明CAS与前面已插入行全部回滚。
- 原选择物化遇前项父取消直接break，会漏掉后面的实际旧成功。真实文件红测保留，修复为选择模式逐项处理取消并继续继承；普通模式和目录整体取消语义保持。
- 当前真实storage259通过／3跳过，App1207通过／1跳过、Core335通过。额外保存阻塞时查询证明旧完成、新下载未交付和旧未选取消同时可见。新下载在capture失败／重启后的持久交付证明仍未完成，不把entry_bound继承事实冒充所有接收保存已解决。


## 2026-10-22 新成果实际保存证明

- 真实SQLite NoEntry红测复现裸下载Completed在终态仍错误报告交付；不放宽断言，保留delivery-proof-red.log。Core查询现在只认可明确entry_bound或完整Completed attempt，未提交的下载在终态显示中断／取消。
- 既有InboundCaptureCommitContext与InboundReceiveSettlement的Complete／Partial携带内部delivered_item_ids；实际成功物化构造候选，capture与密文装饰器原样交给唯一原子提交。NoEntry没有该字段。SQL同事务核验entry／attempt／Completed行并写既有binding_state标记，缺失行回滚记录和标记，Backend source保留具体不变量类型。
- 新receive_item_ids集中原数组映射，来源／retry seed／capture共用；目录仍整单元，不声明单成员交付。App真实重试测试同时验证只把旧成果和新增成果提交，不包含未选取消项。
- 定向真实提交／加密／目录契约共10通过；完整App1207通过／1跳过、Core335通过、Storage260通过／3跳过。首次check的私有identity路径错误改用有限crate内导出；新增集成测试子模块路径需显式path属性，修正后通过。失败日志保留。
- 历史partial行可能只有裸下载Completed；其原始行未改写或删除，但在无保存证明时保守展示中断。完整历史成果恢复必须基于已有密文快照／产物资料继续实现，不能靠这次新写标记宣称已兼容全部历史结果。


## 2026-10-22 生产下载旁路删除

- 普通redelivery在begin后retain_source会把旧receipt及landed覆盖；后续partial-vs-partial分支又保留旧entry、回滚新输出。不能按fresh数组直接移植旧证明：Representation保存证明核验原ID和正文，新包中的ID／来源可能改变。需要原资料完整衔接；当前未实现，不以任意拒绝全部重投缩减能力。
- 确认旧BlobTransferPort.fetch／fetch_to_path／shutdown_inflight_fetch仅剩测试消费者，但真实IrohBlobTransferAdapter仍完整保留Downloader缓存、endpoint预热、4次重试和shutdown_endpoint。已删除全部生产实现和trait方法，Engine测试发布装饰器也不转发旧方法。
- 本地导出回归移至实际BlobReadStorePort，返回值为字节数，因此迁移初次check发现4处旧Digest返回断言；改为实际长度、独立ticket摘要和精确字节验证，未删断言。真实Uni远端读取＋local／tag／TryReference19通过，实际生产provider的同连接A取消B不受影响及旧客户端空连接2通过。
- FakeLocalStore不再调用旧网络fetch，复用独立本地port；完整App1207通过／1跳过、Core335通过。共享SDK租约fixture拆出纯租约模块，首次整组check漏引ConnectivityError，修复后重跑；失败日志保留。
- SDK Downloader仅在明确旧角色fixture保留；阶段0的1GiB基线直接固定SDK，不通过已退休生产adapter，保留相同长寿命pool到原资源采样完成。它仍只代表历史基线，最终生产资源矩阵尚未完成。
- 新repository五个反例及当前源码均通过：旧能力、SDK pool、peer-wide取消、原生provider与bridge内部流编排不可回流。完整P2P／向量／故障探针与1GiB历史基线继续运行。


## 扩大回归揭示SDK与长期actor的span所有权

- 全库初轮320通过／2失败／3跳过：真实固定SDK meta.rs:708的info_span(parent: &cmd.parent_span())在全局subscriber创建子span，却拿到请求局部subscriber的ID。独立运行通过，但先运行accepts_single_inbound_frame_from_known_peer安装全局过滤subscriber后，publish_path_logs_do_not_include_source_path在0.3秒稳定失败，RPC sender关闭。全局＋请求subscriber是load-bearing条件，原生连接诊断关闭也在actor析构时向全局registry释放局部parent。
- 单变量把SDK add_path future的创建／驱动移出请求parent后，原始3测试从2失败降为仅node关闭失败；领域完成日志仍在外层，新增正向捕获断言，未过滤第三方记录去掩盖问题，也未改固定iroh-blobs源。之后把相同私有store_scope策略覆盖生产SDK能力，maker接缝接受IntoFuture，防止future创建时就捕获局部parent。
- mDNS外层actor需要显式parent None；acto发现服务的同步spawn也不能保存启动请求parent。只改外层时Rust测试状态ok，但stderr仍有guardian析构panic，不作green。恢复两层隔离后原最小组无panic。
- 新startup_trace_tests使用真实Node完整bind＋session激活，并在发现仍运行时检查根span已关闭；正确async instrument red在临时移除discoverer隔离后0通过1失败（0.01秒），parent树明确为startup→Node.bind→guardian→sender/receivers，恢复后green。第一版enter守卫跨await会给其它任务错误parent，已经纠正，该版失败不作修复证据。Endpoint原生构造与bind future创建／驱动统一隔离；诊断recorder在外层捕获并继续输出实际网络结果。
- 当前正确4测试组合0.26秒全通过且无panic；整库／SDK资源验证仍在运行。SDK协议、票据、blob字节、失败链与停止等待算法未改；没有新增Engine业务步骤接口。

- 最后整库322通过／1失败／3跳过，SDK／向量／1GiB独立二进制各通过；失败堆栈已从SDK事务worker转为测试Endpoint关闭，socket.rs:1128／1208显式使用启动subscriber的span。扩大78测试复现77通过／1失败，完整backtrace保留trace-dispatch-repro-expanded.log。隐私测试显式跨两个registry后单跑0通过1失败（0.08秒，endpoint-subscriber-red.log），不再依赖全局前置顺序。fixture SDK启动按生产边界隔离后最小3通过，日志正向捕获与路径隐私断言保留。最终整库323通过／3跳过（167.73秒），SDK结算1通过（0.43秒）、协议向量1通过，remove-legacy-p2p-final.log无panic。


## 2026-10-22 未保存投递的来源提交边界

- 上轮2576a4cf为progress：已提交退休路径删除、跨subscriber修复及完整P2P／Engine／根门禁。本轮工作树核对仅用户四个规划目录未跟踪。
- 真正来源覆盖红测在完整普通接收之后断言原source：新包来自different-sender、两项和新ticket／新表示ID；它被partial-kept分支丢弃，但receipt原三项和已保存项被覆盖，实际left为新发送方。redelivery-recipe-red-fixed.log为0通过1失败（0.00秒）。首轮测试编译因兄弟usecase模块私有字段不可直接替换；改为测试完整receiver构造显式传共享readiness与各port，不扩大生产可见性。
- receive/work的候选动作持有至多一份实际保存旧receipt，只有persisted确认提交后丢弃；否则finish按原source清单重生成当前attempt项ID并重绑定既有facts。旧表示ID、实际输出及原删除所有权均不变，不能继承新包数组。无旧保存成果时保留新来源。finish幂等，不能将已恢复facts再覆写为丢弃下载的facts。
- 普通begin在register前检查旧完整action仍否持有条目，拒绝原因保留具体ReceiveRegistrationError::CleanupInProgress；旧终态但还在收尾不能并发接收。
- 首轮修复完整普通接收＋随后的手动选择和旧动作封口2通过（0.01秒）；扩展manual_retry／work／batch28通过（0.06秒），包含两次未提交候选、保存成功后来源替换／完整释放、关闭重开与删除后迟到finish不复活。正在补capture失败真实分支并扩大Application整库，不提前记通过。


## 2026-10-22 当前候选真实资料矩阵与实际协议证据

- 5a9ae6f7为progress：保存来源切片已提交。核对spec §8／9，不能把此前SDK基线及局部测试当最终候选矩阵；开始用当前宿主、开工main a461和rc.15 f6f的原冻结库跑既有P00资料流程。
- 首轮当前候选十组文件矩阵全部通过：两旧端×Direct／强制Relay×升级第0／1侧，共8组，另新新Direct／Relay两组。旧库二进制与阶段0摘要逐个核对SHA；每组旧旧重开、旧文件升级后读回、新旧双向精确文件、原身份／传输身份／成员／历史、副本和清理／明文检查均真实执行。工件在target/blob-connectivity-evidence/candidate-5a9ae6f7-2026-10-22。该轮实际blob ALPN在物理连接记录中仍为other，因此未当作spec完整协议证据。
- 收紧P00既有harness，复用同一真实协商日志读取能力，要求候选实际协商blob。保持旧二进制运行同一流程后red（blob-protocol-matrix-red.log）：旧文件和双向文件仍通过，但未观察到实际协议。封闭编码红测0通过1失败、实际native握手红测0通过1失败（0.03秒），分别保留contract-red／handshake-red日志。
- 现有ObservedConnections读取实际conn.alpn()，增加固定SDK ALPN→NegotiatedProtocol::Blob映射；wire_enum同时唯一生成封闭编码。未知仍Other，任意ALPN／未知字段仍拒绝。无新业务trace／Engine步骤查询、源地址或原始ALPN，不更改SDK／协议／持久格式。原本地uc.connectivity仍不发往远程Collector。
- 固定协议解码2通过、真实native握手1通过（0.04秒）、实际本地导出1通过、Collector隐私5通过；已重建当前宿主。下一步带实际协商断言重跑十组，尚未预先记为通过。

- 带实际blob协商断言的十组已全部通过，summary逐格核对二进制SHA／旧库修订、源码树／工作差异一致、原资料保持、精确双向文件、真实blob握手、两栈UDP强制Relay、明文和清理。完整P2P323通过／3跳过；普通Cargo观测回归180通过／1失败／1跳过是未改动的两测试共享全局subscriber，按项目nextest隔离后181通过／1跳过，不修改产品语义或断言。根门禁全部通过；完整资源／数量期限／生命周期／最终head CI仍未证明。


## 2026-10-22 真实 Engine 部分重试与取消结算

- 初始fixture错误关闭发送端自动同步，SendFiles被现有1253门禁拒绝；修正设置并等待真实peer connected。后续真实接收成功，重试却仍Failed；诊断日志明确 completed_bytes=24、items_completed=3，但旧StorageUnavailable事实被恢复。
- SQLite最小回归实证 UNIQUE constraint failed: clipboard_snapshot_representation.id（replace-stable-representation-red.log）。重试保留原表示ID，replace插入顺序冲突。原事务内先清旧选择／缩略图／表示后插新表示，失败全部回滚；四项仓储测试通过。真实三文件基本重试首次通过3.36秒。
- 扩展本地导出完成权屏障后，重复点击与旧取消隔离通过；提交后owner尾部未退出时仍AlreadyReceiving合法，要求最终退出后旧attempt明确Superseded。撤销原发送方的真实Engine回归返回AccessDenied，不访问store／改变attempt。
- 取消等待丢弃后，重复取消实际到期为CancellationIncomplete；重试误报AlreadyReceiving，原日志real-engine-retry-lifecycle-second.log为2通过1失败。修正retry owner防重入同时检查同attempt清理屏障，Failing也不允许新重试。
- 下一轮又实证取消终态后新导出文件仍存在（real-engine-retry-lifecycle-fixed.log为2通过1失败）。物化只删除用户预留文件、忽略受管缓存；两者现在都清理，成功复用项从不进入本次清理。清理失败保留原fetch完整source chain及独立cleanup IO，不丢产物日志、不结算终态。
- Application实际文件系统目录拒绝remove_file的两分支回归，验证原StorageFull／Cancelled、IsADirectory清理原因、旧成功文件与Cancelling／Failing非终态。manual_retry相关15通过（0.05秒）。最终真实Engine3通过（16.09秒），含实际写盘后暂停、调用者丢弃取消等待、重复取消5秒到期、拒绝重叠、输出清理、成功保留和撤权拒绝。
- 仍需扩大三crate整库及根门禁；强杀／历史来源恢复、其余生命周期、预算、1GiB和最终CI／平台仍未完成，不以本次三项覆盖替代完整spec。

- 所有权审查新增实际碰撞红测，原代码确实把新目标指向已保存同名文件（managed-output-ownership-red.log）。受管输出改用原子create_new预留，遇到旧文件使用独立随机名称，不覆盖或清理旧文件。成功／StorageFull均保留旧内容，失败只移除本次已预留输出；接收流程105项通过（2.88秒）。
- 首轮三crate nextest在1746通过／1失败／9跳过处fail-fast，25项未运行。真实旧Engine测试engine_start_builds_a_resumable_real_session要求第二次删除NotFound，却得到HistoryEntryDeleted；不是本次完整通过，也未当作外部阻塞。
- 删除owner仍先封口等待，ReceiveCleanup原子保留是否存在接收的事实；无保存条目且无接收时返回Core typed NotFound，facade按具体类型映射，不依赖新增字符串判断。扩展原删除回归核对第二次Missing，未保存接收封口与旧动作停止不变。
- 该修改后Application整库1214通过／1跳过（25.83秒）。准备冻结代码、更新1355站点inventory并用no-fail-fast完整跑三crate，再执行根门禁。未完成的NoEntry持久清理／删除监督、更多生命周期、预算、历史来源、1GiB和最终CI仍保留。
