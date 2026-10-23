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

## 2026-10-22 NoEntry删除与监督

- 核对HEAD09aebb9c，工作树仅四个用户原未跟踪规划目录。上一轮为progress，未出现外部阻塞。
- DeleteUC的NoEntry分支仍调用仅删除clipboard_entry的旧port，实际不清理attempt／file_transfer／产物日志；重启后只剩持久attempt且无内存receipt时，还会直接返回NotFound。完整原子port则强制调用方提供event_id，无法表达NoEntry；需要收紧唯一完整能力，不能伪造EventId或另外保留NoEntry删除实现。
- 内部DeleteUC只有facade和自身测试两个构造点，可以去掉旧可选原子port／旧逐步删除fallback，使用一套显式完整依赖。Core内部port及所有消费方统一迁移，无新schema或Engine步骤接口。
- Engine已有operation监督及abandoned-operation/shutdown真实回归，不能直接假设删除需要第二套spawn／任务表；先核对实际稳定删除入口的完整责任，再决定必要修复。

- NoEntry删除红测no-entry-delete-red.log明确调用旧DeleteClipboardEntryPort，任务panic而未走原子清理。完整port改为仅接受EntryId并返回实际删除bool，旧event参数及DeleteUC的可选原子port／分步fallback全部删除。Facade与所有内部测试同步迁移，无新Engine接口或schema。持久事实决定Missing，不再保留ReceiveCleanup.had_receive。
- NoEntry真实SQLite六项初轮通过；扩展目录未发布日志场景得到no-entry-publish-delete-red.log红测，原实现错误返回true并丢弃发布恢复记录。Core新增完整删除准入规则，Infra在事务内读取事实；未发布目录逐attempt匹配Settled+RolledBack证明，拒绝错attempt、Pending及未完成phase。
- 读取表示失败原先unwrap_or_default，现保留具体source并不删除记录。Application四项定向测试通过（0.00秒）；真实Engine在实际导出完成权屏障内丢弃删除等待后仍停止重试，移除条目／所有接收状态／受管文件，二次NotFound通过（3.53秒）。既有Engine operation监督真实有效，不另建Application任务表。
- 编译首轮新SQLitecount缺QueryDsl（5处E0599），改用原测试已有count_star；引入的EventId未使用警告已清理。六项SQLite／Application3项初步绿灯与两个真实红测均保留，不把旧快照当最终全通过。准备冻结代码、扩大Core／App／Storage／Engine及实际profile commit集成验证和根门禁。
- 额外源码证据：DecryptingClipboardRepresentationRepository列表接口仍会在inline解密失败时返回原密文字节；该历史降级没有由本次get_representations错误回归证明安全，后续应沿已存在的认证文件引用能力核对删除／清理失败边界。本轮不冒称已修复该问题。完整预算、clear／更多生命周期、历史来源、1GiB与最终CI仍保留。

## 2026-10-22 清空覆盖未保存接收

- HEAD cb6c0202，原用户四个规划目录保留；上一轮progress。ClearHistory只分页枚举clipboard_entry，空历史即返回，遗漏未保存／失败NoEntry与孤儿接收状态；并且没有整次动作的登记屏障。
- ReceiveWorkRegistry现有born只跟踪Weak<()>，无法停止并等待尚未登记的准备；本轮将出生身份与独立取消／完成关联，保持原会话世代权威，避免借清空打开外部关闭会话。新增持久目标列表能力归history消费者，Core不加流程接口；完整装配只有一个ClipboardEntryPorts字面量与两个解构位置，可显式迁移。

- 首个真实clear红测因首项导出暂停导致原发送ACK尚未完成，report.accepted为0（clear-no-entry-red.log），修正fixture检查accepted+pending=1；实际收件IncomingPending／SDK导出及无历史条目仍由真实入口证明。随后clear-no-entry-red-fixed.log精确复现ClearHistory在无保存条目时提前返回。
- 生存登记改为出生动作独立子取消／完成，不改变外部会话代次。ClearScope先同时停止出生动作，等待未登记准备退出或已登记动作明确完成；注册完成丢失不是成功，多失败保存全部typed来源，失败后封口。专属目标集SQL UNION只读标识，合并历史／attempt／条目关联的file_transfer／产物／目录日志；不新增schema。原逐步caller编排及错误字符串化移除。
- 真实Engine基本clear8.38秒通过；扩大五项Engine32.69秒通过，含清空后接收新内容。Registry相关17通过（0.05秒）、实际初始readiness准备停止1通过（0.02秒）、真实SQLite目标集1通过（0.07秒）。
- 审查发现readiness.is_ready临时false会启动reconciliation与清空并行；clear-recovery-red.log确证恢复在清空内立即运行。改用共享维护许可，清空先封口／停止再等已有恢复；恢复重新检查ready，不与已接受clear并行。通知也由Registry唯一持有，clear结束能唤醒readiness等待。双向竞争及receive相关58通过（0.11秒），clear相关15通过（0.08秒），待最终整库／根门禁。
- 尚未绑定entry的provisional及更早输入／出站准备不由本轮出生表证明覆盖，留在全生命周期与准备预算验收中；既有认证资料读取／删除缓存失败边界仍未修复。完整目标不缩减，不把已关联条目清空当全spec完成。

- 认证读取红测authenticated-list-red.log确认旧列表在两项解密失败后返回原密文；真实Engine authenticated-delete-red.log在破坏实际保存的V3文件引用密文后仍返回history_entry_deleted（4.251秒）。这会丢弃文件清理及重试资料。修复批量接口为全认证或typed失败，保留全部密码来源并保持单条数量诊断；窄port转换仍应追溯BlobCipherError。真实Engine验收继续覆盖Delete／Clear失败保留状态和文件、恢复原密文后重试清空。

- delete-cache-ownership-red.log用实际文件系统确认cache/../user的原始历史路径被按前缀当作缓存，用户文件确实已删除，读回为NotFound。路径预验证现在完整运行于原子删除前，拒绝ParentTraversal与任意缓存内符号链接，解析失败保留source；缺失目标不误报IO失败，根使用宿主配置解析后的实际目录。正常文件与无归属引用混合时全部拒绝，保护先前正常缓存文件和所有记录。六项删除回归初步通过（0.030秒）；风格门禁要求unit错误标签紧邻map_err，格式化后位置已修复。
- 当前预验证仍通过路径操作，不把它宣称为对抗本地并发替换的文件描述符沙箱。SQL后的remove_file失败仍会丢失引用恢复依据；后续须在不绕过Core未结算拒绝、目录所有权及单一删除负责人的前提下形成完整结算。不得简单把文件删除移到SQL前。

- 审查发现合法的非本机file://host引用会因to_file_path的unit被新拒绝误伤；首轮扩大验证仅编译中已主动SIGINT（delete-cache-ownership-final-nextest.log，终态130，并确认编译子进程退出），未计通过。delete-cache-foreign-uri-red.log实际复现此回归（0.032秒）。改为对没有本机路径的合法file URI保留外部所有权并跳过；UTF-8及URL解析失败仍保留具体source。正常完整删除回归同时加入已缺失缓存文件与外部file URI，另核对原始解析source。

- 下一步预算核验定位：transfer/blob/transport/runtime/download.rs的DownloadedBlob持有DownloadPermit，fetch_blob.rs的execute_inner／execute_path_inner在返回后释放该局部对象；上层materialization/workflow.rs仍负责产物清理和后续保存。当前仅为静态调用／所有权证据，不能据此假报完整预算通过或已经实证超额；应先以受控保存／清理屏障验证，并检查目标预留是否发生在同一admission前。更早provisional由transfer/file/facade.rs及clipboard/inbound/runtime.rs进入，不能用ApplyInbound内部出生动作表单独证明其已封口。

- download-local-settlement-red.log从完整Facade获取进入：两项卡在最终本地进度发布时，第三个同端请求已开始，精确复现许可在Facade本地结算前丢弃（0.035秒）。把同一DownloadPermit交给不透明DownloadSettlement共享持有；底层工作、Facade及接收batch本项结算共同延长同一许可，不再次排队。batch在本项item_result后释放，finish清掉余留项；表示项先赋值完成再交接结果，失败清理／日志交接期间仍保留原许可。
- 首轮基本屏障通过（0.191秒），扩展全局4／同端2、7／9项顺序批次均通过；失败分支quota检查已通过，但shutdown错误预期不正确，expanded.log保留1通过／1失败。既有owner会继续报告Store失败，断言改为核对Store及原StorageFull source，不修改生产关闭策略。expanded-fixed.log两项通过，继续扩大相关流程及真实Engine回归。新增私有字段可见性警告已收窄至transfer范围。
- 边界仍需继续证明：Facade.begin_lifecycle及初始本地进度位于fetch排队／计时之前；目录成员各自交接后仍有目录校验／发布与最终保存；现有inflight completion仍指向底层worker，不能把本轮许可持有等同为完整取消／准备生命周期。后续必须从实际preflight／发布／cleanup屏障进入，保留原4／2、64／16、首批60秒及整体逻辑项语义，不添加第二池或父批次许可，不用本局部绿灯缩减spec。

- download-local-protection-red.log两项均失败（0.033秒）：虽然额度继续持有，原BlobReceived.protection仍在Facade结算前丢弃，实际保护数0而预期2。原生Iroh RetainedContent同时含TempTag及StreamWorkLease，不能只保留计数。完整接收负责人现在通过download_work私有持有者保留该SDK无关port对象；先释放本地工作／内容保护，最后归还同一调度许可，无Any/downcast或新SDK泄露。保护计数覆盖全局／同端、父清理、失败source及最终归零，相关receive／transfer 192通过（2.169秒）。

- 本地结算／保护版本第一次Application／Engine完整nextest 1529通过／6跳过（193.718秒），但它不证明缓存路径的shutdown等待。download-local-shutdown-red.log随后实际证明缓存获取的最终本地进度仍阻塞、额度及保护仍被持有时，shutdown已返回完整成功（0.088秒）。
- 调度器现在在封口后通知并等待原slot全部交回，关闭负责人在原任务收集后继续等本地持有者，再关闭共享存储；不加第二资源池／关闭任务，五秒仍只限制watch等待。完整Facade验证缓存模式五秒ShutdownIncomplete仍保留1项保护、重复Complete不提前结束、释放后同一协调者完成；相关192项通过（5.509秒）。正常失败关闭仍报告Store及原StorageFull，未降低旧错误可见性。

- 下一步静态核验点：Facade.begin_lifecycle／初始progress先于FetchBlobUseCase.execute的prepare；materializer先预留目标再调用fetcher。inflight完成仍取底层DownloadTask而不是完整本地交接。目录所有成员item_result后仍进行发布／最终保存；ReceiveBatch.finish会释放余留local_work，不能直接把动作返回等同为失败产物已经结算。需从实际准备、目录发布及cleanup失败恢复进入，继续证明整个逻辑项／生命周期，不以本轮单项物理工作屏障代替。

- download-preparation-admission-red.log从实际Facade初始进度进入，精确复现第三个同端准备在尚未取得下载许可时已开始。准备现在作为原DownloadTask的owned future，原子入队／取得permit后执行，首批数据计时在同一permit.started_at开始，身份解析及SDK连接沿用剩余期限。准备中取消／到期后仍等已开始的future真实结束，不在排队阶段创建lifecycle或初始进度。
- start_fetch在同一registry临界区先检查旧登记再创建任务，消除重复请求先修改旧session的窗口；准备结果共享实际session与sink，取消读取最新结果。私有BlobPreparationStop区分先接受的取消及deadline，以原业务原因结算准备期session，失败保留source；stop_attempted只用于避免重复结算，成功仍以实际完成结果为准。准备继承既有不透明ObservationContext，不保留原span或暴露内部字段。
- 第一轮Blob相关21通过（5.449秒）；新增虚拟钟的真实DownloadTask准备监督、原StorageFull及关闭来源验证后22通过（5.471秒）。实际Facade扩展全局4／每端2、64／16排队、过载立即拒绝、重复不启动、排队直接取消、活跃准备五秒未完成及调用方退出后仍结算且不迟到读内容；相关transfer／receive 194通过（10.252秒）。最后追加准备30秒＋身份解析30秒的跨阶段剩余期限断言，准备扩大最终检查。
- 边界仍在：materializer在调用Facade前预留目标；目录发布／最终保存、cleanup失败移交及底层completion之后的完整本地inflight取消仍未由本轮证明。不得把Facade准备提前准入当整个逻辑项或完整spec已经完成。

- 准备错误转换不能一概Settlement：原begin_lifecycle的正常LifecycleClosed／冲突／已终态在迁入监督后会污染transport.failure及shutdown。实际监督红测已证实，按原具体FileTransferApplicationError分类且保留完整source后恢复；真实Store及停止结算失败不降级。
- 初次完整快照Engine测试20秒超时，单独8.795秒与最终完整11.683秒通过；未修改nextest配置。最终1533通过／6跳过、根门禁全部通过，不能以一次单独复跑替代完整结果。

- 独立文件预留不应由materializer先做完才fetch：完整Facade调度屏障证明同端第三项会提前创建目标。现在目标及artifact journal为原下载准备的owned future，路径回执只在成功准备后可供export读取。准备失败须以具体标记保留原有整计划中止语义，不能当普通获取部分失败继续。
- 目标future即使超时仍实际完成，可能已创建输出；产物事实必须在日志写入前保留，父流程才可精确清理。没有回执的排队失败不得移除前一成功产物。最终192相关／1536完整通过及根门禁通过，目录发布／最终保存与完整inflight仍未完成。

- Facade原inflight只指向底层worker，原worker返回后最终进度／终态仍可能阻塞，重复请求与取消提前失去所有权。实际红测已证实。现在原任务包含统一最终交接，完成通知及重复排他覆盖完整Facade；取消协调者不再写第二次终态，旧结束项在后续登记统一回收。
- 终态写入失败不能只记录诊断：真实ReceiverTransferHandle三个终态注入StorageFull均要求完整Settlement及原获取／取消来源，失败取消不能报告成功。纯最终存储失败此前分类Unknown，具体红测后改按既有交换类别和IO原因识别。完整条目保存CAS及父流程清理仍另待验证。

- item_result原先释放最后的DownloadSettlement，实际ApplyInbound capture尚未结束时保护计数已0；源码及完整重试屏障均证实。现以原对象交给batch.commit_handoff至ReceiveAction.finish，未选／继承项不覆盖，既有7／9项及实际关闭等待通过。
- 仍有确定的资源边界：下一项local_settlement先释放旧交接，随后解析／排队若拒绝或取消未必拿到新许可；父部分保存可能无持有者。finish错误分支也仍清除未结算清理的guard。二者未修复／未验收，不能把普通最终capture屏障推断为完整预算或恢复证明。

- 调度／提交存在必须明确的组合语义：spec §5.3规定每个逻辑接收项一份许可且业务结算前不归还；§5.6要求独立文件逐项结算，而当前workflow先物化整批、usecase后统一capture，若全部文件持有至统一提交则7／9项会耗尽4／2并互等。临时commit_handoff只覆盖最后一项，不能完整解决中途排队取消或旧成功项尚未提交。用户已明确选择“逐项保存成功文件，保留现有预算”：逐项实际保存后归还原许可，整批仍接收中、最终汇总，已保存项不得被后续失败回滚。该语义已获授权，下一步接入原子保存／交付事实／产物日志移交，不再等待确认。

- 逐项保存的原子基础能力已实现：在同一Receiving attempt下保存记录、标记本项entry_bound、认证并重封原产物日志且仅移除实际同身份／同路径／同归属项。跨记录归属规则在Core，Infra以原密文比较防止准备／提交之间的日志并发覆盖。V1旧日志及生产V3／inline AEAD均沿原保护能力，无schema、状态字符串或格式版本变化；真实SQLite／文件清理／重开、取消、晚段回滚、连续替换、日志损坏与并发更改共16项通过（0.167秒）。完整materializer逐项调用及实时receipt更新尚未接入，不能声称用户选择已端到端实现。
- 该切片两次测试辅助编译失败已定位并修复：InMemorySession／MasterKey须从uc-infra-security导入而非profile私有入口；模块声明必须位于文件内层doc注释之后。per-item-progress-expanded-tests.log现为最终16项通过记录。

- 进度提交进一步收口：Progress携带当前保存快照的delivered_item_ids，可覆盖实际已经保存的内联表示／继承文件；本次必须包含实际移交的文件，且不能把日志中其他仍待清理文件一并标为交付。文件角色及completed事实由同一事务核对，其他交付行沿既有原子检查。新定向17项通过（0.163秒）；扩大Core／Application／Storage／Content库1921通过／4跳过（23.719秒）。首次扩大编译暴露manual_retry测试capture对新context缺少穷举分支，已补同样的交付事实映射，实际扩大验证通过。

后续逐项调用接线已确认的现有结构（未实现）：
- ReceiveWorkRegistry::action原本就返回Arc<ReceiveAction>，execute_owned也一直持Arc；execute_internal目前只借&ReceiveAction，改为&Arc即可把同一完整负责动作交给内部进度保存helper，无须新注册表、监督池或可克隆的Action。
- ReceiveWorkPlan可带crate内可选完整进度保存能力；生产独立文件计划接入，直接materializer测试无调用者继续沿原完整计划入口。helper使用既有InboundCapture及Progress context，保存成功即action.persisted(false, completed)，同步实际receipt来源及交付事实。Engine不介入内部步骤。
- 当前batch.item_result会将最后许可放入commit_handoff；需要文件的“实际保存成功”结果交接原子地记录事实并移除同一个local_work许可，而不是保存完再调用旧item_result重新留handoff。失败／清理尚未结束时不得释放；清理失败的恢复责任仍须另外落实。
- 中间快照必须以完整原blob位置构造：已保存文件和当前文件的本机URI、全部其余缺失项、已成功的表示；不能把尚未下载的声明当真相。可复用finalize_partial和显示元数据重写，但最终完整快照／图像合成仍由原materializer完成。目录继续整体发布，不能接逐成员Progress。
- 每次Progress后只保留尚未交付的receive_artifacts给下一次journal预留和最终清理；否则新写日志会把已交付文件重新登记成可删除产物。最终has_receive_artifacts仍反映确有日志，空pending向量可由终态提交结算。
- 根流程需要区分“本次逐项已经保存”的记录与原本existing partial：最终应replace本次保存条目，不能再create，也不能走普通重复partial的skip分支；原有rapid dedup在materialize之后，须移到首次实际保存之前，不能保存后再当NoEntry回滚。原本已有partial的普通重复接收必须处理原成功映射，不可简单跳过逐项保存来声称覆盖全部路径。
- 最终capture失败只可清理当前未交付集合，保留已保存快照／文件和receipt；后续独立项保存失败应移除其新输出、保留原source且继续其他项。现有manual_retry Capture替身按快照摘要数直接设Completed／Cancelled，接线后Progress必须保持Receiving，不能让替身抢先终结。

- 完整逐项调用首个切片已接线：ReceiveWorkPlan带同owner的内部保存能力，新接收／手动重试的独立文件在实际capture Progress后调用item_saved归还原许可并从下一次产物日志排除；终态改为replace本次保存记录。中间快照合并全部已校验的继承成果，不因某些旧成功尚未遍历而丢失；保存失败只移除本项输出、保留source并继续后项。当前普通已有partial重投仍沿原策略，尚须处理不可直接按新数组映射旧成功的情形，不能把此切片当逐项语义全路径完成。
- 首次manual_retry回归11通过、3失败、2超时：capture替身已先按Progress保持Receiving，首次回归仍因每次capture均阻塞及保存次数期望过时而失败；改为屏障只阻塞一次，并把“逐项保存＋批次终态”两次保存的实际证据纳入断言。新下载在保存前需显式记录materialized业务事实而不释放许可，避免无网络进度替身时被误报Pending。修复后16通过；新增9文件实际Facade／调度保护计数及保存失败后续项继续两项后18通过（5.034秒）。该证明保存端是显式替身，不冒称SQLite或网络。真实Engine既有3文件部分失败及选择重试在新的逐项调用下已通过（4.523秒），新“后项仍Receiving时首项已可从稳定历史入口读取”验收继续。

- 真实Engine扩大2项通过（14.000秒）：第三项仍在真实export屏障时，稳定ReceiveProgress为completed／failed／transferring，首项可经实际历史／解密入口读取；释放后最终保存两个成功并保留正常选择重试。当前RootSource helper随后补充保存前原action取消检查及CAS封口反应；扩大Application相关199项通过（15.698秒），含先CAS后通知的确定性用例。第一次扩大暴露两个测试替身契约问题：AttemptGate已Cancelling时须返回AlreadyCancelling；result_query Capture的Progress必须保持Receiving，且最终保存须支持replace和全保存失败时的缺失快照。已对齐实际port语义，非放松业务断言。
- 下一步必须继续核对完整结算的两处断点：当前Progress发生在fetch_blob_to_path返回之后，原许可／内容保护仍由同一DownloadSettlement持有，但Facade自己的inflight结果已完成。因此逐文件CancelInboundTransfer在保存阻塞时可能提前报告AlreadyCompleted；且该项保存失败仅进入ReceiveBatch事实，底层file_transfer行仍可能保持网络Completed，重启后的存储失败分类尚无完整持久证明。不能把当前Root／Entry取消及预算验收当作单项取消和全部持久失败分类已完成。可考虑把原Owned目标准备同样的完整本地提交回调交给原FetchBlobToPathCommand／DownloadTask，在实际文件结果产生后、现有FetchFinalization会话终态前执行，这样同一inflight、取消token、会话失败及原许可覆盖真实保存；仍由同一ReceiveAction提供保存能力，不把步骤交给Engine。需从实际阻塞保存＋单项取消红测及真实SQLite存储故障证明再定实现，不能另建监督池或破坏原source。

- 最终审查补正保存失败后的再次空日志写入：本项真实输出删除后，原pending日志可保守保留已不存在的项，下一次预留或最终提交更新即可。再次写空日志若失败会覆盖原保存source并提前中断后项，故移除该冗余写入。用明确拒绝空日志的存储替身强化原失败继续用例，验证仍完成后项并保留原StorageUnavailable；该用例首次编辑将journal断言放到相邻取消测试导致E0425，已移回正确作用域。此变更后的最终测试需重跑，不复用前一快照1544的结论。

- 冗余空日志写入修复后的最终验证已完成：强化保存失败／后项继续用例通过（0.056秒）；最终Application／Engine1544通过／6跳过（183.042秒），metadata812包、workspace全目标check（1m30s）、repository、fmt、style、1348站点隐私及diff全部通过。后续须实际检查“已有逐项成功后，后项目标准备致命失败”时原日志的最终结算；当前Root materialize Err仍按[]／has_artifact_journal=false调用NoEntry，可能留保守日志。尚未从实际SQLite证明其影响，不假称永久拒绝重试：replace_terminal_attempt当前并不核对旧artifact日志，必须按实际恢复／删除行为设计并验证。

- 单项保存取消红测已证实：实际ApplyInbound／Facade原许可＋保存端口屏障时，cancel_inbound_transfer立即返回Ok(NotInflight)，此时保护仍1且尚无保存快照（single-save-cancel-red.log，0.035秒）。当前实现为原DownloadSettlement增加无资源所有权的本地结算观察信号，只有受管逐项文件在fetch前启用；Facade登记不再仅依据网络结果完成而回收，原有取消协调者等待网络与实际本地结算，没有第二池或新监督任务类别。Core定义取消／不可遗弃提交的原子竞争，取消先封口拒绝开始保存；已经开始的保存等待实际结果，成功后只返回实际终态。ReceiveAction异常退出把完整source移交信号，观察者不持许可，9文件不会积累额度。
- 首次编译E0364／E0365来自receive/mod.rs重导出范围比local_settlement.rs类型宽（mod入口super::super为crate），已收紧为pub(super)。Core两项及实际阻塞保存等4项通过（5.073秒）；扩展重复取消等待方退出、重复获取Busy／无额外输出、保存失败两次取消仍保留StorageFull、关闭报告保留原失败等，23项通过（5.127秒）。保存端是明确替身，真实许可／文件写入运行，不冒称SQLite单项取消持久化全覆盖。
- 正在验证已终结的文件保存失败是否因之后一次取消观察而新建失败协调任务、反向污染原本健康的传输关闭；该场景不应再启动停止工作。新增确定性回归single-save-cancel-observer-red.log运行中，若确认则直接复用已有完成事实回答。

- 已终结失败的观察红测确认：保存失败已经结算、许可已0之后，一次取消仍新建失败协调任务，随后transport关闭被Settlement反向污染（observer-red，0.071秒）。现改为网络和本地均已结束且没有既有协调者时，直接返回原完成事实；正在收尾或已有协调者的重复取消继续加入原任务，仍保留StorageFull完整source。扩大测试重跑中。

- 观察终态修复后24项通过（5.094秒）；取消实现按完整职责移入facade/cancellation.rs，未保留转调副本。新增实际下载结果交接屏障证明：取消先于本地提交时拒绝该文件保存、删除本项输出，随后仍保存同批其他所选文件且保留旧成功。Core／手动重试25项通过（5.093秒）；再扩大到全部Application blob／receive／ApplyInbound及真实Engine接收重试组，214通过（55.820秒）。最终Core／Application／Engine库验证运行中。

- 最终同一源快照Core／Application／Engine1887通过／6跳过（199.675秒）。metadata812包、workspace全目标check（1m36s）、repository、fmt、Rust style、1348站点隐私及diff通过。手工审查实际取消与资源所有权，无可用code-review skill。下一步持久保存失败分类须用SQLite／重开证明，不能将本地完成信号当持久事实；完整spec仍未完成。


- 保存失败持久结算：原网络Completed不回退，在既有CommitInboundReceivePort增加FileFailed完整能力。物化负责人先清理本项输出，再由同一事务验证原attempt非终态、原AEAD日志密文／身份／归属，将固定失败原因写入既有failure_code并仅移除该产物。原许可仍在batch中；失败事务不成功则返回FileFailureSettlementError，保留保存与结算两个来源，Root不再按NoEntry假称结束。Core负责可接受的attempt阶段、已交付优先及本地结果投影；Infra仅编码既有固定词表／原子CAS。迟到最终保存不得把有本地失败事实的文件重新绑定成功。
- 首次真实SQLite新故障组2通过／1失败：重新打开后items_completed仍为2（预期0），实际保存触发器已失败、两个文件均未交付。修正查询计数和字节汇总，避免按底层Completed把本地失败／取消算成功。原状态词表、schema、V1／V3及metadata版本保持不变；不新增持久recipe或自由文本。完整原子契约最终20项通过（1.71秒），另加密decorator2项通过；包括生产V3重封／重开、保存事务失败、混合取消、晚段SQL回滚、已交付保护、迟到汇总拒绝。
- Application首次扩大22通过／1失败：新增双来源测试错误地只取一层source进行downcast，原owner已有Arc<SharedSource>，需遍历完整来源链；首次改为Internal的anyhow直接downcast仍未穿透共享包装，再改为chain遍历。最终Core／完整手动重试24通过（5.088秒），包含保存失败继续后项、本地失败持久事实端口接线、结算失败保留StorageFull与PermissionDenied、旧成功不变且attempt非终态。此接缝保存端明确替身，未冒称真实Engine保存故障的SQLite验收。最终五crate库验证正在运行，不能据局部通过宣称完整spec完成。

- 首次五crate库扩大2232通过／1失败／9跳过（200.781秒）。真实Engine abandoned deletion最后立即重复删除得到Internal而预期NotFound；同源原样单测复跑4.510秒通过。实际删除代码在SQL／文件删除之后仍持ReceiveCleanup封口执行搜索清理，测试的数据消失条件不是操作完成条件。测试现保留接收端原EventStream，仅本例有一个被丢弃的操作等待，等待其OperationFinished(Cancelled)实际完成事件后再执行原NotFound断言；不改生产语义、不放宽断言、超时或并发。需重新验证此最终测试修改后的完整快照，不能将首次失败当通过。
- 该持久失败切片尚不证明结算存储本身失败时资源持有至恢复的完整移交：现沿既有非终态恢复路径，finish仍释放资源，完整预算／失败恢复责任仍须继续。真实Engine完整保存故障尚需独立注入验证；现有Engine部分失败是导出故障，不等于网络完成后的实际保存故障。

- 最终删除完成屏障4.622秒通过；同一最终源快照五crate库2233通过／9跳过（197.177秒）。metadata812包、workspace全目标check（1m43s）、repository、fmt、Rust style、1349站点隐私及diff通过。手工审查原子日志／失败事实、真实许可、终态保护及来源链，无可用code-review skill。下一片先用故障注入验证FileFailureSettlementError之后begin_receive_failure也失败时是否覆盖原双来源；实际Engine保存故障、未结算资源恢复及完整spec继续。另需核对无failure_code但未entry_bound的网络完成项统计，当前只排除持久本地失败，不冒称整个保存期间统计已统一。


- 连续写入故障红测已证实：实际ApplyInbound／Facade及原许可中，保存StorageFull、FileFailed结算PermissionDenied后，再写begin_receive_failure的ReadOnlyFilesystem覆盖了原双来源，来源链已无法找到FileFailureSettlementError（consecutive-write-red，0.007秒）。现在Root在任何批次状态写入前直接交回该未完成结算错误，保留原Receiving／Cancelling及日志供恢复；不再尝试可能覆盖来源的第三次写入。完整手动重试23项通过（5.096秒），明确断言原StorageFull与PermissionDenied仍可追溯、failure_begin_calls为0及旧成功文件不变。资源移交仍沿前轮未完成项，不把Receiving当完整恢复验收。
- 新增真实Engine保存边界注入：生产网络及实际V3／SQLite，测试在首项实际导出结束屏障处安装定向SQL触发器，仅在指定原文件网络Completed且尚无失败事实时拒绝clipboard_event插入；FileFailed记录后允许最终部分汇总，无新测试schema／生产接口。最初末项与既有导出故障组合9.561秒通过，中间保存单故障9.517秒通过；最终独立首／中／末三个Native场景共27.248秒通过。每个场景均让全部三文件网络／导出成功，指定项保存失败，其他两项实际落地；重新打开数据库核对三个网络Completed、本地storage_unavailable及交付绑定，再通过稳定Engine重试仅获取原失败摘要，其余两个成功文件路径与字节均保留。明确是数据库连接重开，不冒称该场景跨进程重启或1GiB资源验收。
- 首次最终扩大五crate库2235通过／1超时／9跳过（253.933秒），已有engine_start_builds_a_resumable_real_session超过原20秒限制；该测试原样单独复跑13.463秒通过（Summary13.485秒）。历史记录已有同名启动超时，确切触发点本轮尚未证实，未改超时、并发4或断言；最终同源快照扩大重跑中。三个新增Native保存故障在首次扩大均通过。

- 最终同一源快照五crate库2236通过／9跳过（215.351秒），启动测试11.695秒通过；metadata812包、workspace全目标check（1m34s）、repository、fmt、Rust style、1349站点隐私及diff通过。没有修改超时／并发来获得通过。手工审查完整负责人、来源、原事务／产物边界及Native仅失败项重试，无可用code-review skill。下一片回到ReceiveAction.finish→ReceiveBatch.finish清除local_work／commit_handoff的未完成结算资源问题，须实际屏障证明并落实唯一实际负责人的恢复移交；不以本轮保存故障通过冒称完整预算或全生命周期已完成。

- 保存＋物理清理故障首次真实红测（save-cleanup-red）：再写begin_receive_failure失败覆盖原StorageFull。原IsADirectory来自把本次实际文件替换为目录，未使用fake清理返回。继续优先断言原资源，单独确认是否提前释放，再统一同一动作恢复责任。

- 第二项独立资源红测save-cleanup-resource-red：物理输出仍为目录且未清理，实际Facade原保护已变0，期望1；0.006秒失败。现把保存后的物理清理责任与FileFailed结算复用原ReceiveAction恢复；清理成功布尔状态只由原处理任务持有，后续存储失败不得重新触碰该路径。

- 物理清理成功到FileFailed持久交接之间仍有崩溃窗口；本轮路径复用证明限定同进程后续CAS／存储重试，不声称新的跨进程文件身份保护。重启沿既有受管／用户目标所有权和产物日志；后续完整重启验收须核对该窗口。

- 获取失败后的物理清理红测fetch-cleanup-red：实际Facade与本地受控导出返回StorageFull，目标目录仍未清理，原保护0而非1，0.007秒失败。新恢复意图区分FileCommit、FetchFailure、FetchCancellation，只为FileCommit补写FileFailed；其余沿原网络事实及实际journaled标记完成NoEntry。取消先由原CAS取得权威，不把取消误结算为失败。普通旧partial、目录、表示以及下载结果交接本身的持久分类仍须完整验证。

- 当前Native验证前静态检查：Facade反向终态只调用ReceiveBatch.record，保存None的failure_reason；物理清理失败在workflow.item_result之前返回，registry会用batch事实覆盖SQLite正确的StorageUnavailable，可能变Unknown。新增Native.pending.items[1].failure_reason断言将直接检验，未得到实际结果前不记为已复现。修复须仅补充失败事实，不能提前调用item_result而完成LocalSettlement／释放资源；同源导入也可合并progress.rs两行相同materialization来源use。

- 该缺口已被真实Native复现：fetch-cleanup-final-native新增场景8.228秒失败，清理等待中failure_reason为Unknown，期望StorageUnavailable；同轮其余5 Native通过（总53.704秒），门禁因失败未继续。修复只在原批次内记录获取已经失败／取消的事实和来源分类，不调用item_result、不完成LocalSettlement、不移动或释放资源；业务保存失败仍等其既有FileFailed持久事实，不把未保存raw Completed提前终结。App增加原资源仍持有期间的查询断言；同源materialization导入已合并。

- 后续真实接缝：workflow获取失败／取消在remove_uncommitted_output成功后仍对record_receive_artifacts(...).await?，该写入失败可能同时丢失原获取source并释放资源，尚未取得该分支红测。考虑让原完整提交能力原子解除已网络Failed／Cancelled成员的产物责任，而非硬用FileFailed；必须区分准备失败时本项从未成功记入日志，及最终网络生命周期写入失败后raw仍非终态的情况。不要仅依据fetch返回Err推断raw状态，也不提前调用item_result来换取查询分类。

- fetch-ack-red已实际复现：输出删除成功后，原保护与原获取source的组合为(0,false)，期望(1,true)，0.008秒失败。现拟新增FileCleaned到原完整提交port，Core从当前attempt、唯一文件网络事实与已交付标记决定是否允许；Infra认证原AEAD产物并在同一事务内验证和移除本项，只为FileFailed写本地失败码。原网络Failed／Cancelled不重写；准备失败用独立PreparationFailure意图，避免对尚未登记的文件补写网络结算。原任务修复失败交接，不重删已清理路径或重试内容。

- 新合法准备状态的实际SQLite红测fetch-ack-missing-red：认证产物存在、物理输出已删除但尚无网络登记，原规则错误返回Backend/FileMismatch，0.072秒失败。Core现用Missing／Tracked／Unusable明确区分：清理允许缺失而不补造网络状态，本地保存失败仍必须有Completed；未知、重复、已交付及活动网络记录继续拒绝。新增Native SQL拒绝第二项网络登记，要求原临时输出清理、第一／三项正常落地且原任务不能永久Receiving。该Native尚待执行，不记通过。

- 普通partial重投真实Native redelivery-reuse-red在8.300秒失败：第四次实际导出是原已成功项的摘要，而非原第二项失败摘要。旧普通路径确实重新获取成功项。此测试已用新IncomingPending消费旧通知，第四次导出沿原fixture暂停并释放，不靠sleep或新订阅者。
- 现有显式重试有可复用的完整机制：RetrySnapshotReader从原wire或已保存密文快照按原表示ID加载正文，PreparedReceiveRetry＋accept_retry核对当前成员、身份与旧attempt，再由BeginReceiveRetryPort在同一事务建立全部新项及成功继承事实。原CommitInboundReceive严格要求delivered_item_ids属于当前attempt，普通begin_redelivery只换parent无法直接复用旧成功；不能只移除FileProgress禁用条件。下一步考虑抽取共享准备能力而非复制整套流程，普通投递仍使用原出生ReceiveAction，不递归另建owned任务。来源票据身份需通过既有能力验证，禁止直接新数组index套用旧landed。

- 普通 partial 重投实测绿：正常顺序与 `[1,2,0]` 顺序变化均只导出原失败摘要；既有两项成功路径不变。42项 Application 相关回归通过（42.432s）。准备器使用原票据、原表示ID与完整旧blob数组，不做新信封到旧索引的猜测映射。
- 手工代码审查发现并修复：normal prepare 仅用于带blob／manifest的再次接收，纯inline继续原地升级。现通过现有CommonDeps装配验证Applied、正常重投attempt一次、blob retry集合为空，不新增测试setter或扩大接口。
- 后续网络终态交接风险（代码观察，尚未实际红测）：ReceiverTransferHandle::finish 在 publisher 成功前设置 terminal_event，并在 publisher 失败后仍移除 registry；相同终态再次调用会直接返回旧 event，可能无法修复实际 projection。因此不能只给 Facade terminal 调用加循环；需以真实SQLite终态拒绝复现，并验证重复事件的投影修复、原下载监督和完整来源。FileCleaned继续禁止活动／未知／冲突事实，不能放宽它掩盖上层缺口。

- 完整成果重复投递实际红测：full-redelivery-red.log，9.400s，total_accepted0／total_errored1，exit100。Storage availability直接按URI解析inline_data，而实际写入经EncryptingInboundReceiveCommit／BlobCipherPort AEAD；原接收成功后source已正确释放，错误的unavailable导致普通重投准备失败。须让唯一Storage能力按原event／representation AAD认证文件清单，且Application不再unwrap_or(true)吞可用性读取错误。Engine现有blobCipher在primitive infra之后构造，可通过一次性私有port工厂后置装配，不能装一个明文／缺cipher占位实现。

- 后续终态交接定位校正：实际FileTransferHostEventPublisher只发宿主事件并返回Ok；SQLite网络事实由SqliteReceiverFileTransferStore::append将事件与projection同事务提交。该store失败时ReceiverTransferHandle::finish尚未设置terminal_event，重复相同终态可重试真实事务。因此不要把publisher失败假设当实际SQLite根因或先改其幂等语义；原DownloadTask的终态store错误早返回后未持续修复才是待用真实trigger证明的缺口。FetchPreparationState.stop_attempted失败后仍跳过也须实际验证；Completed投影失败与Failed／Cancelled投影失败要保留原网络完成／失败分类，不能放宽FileCleaned接受活动记录。

- 网络终态当前实现后的待验收边界：独立文件父流程通过FetchTerminalFailure的纯完成信号继续持有原action，信号不持下载资源，防止metadata交接未结束时新attempt。representation／directory无FileProgress时早期错误后的NoEntry恢复仍需实际测试，不能把文件47项当全路径证明；初始begin_lifecycle已形成raw pending但未返回session的失败也需覆盖。当前无新持久队列或schema，原网络task／同一DownloadSettlement持permit及保护。

- TDD后续测试边界沿用户已要求实现的spec／plan：稳定Engine操作与查询、Application完整能力、Core规则／port合同，以及明确要求核验的实际SQLite／AEAD原子性；不为观测新增Engine原始阶段查询。无CONTEXT.md。新终态恢复先通过真实Failed SQL trigger红测，再修复并通过Failed／Completed两种；取消原原因必须按既有projection核对local_user，不能把新测试错误空值要求当产品契约。


## 2026-10-23 构建与临时资料资源边界

- 所有本轮Cargo／fmt／commit保持串行，systemd scope：MemoryHigh=3GiB、MemoryMax=5GiB、MemorySwapMax=4GiB、jobs=1，保留共享target和RUSTC_WRAPPER。Started切片实际观察峰值约3.0GiB，限额scope退出后不把inactive的infinity当实测。
- 只读df发现/tmp是7.6GiB tmpfs，已用6.7GiB（89%）、剩920MiB；仓库及用户cache所在实际磁盘有847GiB可用。不能据此认定全部为本任务，也不能清空用户/tmp。后续1GiB和资料矩阵须用磁盘上的明确任务临时资料目录（例如cache下独立test-tmp），通过TMPDIR／harness资料参数传入；这是测试资料位置，不改变Cargo target／共享缓存。检查相关活动进程后回收本任务新建目录，冻结旧锚点及尚需复核资料保留至矩阵完成。
- 下一阶段按完整边界集中验证：目录成员与表示失败的原完成屏障、目录回滚实际失败、父NoEntry持久提交失败及生命周期／来源C；完成必要修复后再做一次最终资源、旧端矩阵、workspace与平台门禁。局部根门禁不能当全spec完成。


## 2026-10-23 下一条目录责任接缝

- 当前父NoEntry表示切片已全验证，后续不要扩大为目录已完成。直接源码：materialization/workflow.rs 的 DirectoryPublication::rollback 消费自身，部分rename失败也discard_staging；usecase.rs 的 withdraw_publication只记录PartialPublication计数，调用者随后仍可提交NoEntry。下一条应从完整Application接收＋真实目录输出／明确AtomicPublish故障取得证据，保留源链和原资源／清理责任，不能只更新日志或沿错误持有Publication资源。
- 现有目录持久合同只含Staging／Publishing／Landed，DirectoryPublishRecord为完整root_map、partial_visible_roots、landed；不要臆造不存在的回滚phase或新格式。RecordDirectoryPublishPort要求完整映射；重启reconciliation按非终态attempt读取root_map并以UserDestination清理。任何恢复方案须检查实际已撤回路径重用与重启所有权，不能重复触碰确认清理成功的路径。
- HostFileAccess稳定接口只有文件metadata／read_chunk／write_chunk／finish_write，未提供目录枚举；目录端到端接缝应优先使用既有完整Application接收能力和V3目录合同，不为测试新增Engine阶段／步骤入口。所有新增测试仍从用户spec已批准的完整cap与真实SQLite／AEAD／文件契约进入。


### NoEntry提交回执丢失边界已复现及局部修复（2026-10-23）
真实SQLite执行器在事务成功后返回一次BrokenPipe，持久尝试已为Failed，随后同一NoEntry返回AttemptStateMismatch（红测0.257秒）。Core现持有同条目／同尝试／同终态的只读确认规则；RolledBack还须既有settled／rolled_back事实，None只确认父终态、不移交产物。Infra在同一事务内加载及解析事实并调用Core；确认成功不执行写入。真实SQLite／AEAD 29项、Core 2项及完整Application目录结算7项通过，最终门禁仍运行；不把该组合测试写成真实Engine回执丢失注入验收。

### 目录成员网络终态边界已复现及局部修复（2026-10-23）
真实Engine目录URI复制＋V3／SDK／SQLite触发器已复现：成员仍Transferring时，父目录报告Failed（6.770秒）。表示和目录现共用私有原终态等待，先保留已知业务失败，不完成原资源交接，原成员完成后再继续目录失败结算。Core聚合保留非终态目录的已知失败原因，仍不把它当父终态。Core 3项、实际Native目录及表示2项通过（25.138秒）；扩大门禁正在进行。成功staging清理、open/probe失败及其他撤回入口仍未完成。

后续目录Native测试入口已定位：SendFilesInput只有HostFileHandle列表，当前receive_retry::Files仅支持数值句柄，不能把目录路径直接传给它。capture/usecase的build_entry_file_set支持inline text/uri-list并通过实际文件系统展开目录；应使用测试内可变HostClipboard，在配对和SQL故障安装完成后提交真实目录URI，再走ObserveClipboardChange(dispatch=true)，避免启动时提前同步。ReceiveItemRole只有Representation／File，目录成员仍是file，不存在directory_member角色；触发器应针对本次目录接收的真实file行。不得为了测试新增公开Engine步骤或发送原始信封操作。


### 提供端 C 撤销后缓存获取缺少当前权威检查（2026-10-23）

真实三端 B 消息／C ticket 已完成 A 的原始文件精确读回。撤销 C 后 A 仍确认 B Active；缓存获取经过完整原动作后得到 Integrity 而非准入拒绝，说明原始文件进入了内存解密。源码确认 IrohConnectionAdmission::Attempt::check_current 只看冻结撤销 epoch；新请求冻结的是撤销后的新 epoch，未建立连接时没有 Grant 的成员权威检查。BlobDownloadResources 对缓存前后调用 request.validate，但此检查不能拒绝新的已撤销成员请求。须仅收紧 Blob 的既有准入检查到 owner.check，保持其他协议策略。先改为真实原始文件 fetch_to_path 取得成功越权的直接红测，再改生产实现。

此前测试装配失败不算产品红测：StartupProgressInput 需 clone；终态不能从仅非终态列表查询，应从实际 IncomingPending 中取得 entry_id 并按项查询；原始文件必须用 publish_blob_path，内存 publish_blob 会加密。B 的撤销通知30秒未收敛，单列为未通过，不作为 C 获取授权的前提。


### ticket提供端额度已由完整原生动作验证（2026-10-23）

实际B消息携带C票据的获取与直接C获取共占C两槽；第三C等待。保留另外一项B原完成权后，另一B仍使用第四槽交付，避免以Uni每端流上限误认下载许可归属；释放后实际导出C／C／B／B／C、精确文件和原关闭全部通过。生产调度无需修复，测试屏障改为明确索引集合且旧24项真实接收回归通过；CI nextest额度8.363秒／授权13.933秒及最终门禁通过。总连接／tag／staging及最终交付head矩阵仍未完成。

### 下一强杀切片的只读方案（尚未实现／验证）

后续首次Staging前强杀切片的只读调研（尚未实现／验证）：
1. materialization/workflow.rs open_publication 先创建 staging 并执行 supports_no_replace，再在 build_and_publish_directory 计算 roots/plans 及 record_phase(Staging)。现有 startup sweep 只扫描当前 save_dir 与 managed cache；切换 save_dir 后旧候选不可达。reconciliation.rs:323 从已持久 root_map 推导 staging 区；phase=Staging 时不接管 final_path，因此可安全提前记录完整根计划，不需要新格式。
2. 强原生红测建议放在现有 testing::host_adapter_contract::offline_lifecycle::crash 的子模块 directory_probe，复用 OwnedChild、MemoryHostSecureStorage、persistent_engine_host 与 TCP checkpoint／stdin 私有管道。已存在 crash.rs 的 ChildInput 将 secure_storage HashMap 通过 stdin 交给 ignored 子测试，不明文落盘密钥。父进程正常创建／配对两个 Engine，保存接收端 memory secure storage 后原 shutdown；子进程重开同一 root／bind_port／keys，安装原 AtomicPublishPort 的 test-only probe 完成屏障。父 sender 从可变 HostClipboard 的真实目录 text/uri-list 调 ObserveClipboardChange(dispatch=true)，沿原V3／SDK进入目录接收。
3. 测试组合根可仿 ReceiveExports：testing::directory_probe 仅在 cfg(all(test,feature=dev-tools)) 下装饰已有 FsAtomicPublisher。assembly/sync_engine.rs:570 的 active_pull_publisher 本就 Arc<dyn AtomicPublishPort>，同时供完整 interactive/store-only 接收。按实际 probe_dir 匹配安装的旧 save_dir，先调用真实 inner.supports_no_replace，再 Notify + pause，拒绝增加 Engine 阶段接口。安装只在子进程、目标目录严格隔离；其他 publish 方法透传。
4. 子进程在真实探测已完成且 staging 实际存在后，通过完整 Engine.UpdateSettings 将 file_sync.auto_save_dir 换到另一目录，再发固定 checkpoint。父收到后 kill 子进程并 wait（OwnedChild Drop 保底 kill/wait），关闭 sender 避免重投，再用原 root／keys 启动 receiver。断言 readiness 成功、原尝试中断失败、旧 save_dir staging 消失；保留旧目录无关文件。可在 kill 后、restart 前创建原计划 final basename 的用户目录／keep 文件，验证 Staging 恢复不误认尚未发布的目标。旧代码若仅扫描新 save_dir 会留下旧 staging，必须在实际 kill+restart 后取得红测。
5. 最小生产方向：在 open_publication 的候选内先验证 manifest roots、只读解析完整 root plans、record_phase(Staging) 成功，再构造 DirectoryPublication／set_journal 并创建／探测 staging。build_and_publish_directory 复用同一 roots/plans，不重建另一计划；Publishing 与 gated rename 仍保持当前流程。避免新格式、sentinel root_map 或 Engine 查询。
6. 关键回退陷阱：若早期 journal 已持久，当前 publication.rollback 会 record_partial(0)。随后换到 managed 候选时 record_phase 的SQL upsert不重置 partial flags；旧 partial=0 会让新 Publishing 恢复跳过实际可见根。无需扩展SQL／格式：尚未开始发布的 open/probe 失败只清理 staging，保留原 Staging 日志语义，不写 partial；实际清理成功后才能写新候选 root_map。清理失败则保留原 publication 给原 batch，走现有本地 rollback／NoEntry 修复，不继续回退。
7. DirectoryPublication::commit 当前只 remove_dir_all(staging) 并 settled=true，仅 settlement_recovery.rs:33 调用。可把该内部完整清理能力改名 cleanup_staging 并迁移原调用，注明仅在无可见发布根或已持久保存时调用；open/probe 失败也用它。失败包成原 DirectoryRollbackError { visible_roots:0,cause,additional:[] }，交接原 batch，不新增清理任务。真实发布后撤回仍用 rollback，继续持久 partial=0 避免删除用户重用路径。
8. 现有 directory_settlement/preparation.rs RejectStagingLog 用 root_map[0].0.parent 推导区域并阻塞 destination；新顺序下日志失败不应创建 staging，旧 fail_log=true 测试若要求实际 cleanup 被阻塞需改为准确无产物结果，仍验证 StorageFull source、无成员下载／保存及未调用不存在的 record_partial。非日志失败 cleanup 用例继续验证原责任。
9. 路径规划无需扩大接口。可将 roots 校验和只读 plans 提前，PublishPlan 私有结构持 roots 与 plans；build 阶段沿同一计划创建、获取、验证身份、Publishing、claim commit、rename。旧 publish_gated_root 已在冲突重选时更新 logged_root_map，不要破坏它。空目录以 Directory member 表示，应保留，勿未经证据改变空 manifest 语义。
10. 所有内容为只读规划，未算完成。h12 quota gate 完成后先提交并回收 h12，再新短磁盘 TMPDIR 开此切片；继续单scope串行、共享 target／缓存和既有5GiB／4GiB上限。用户四处其他 planning 目录不动。C额度当前代码仅测试变更；生产授权修复已在 b14e110d。


### 2026-10-23：首次Staging前强杀的实际责任缺口

- 原生kill／restart红测8.87秒：原AtomicPublishPort真实探测结束后暂停、更改auto_save_dir、SIGKILL、关闭发送端、用原SQLite与私有管道钥匙串重开；readiness已ready、attempt已Failed，但旧保存目录残留一个staging。计划目标被用户重用及无关文件均独立读回，不靠当前配置扫目录。
- 原顺序为创建／探测staging后才记录完整根映射。现将根校验、冲突名称规划和原密文Staging前移，日志落库前不创建staging；原reconciliation可沿既有完整映射清理，不增加版本、字段、Engine能力或任务。
- 尚在候选打开／探测阶段的回退不写partial=0：Staging从未发布目标，恢复本来就不认领final。只有旧staging实际清理后才允许新候选替换日志，避免遗失旧责任，也避免旧候选零可见根事实污染新候选发布。
- 提前名称规划遇到原保存目录IO不可访问时，须保留原缓存回退；只针对IO，不把碰撞数量耗尽等业务失败改成回退。完整物化用例以000／500真实权限覆盖提前解析失败和稍后创建失败。
- 首轮目录相关App63项通过12.19秒；最终源的全量App、真实kill／restart及既有SDK目录／离线恢复、根门禁正在串行验证，未取得结果之前不标完成。

最终实证：首次Staging前强杀恢复11.070秒通过，连同原文件进程恢复和SDK目录终态三项40.744秒全通过；App1279通过／1跳过及全部根门禁通过。最终scope已结束，专用临时目录无活动FD并回收，完整spec剩余矩阵未关闭。


### 2026-10-23：真实SDK tag与原生连接的资源计数

- 原真实AppFacade／Uni／SDK双流用例新增SDK152c42a公开list_temp_tags计数：活动下载须真实持有tag，各轮取消A且成功B结算后源／目标均回初值0。已有source/provide调用、stream ID、唯一连接及B不重试断言保留；使用原native weak handle确认原关闭后实际释放，不新增Engine／Application查询接口。
- 初版三项8.47秒通过，27轮。最终改为64KiB源文件生成和独立逐块读回，移除fixture中与size成比例的Vec及整份输出读取；增加连续两轮1GiB。CI nextest最终四项90.320秒全通过，29轮；大文件85.427秒，两轮活动tag均2、结算均0，原生连接均1、关闭后0。源端临时tag也回0。
- 只读/proc采样实际大文件测试进程160次，读取其全生命周期VmHWM：52416KiB（约51.2MiB），观测RSS45184KiB、线程峰值29、FD22。工件resource-stream-evidence/blob-resources/large-process-sample.json及managed-isolation-false-1073741824-2.json；不含内容、摘要、身份或路径。它是实际完整获取路径，不等同于双Engine全对象图资源／所有生命周期和配额已证明；工件full_engine_resources_verified、quota_counters_verified仍false。
- 已建立completion-audit.md逐条保留13个spec验收项和48个阶段项目，未以历史勾选替代当前证据；当前提交GitHub工具仅查询PR-triggered runs为空，未把其推论为所有事件无CI，也未登记通过。


### 完整验收及观测环境接续

- 最新完整关闭实现收口ReceiveReadinessCoordinator.close，2项真实registry合同0.04秒通过；原生锁定／暂停／在途成员撤销／删除／清空5项34.846秒通过。根metadata、workspace all-targets（3分26秒）、fmt、style、repository、diff均退出0，源码在当前完整回归scope内保持不变。
- 当前仍在uni-blob-complete-validation scope构建／执行workspace组，随后成员全集、升级smoke。不可同时启动Cargo或改变Rust／HEAD；状态completion-status.log，活动exec session53955。
- Docker只读访问permission denied，sudo -n也需要密码；尚未请求提权，采用官方同版本独立二进制准备本地验收。GitHub已连接个人账户与本仓Git作者一致；API核实Jaeger2.20.0、Collector-contrib0.160.0，官方下载分别53590436／100171851字节，SHA256为582261d2d8e46721246f5f8072dca7f6762355b248e0dc493bb85990e6226952／bff414e6a287309dfd0c51350c1e13c96c4ab807127b4320bd684f605e781326。工具不支持binary，使用urllib流式下载并验证后解包于target/blob-resource-tmp/observability-tools；session74333仍在下载Collector，Jaeger已验证并可运行--help，尚未启动任何服务。结束前须检查进程／FD并清理。
- 下一观测证据缺口：真实Engine file retry fixture目前不安装OTLP；既有clipboard_trace测试走真实Engine→MockServer OTLP，并验证text／多目标部分完成，不能冒充文件／单项重试／取消的Collector可视化验收。静态看到receiver的Applied ack一律succeeded，ClipboardReceiveObservation只保留failure；RetryEntryReceive dispatch未见专属业务装饰。须先以真实文件流程导出并验证，不能仅据源码改产品或为观测扩Engine阶段API。可复用既有init_test_tracing_with_otlp和flush_test_tracing（显式test环境）及ReceiveCase真实能力，SDK／业务协议不变。


完整验收定位补齐：spec5.3全部默认常量与唯一policy.rs逐项一致；scheduler／timing／budget／cancellation／provider runtime已有恰好到期、累计预算、队列和许可结算覆盖，具体路径已写入completion-audit。当前仅是静态覆盖核对，等正在运行workspace动态结果后才可登记通过。根repository架构负例实际通过，拒绝旧SDK自拨池、对端级取消和原生handler回流。

### 真实文件观测闭环已实际验证

同一真实Engine／SDK／SQLite用例15.991秒通过；严格生产processors逐字匹配。实际Jaeger列表与展开Tags截图逐一查看：初始接收partial，输出故障error.type=storage，独立手动重试ok／cancelled（64.8ms／5.1s），各含export及持久化子节点；旧send仅106／154ms，没有被后续重试延长。92 spans／103完成logs无缺父节点或结果不一致，环境test且无fixture内容／名字／口令；观测绿工件在observability-green/，原失败与中间部分绿仍保留。collector文件出口晚于HTTP flush，验收先等待原批次到齐再判语义，不能通过重跑业务覆盖失败。

### 完整验收剩余生命周期与资源边界

重新核对spec §8原文，集成验收明确要求成员撤销、锁定、切Space、删除及shutdown后检查任务／流／连接／FD／许可／未完成产物。现有新Native明确覆盖锁定／暂停／源成员撤销／删除／清空／离线通知取消，但尚无活动导出中的本机离组或切Space事实；不能只据共用close推论这些已通过。稳定API没有SwitchSpace／LeaveSpace操作，真实本机离组沿远端RemoveMember→QueryDeviceGroupChoices→ChooseDeviceGroup(apply,confirm_local_removal=true)；现有成员E2E harness/membership_topology.rs:814及removal_convergence.rs:R2可复用行为步骤。切Space应进一步核对原ResetSpace／JoinSpace与成员分支切换的实际生命周期，不新增观测阶段API。

资源证据目前分层：Engine2×1GiB的RSS／FD／线程／任务；真实AppFacade-Uni-SDK的tag2→0及native连接1→0；原生三端C额度用例的4全局／2提供端占用与释放行为。它们不能冒称每种Engine生命周期的全部数字已直接采到；后续先定位既有测试装配与资源采样能力，在原合法测试边界补证据，避免为观测扩大业务facade／port／结果。当前完整candidate串行Cargo运行中，未修改Rust来补这些缺口。

### 切Space／本机离组正在取得真实证据

合并main后新增lifecycle/space.rs两项，只用既有ResetSpace／RemoveMember／ChooseDeviceGroup／readiness／成员／进度查询，真实第四次SDK导出持完成权，验证旧输出清理和两份旧成功。唯一scope uni-blob-space-lifecycle（exec44078），尚在构建，不计通过。静态定位：RebuildSpaceUseCase执行prepare(snapshot)→stage→rebind→membership rebuild→commit/finalize，目前未见原session activity封口；SpaceFacade.reset只调用该owner并wake maintenance。原SpaceFacade已经有DeferredSpaceSessionActivity并在bind_session_activity绑定真实receive/search/connections，pause_for_lock内部先完整close_receive_gate再pauseconnections/search；combined activity还包membership暂停。若Native证实reset缺口，应由Application完整Space重建owner统一暂停／实际排空、快照／重绑／提交、恢复，不让Engine增加步骤或Infra编排业务；注意maintenance exclusion与membership.pause的锁顺序，不能等拿到维护排斥锁后再等待maintenance停止。尚未修改生产实现。

已纠正仅看App Rebuild的静态推断：完整稳定Engine ResetSpace先走SessionSupervisor.reset_space，封operations、等待/关闭旧session、取消原file transfers，再调用App完整数据重建并安装新session；交接期间facade缺失使readiness查询按1103不可用。真实本机离组6.505秒通过，Reset当前关键生命周期／产物断言也已通过，只在新Space重新配对态使用了不适用的device-group选择查询，现按既有Reset验收改用ListDevices。未修改生产行为或增加接口；不将测试合同错误称为生产缺口。

生命周期资源首轮实际中间结果：锁定34.149秒、源撤销33.175秒、本机离组33.227秒通过；三份原始报告均initial fds15/threads17/tasks0，closed_after_idle同为15/17/0，实际idle26.230／28.083／27.493秒。说明只看动作返回时的线程即时计数会误判原共享库空闲回收；专门外层90秒包含观察，业务取消5秒等内部合同未改。剩余三场景仍在同一串行scope。

下载真实计数已补：原三端C场景9.583秒通过，5个scheduler实例、2个实际工作，主实例peak active4／per-provider2／queued1，两个实际工作实例最终closed、settled且slots/active/queued0。原SDK资源证据仅Infra层，现开始真实Engine直接计数：IrohPeerConnection构造路径仅测试显式模式注册weak_handle，技术getter只返回registered/alive两个整数；SDK已有BlobStoreLifecyclePort.shutdown原能力在store仍开时数temp tags，固定数字工件不含hash／tag名。新test-util模块不创建后台采样任务，不返回连接，Engine只在私有测试采样器组合外部/proc与技术弱句柄数。先锁定单项实测（exec17798），不据代码假定所有资源已绿。

- SDK技术资源证据只在test-util＋显式资源目录开启：原生计数保存WeakConnectionHandle，不持强引用、不启动任务；原store shutdown前查询真实temp tags。锁定场景实际弱句柄2→0、tag3次0，不把端侧句柄数当作跨双端去重的逻辑连接数。完整Engine资源与下载quota分别使用现有真实计数，聚合器要求各自原始证据。
