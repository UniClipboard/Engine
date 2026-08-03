# Findings

## 2026-08-03 Phase 5 文件传输会话审计

- 本阶段只建立文件传输的完整负责人并迁移一条真实入站路径；移动上传的活动表、临时文件和稳定四动作留给详细 Phase 6。
- 退出条件要求创建时建立开始状态，进度由会话校验，完成、失败和取消互斥且幂等，关闭时所有活动会话必须得到终态。
- 工作区仅有用户既存的成员移除方案和对应架构维护记录；本阶段不修改、删除或提交这些内容。
- 当前 `FileTransferFacade` 公开 `start`、`report_progress`、`complete`、`fail`、`cancel` 五个独立动作，并分别持有五个 use case；调用方必须自行保证顺序和终态。
- receiver projection 的 pending/provisional 建立与 lifecycle 开始状态也是独立入口，真实调用方可以建立其中一半后失败，留下半流程。
- 现有 10 项集成测试直接构造和调用五个 use case，保护了旧分步 Interface；本阶段必须迁到最终 `FileTransferSession` 边界后删除旧测试形状。
- 真实调用者包括 Blob 拉取、移动同步导入和后续移动流式上传；详细 Phase 5 要先迁移一条 Blob 入站路径，移动流式上传留给 Phase 6。
- `AppFacade` 和 Engine 运行期仍直接持有并公开 `FileTransferFacade`，后续审计需区分真正需要的内部装配与应删除的公共逐步入口。
- 五个 use case 每次都先加载历史再追加事件，没有跨动作的共享互斥；两个终态并发时可能都基于 active 历史继续，单靠 `TransferTimeline` 不能保证唯一终态。
- 远端 Blob 拉取是首条合适迁移路径：当前 seed、start、complete/fail/cancel 和各自重试分散在 `BlobTransferFacade`，调用方掌握完整状态推进知识。
- Engine 侧现有 `FileTransferLifecycle` 负责 pending/transferring 超时、启动时遗留失败和缓存清理；它是数据库恢复与健康维护，不提供同进程会话互斥，也不替代会话关闭。
- 新会话应复用现有事件存储和宿主发布，不复制数据库恢复；关闭时只终结当前进程登记的活动会话，重启后的遗留仍由现有启动恢复处理。
- `FileTransferHostEventPublisher` 已能把 Progress 事件转换为完整宿主进度通知；Blob 的 `HostEventProgressSink` 当前绕过它直接发同类事件，迁移后应只经会话上报，避免两套进度路径。
- Blob 批量拉取会在多次调用间共享同一 `transfer_id`，第一项开始、最后一项结束；会话必须由 facade 注册表持有，不能只靠单次 fetch 的局部对象生命周期。
- 取消流程还需要先反向通知发送方并停止网络拉取，再让会话记录取消；会话只隐藏状态推进，不接管 Blob 网络取消顺序。
- `FileTransferEventStorePort` 只有独立 load/append，没有事务性 compare-and-append；唯一终态必须通过进程内同 transfer 互斥保证，跨重启遗留由启动恢复先收尾。
- 最终实现由 `FileTransferFacade` 登记活动会话，`FileTransferSession` 固定传输编号、对端、接收登记方式和进度；同一会话内所有写入串行执行。
- 创建会话会在同一个入口写 receiver pending 或 provisional 上下文和 Started；Started 保存成功但发布失败时会话仍保留，重试只会复用，不会追加第二个 Started。
- 完成、失败和取消以持久化成功为本地状态切换点；相同终态重复调用幂等，不同终态并发只允许第一个写入。
- Blob 批量拉取通过活动表跨调用复用会话，并把每一段局部进度换算为累计进度；带会话时所有本地进度都经现有事件发布器，不再直接旁路宿主事件。
- Blob 取消继续保持发送方通知、网络取消、连接关闭、会话结算的既有顺序；会话没有接管网络细节。
- 为删除旧五动作入口，移动流式上传本阶段只提前迁移文件传输会话句柄；活动上传表、暂存句柄、节流和失败清理仍在 Engine，完整迁移继续属于详细 Phase 6。
- 稳定 `AppFacade` 已删除文件传输对象字段；应用内部真实使用者只通过会话推进，五个旧 use case 及其 10 项旧边界测试已删除。
- Engine 暂停在网络运行期退出后取消剩余会话，最终关闭停止接受新会话；数据库超时和启动恢复仍由原 `FileTransferLifecycle` 负责，没有复制职责。
- 删除检查通过：若删除 `FileTransferSession`，同一传输的互斥、累计进度、幂等终态、批次复用和关闭终结会重新散落到 Blob、移动上传和 Engine；当前对象隐藏了真实复杂度。

## 2026-08-03 Phase 4 剪贴板入站运行期审计

- 当前引擎层 `spawn_clipboard_runtime_tasks` 仍订阅解密后的通知、生成宿主事件、调用入站应用、把结果映射为 sender receipt，并由引擎任务表取消循环。
- 应用层当前分成两段：`IngestInboundClipboardUseCase` 只负责订阅原始接收、成员策略、解密和广播；`InboundClipboardFacade` 只负责单条通知到应用结果的转换，中间完整流程仍由引擎拼接。
- 最终 `ClipboardInboundRuntime` 必须同时持有原始 ingest 和内容应用，并自行拥有后台任务与关闭等待；否则删除引擎循环后相同知识会重新散落到调用方。
- 宿主通知必须继续使用收到时的元数据和轻量摘要，不得携带完整剪贴板正文；sender receipt 必须由最终应用结果唯一映射为成功、重复或拒绝。
- 本阶段需要先从最终运行入口固定四类结果与关闭等待，再迁移生产装配；不能保留旧订阅入口作为第二套生产路径。
- `ClipboardSyncFacade::subscribe_inbound_notices` 目前为每个调用者再启动一条桥接任务，并由订阅对象在 Drop 时中止；生产接收只需要一个消费者，这层广播和桥接在最终运行期中应删除。
- `ClipboardSyncFacade::spawn_ingest_loop` 返回的句柄当前存放在 `SyncEngineAssembly`，网络关闭时直接 abort；最终运行期需要显式关闭信号并等待原始 ingest 与应用循环退出，而不是依赖 Drop 中止。
- `AppFacade` 仍公开原始通知订阅，旧 Engine 端到端测试也直接订阅并手工完成 receipt；这些都是 Phase 4 结束时必须审计、删除或限制到测试范围的旧内部步骤入口。
- 生产启动当前分裂成两处：网络装配时立即启动原始 ingest，Engine 会话启动后再把通知应用循环注册进全局任务表；两者关闭顺序也分别位于 `SyncEngineAssembly::shutdown` 和全局任务表。
- `SyncEngineAssembly::shutdown` 在网络路由关闭前直接 abort ingest；最终运行期若仍依赖该顺序，必须由自身显式取消并 await 两个任务，网络路由关闭只作为发送端自然关闭的后备信号。
- `build_sync_engine_assembly` 在应用内容处理依赖装配完成之前就启动 ingest，因此 Phase 4 需要把启动推迟到完整 `ClipboardInboundRuntime` 构造完成之后，防止启动期间出现无消费者而拒绝帧的窗口。
- 最终设计采用单任务 `ClipboardInboundRuntime`：直接订阅原始接收流，依次执行成员策略、解密、内容类型策略、应用、宿主通知和 receipt settlement；不再保留解密后广播与每订阅者桥接任务。
- 原 ingest 规则将保留为运行期内部的单条准备步骤，但旧 `spawn_ingest_loop`、`subscribe_inbound_notices`、`IngestHandle`、`InboundNoticeSubscription` 和引擎任务注册入口必须删除。
- 运行期宿主通知使用应用层定义的轻量事件端口；引擎只把该事件转换为稳定 `EngineEvent`，不再知道应用结果分支或 receipt 映射。
- 最终关闭由运行期取消令牌和所持 join handle 实现；Drop 仅作为兜底取消，正常 `shutdown` 必须 await 任务退出，并在网络路由关闭之前执行。
- Engine 会话构造时已经同时拿到 `EventSender`、完整应用依赖和网络接收器，因此可以在 `build_clipboard_runtime` 中一次启动最终运行期，不需要让网络装配提前启动半条流程。
- `AppFacade::subscribe_inbound_clipboard_notices` 当前没有调用者；删除它不会改变稳定 `uc-engine` operation 或事件契约，并能关闭一条绕过最终运行期的内部入口。
- Engine 暂停/关闭当前先结束全局任务表、再关闭搜索和网络；迁移后需在网络关闭前新增 `clipboard.shutdown().await`，并从全局任务表删除旧入站任务，保持其余任务顺序不变。
- 网络接收器目前被移动进 `ClipboardSyncFacade` 只为旧 ingest 使用；迁移后它应由网络装配保留为仅供完整运行期构造的能力，而 outbound `ClipboardSyncFacade` 不再持有或暴露入站生命周期。
- `ClipboardSyncFacade` 仍需要成员、密钥和时钟用于 outbound 规则，因此只删除 `clipboard_receiver` 与 ingest 字段，不改变发送、投递视图或接收取消能力。
- `usecases::clipboard_sync::ingest_inbound` 的八项策略/解密测试不能简单丢失；其可观察规则需要迁到最终运行期测试后才能删除旧模块。
- `slice2_phase2_clipboard_e2e.rs` 的三项测试全部以已删除的原始通知订阅和手工 receipt 为核心，其中“相同内容接收两次”的断言已经与当前 receiver-side 去重事实相反；该文件应整体删除，而不是适配旧步骤。
- 该旧 E2E 的有效覆盖分为两类：真实双端成功/重复/关闭已有稳定 Engine 场景保护；接收总开关和内容类型策略需要在最终运行期增加直接测试后再删除旧文件。
- 最终运行期已补齐接收总开关、成员缺失、成员读取失败、解密失败后继续处理和文字类型拒绝测试；策略拒绝均在内容应用前完成并产生 rejected receipt。
- 旧 E2E 已整体删除；真实双端成功、重复、历史不增和关闭继续由稳定 Engine 场景保护，不再保留与当前去重事实相反的内部订阅测试。
- 全仓旧名称扫描无匹配；原 ingest、公开订阅、桥接任务、薄应用包装、引擎循环和手工 receipt 映射均已删除。
- 删除检查通过：若移除 `ClipboardInboundRuntime`，成员策略、解密、分类、应用、事件、receipt 和关闭顺序会重新散落到引擎或其他调用方；当前模块隐藏了完整流程，不是转发层。
- 关闭循环使用取消优先的选择顺序：允许当前正在应用的内容完成，但取消后不再启动已排队内容；直接测试证明只产生当前内容的应用结果。

## 2026-08-03 Phase 3 入站模式审计

- Phase 3 实现后，生产代码只剩两个构造位置：普通 P2P 接收使用 `interactive_receive`，活动剪贴板按需拉取使用 `store_only_pull`；`ApplyInboundClipboardUseCase::new` 的剩余调用全部位于 `#[cfg(test)]` 模块。
- `StoreOnlyPull` 的构造参数没有系统剪贴板写入、活动寄存器、重复内容恢复、临时接收终结或出站进度能力；成功行为测试证明它仍会持久化新内容并进入搜索。
- 引擎层 `NoopPullStoreWrite` 已删除；只保存语义不再依赖调用方提供假写入器。
- 普通接收的完整模式强制提供系统剪贴板写入、活动寄存器、移动可用性判断、重复内容恢复、临时接收终结和出站进度。
- 删除检查通过：若删除两个命名模式，普通接收和按需拉取的能力差异会重新散落到引擎装配调用方；当前模式构造隐藏了真实差异，不是单纯转发。
- 普通 P2P 入站在 `crates/uc-engine/src/assembly/clipboard_runtime.rs` 通过 `ApplyInboundClipboardUseCase::new` 后连续调用十余个 `with_*` 方法完成生产拼装；缺少其中任一能力仍可编译和启动。
- 活动剪贴板按需拉取在 `crates/uc-engine/src/assembly/sync_engine.rs` 重复构造同一入站流程，并通过引擎层 `NoopPullStoreWrite` 假装系统剪贴板写入成功；只保存语义依赖调用方传对假对象，而不是由应用层模式保证。
- 普通接收必需能力包括写系统剪贴板、活动状态推进、重复内容重新激活、临时接收终结和出站进度；只保存拉取必须明确排除这些能力，但仍需要文件落地、接收记录、搜索、可用性判断和宿主事件。
- 当前单元测试大量使用自由拼装覆盖细分失败分支；Phase 3 可将旧 `new` / `with_*` 缩为仅测试编译可见的真实测试接缝，同时让所有生产调用只能使用两个完整构造入口。
- 本阶段只收口单条内容的模式构造；通知订阅、确认、宿主结果映射和关闭循环属于实施计划 Phase 4，不提前混入。

## 2026-08-03 Phase 2 搜索运行期审计

- 空间会话已经在创建、解锁和恢复后统一通知搜索恢复，但锁定和重置前没有暂停搜索；当前 `SearchSessionActivityPort` 只定义恢复动作。
- 生产搜索仍由 `uc-engine` 创建 `SearchCoordinator`、把它作为可选项传给 `SearchFacade`，再通过引擎任务表单独启动，关闭也依赖引擎任务表。
- `SearchFacade` 仍使用 `Option` 和 `OnceLock` 表示后台能力，保留运行中 `set_coordinator` 补装入口；重复补装会执行生产代码中的 `expect()` 并终止进程。
- 当前仓库没有 `set_coordinator` 的真实调用者，删除该入口不需要兼容过渡。
- Phase 2 需要建立 `SearchRuntime`，由它一次性构造生产搜索、持有后台任务并等待关闭；只查询场景改用名称明确的只读构造方式。
- 搜索暂停必须等待正在运行的重建和修复退出，并允许后续会话恢复重新开启任务范围；进程关闭后则永久拒绝新后台任务。
- 删除检查通过：若移除 `SearchRuntime`，引擎层必须重新创建、启动、取消并等待搜索后台工作，且生产与只读构造会重新混在同一路径；该模块确实隐藏了完整生命周期，不是转发层。
- Phase 2 完成后，仓库扫描只在搜索模块内部保留协调对象；引擎层没有搜索恢复、暂停、协调器启动或补装调用。

## 2026-08-03 Phase 1 初始盘点

- 用户已明确恢复本计划，因此活动计划指针从已完成的空间前置计划切换到本计划。
- 工作区另有 `.planning/2026-08-03-member-removal-deadlock-recovery/` 和对应架构维护记录，属于既有改动，本计划不修改或覆盖。
- 历史维护现有顺序与失败策略测试位于 `crates/uc-engine/src/subsystems/history_maintenance.rs`，直接测试引擎层内部步骤；它们不能作为后续删除引擎维护循环时的稳定行为保护。
- 移动上传已有 `engine_mobile_upload_owns_transfer_lifecycle_events` 从 Engine 宿主适配入口观察生命周期事件；仍需逐项核对开始、追加、完成、取消、追加失败、进度失败和关闭清理。
- 文件传输当前已经用时间线拒绝终态后的再次推进，但仍需确认保护是否通过未来负责人 Interface，而不是固化逐步状态入口。
- 剪贴板入站已有应用层和引擎层零散覆盖，尚需按成功、重复、解码失败、文件失败、只保存差异和关闭逐项建立覆盖矩阵。
- 移动上传稳定 Engine operation 已覆盖正常开始、追加、完成、取消、重复取消，以及暂停/恢复后旧句柄失效；生命周期事件测试还覆盖开始、进度和完成事件。
- 移动上传尚未从稳定入口覆盖暂存追加失败、传输进度记录失败，以及 `shutdown` 直接终结活动上传；这些分支需要可控的宿主或最终负责人 Interface 才能稳定注入。
- 当前移动上传活动表、暂存句柄、字节计数、进度节流和各失败分支都仍在 `crates/uc-engine/src/runtime/mobile_upload.rs`，符合后续 Phase 6 的迁移目标。
- 当前剪贴板通知订阅、Engine 事件、应用结果分类和 sender receipt 确认仍集中在 `crates/uc-engine/src/assembly/clipboard_runtime.rs`；这正是 Phase 4 要整体移走的循环。
- `crates/uc-engine/tests/slice2_phase2_clipboard_e2e.rs` 虽然使用真实 P2P 传输，但测试直接取得 `ClipboardSyncFacade`、订阅内部通知并手工完成 sender receipt；它证明底层传输，却固化了 Phase 4 要删除的编排，不能作为稳定 Engine 行为保护。
- 剪贴板应用用例已细致覆盖写入失败、文件部分失败、重复内容、重新浮现和接收终态等规则；Phase 1 不应复制这些内部断言，而应补一条稳定 Engine 场景确认对外历史、事件和关闭结果。
- 文件传输时间线会拒绝终态后的进度或第二终态，现有行为规则位于应用层；Phase 1 仍需把“有且仅有一个终态”保护放在未来会保留的负责人边界或稳定 Engine 可观察结果上。
- 稳定 Engine 已提供空间创建、邀请、加入、剪贴板观察、历史查询和关闭动作，可以建立双 Engine 的真实 P2P 入站保护场景，无需新增公开契约。
- `slice1_handshake_e2e.rs` 和 `slice2_phase2_clipboard_e2e.rs` 已提供 loopback iroh 与本地 rendezvous 的可靠搭建证据，但都绕过稳定 Engine；新基线测试应复用场景知识，而不是复用其内部 Facade 调用方式。
- 稳定 Engine 目前没有文件传输状态查询 operation，文件传输终态只能通过事件或应用负责人边界观察；移动上传的 `TransferProgress` 事件已经提供一部分稳定观察面。
- `EngineConfig::with_rendezvous_base_url` 在 `dev-tools` feature 下提供测试用配对服务覆盖，适合建立不依赖公网的双 Engine 场景。
- 新剪贴板基线测试可以放在 `host_adapter_contract` 中复用公开宿主接口的内存实现，但断言只使用 `Engine::start`、`Operation`、`OperationResult`、`EngineEvent` 和 `shutdown`，不访问应用 Facade。
- 有效 payload 的成功、重复和关闭可以立即用双 Engine 场景固定；稳定发送 operation 不允许制造畸形 payload，因此解码失败和受控文件失败仍应在 `ClipboardInboundRuntime` 最终 Interface 出现时补齐，而不是为测试新增危险的公开“发送原始密文”入口。
- `SendText` 的空目标列表代表向全部可发送成员派发，适合作为双端成功场景；第二次应使用稳定的 `ResendEntry` 动作发送同一 entry。
- 当前网络通知的 `action` 不可靠地区分应用层重复内容，因此重复行为应以 sender 的重发结果和 receiver 历史仍只有一条为准，不把内部通知分类固化为契约。
- 稳定 Engine 双端场景已实际通过：真实配对后首次发送返回一个 accepted，接收端产生入站事件并只落一条历史；重发同一 entry 返回一个 duplicate，历史仍为一条；双方 `shutdown` 均在 15 秒期限内完成。
- 移动上传生产暂存由 `FilesystemMobileFileStaging` 管理；每个流式上传有独立 scope 目录，`abort_stage` 会删除部分文件并尝试删除空 scope 目录。
- `ProductionRuntime::abort_all_mobile_file_uploads` 已在运行期停机路径调用，但现有稳定入口测试只在 suspend/resume 后断言句柄失效，没有扫描缓存残留或确认 shutdown 事件流关闭。
- 宿主 `private_data` 会派生出 `<private_data>/file-cache`，因此真实流式暂存目录为 `<private_data>/file-cache/mobile_inbound`；关闭清理可以通过关闭前后递归文件数量直接验证。
- 移动上传关闭清理的稳定 Engine 测试已通过：追加 4 字节后真实暂存文件数为 1，`shutdown` 完成后为 0。
- 剪贴板应用规则已有明确测试：`decode_failed_on_truncated_envelope`、`partial_materialize_persists_entry_but_skips_os_write`、`file_cache_blob_materializer_removes_reserved_placeholder_on_fetch_error`、多组 duplicate/resurface 测试和 happy-path 事件测试。
- 上述剪贴板用例测试可以保护规则迁移，但不能替代 Phase 4 最终 `ClipboardInboundRuntime` 的确认与关闭测试；Phase 4 退出条件必须明确迁移这些场景到最终 Interface。
- 文件传输 `TransferTimeline` 已实现终态后拒绝继续推进的规则，但当前模块没有直接单元测试；该时间线规则会被最终 `FileTransferSession` 复用，适合在 Phase 1 补纯行为测试，不依赖旧 Facade 的逐步公共入口。
- `MobileSyncFacade` 流式方法本身没有对应成功/失败测试；正常行为目前主要由稳定 Engine 的 lan-compat 场景保护，受控端口失败应等 `MobileFileUploadCoordinator` 最终 Interface 建立后注入。
- 文件传输时间线新增的两项测试已通过：任一终态后 `ensure_active` 均返回已结束，完成后再追加失败终态也被拒绝。
- `crates/uc-application/tests/file_transfer/` 已有 10 项集成测试，覆盖开始、进度、完整完成流、倒退、对端不匹配、取消后再失败和非法历史；新增时间线测试补足完成/失败/取消三个终态的对称性。
- Store-only pull 在 `sync_engine.rs` 使用 `NoopPullStoreWrite`，明确只持久化，实际系统剪贴板写入由后续 active-clipboard convergence 负责；相关应用层边界测试位于 `facade/active_clipboard/mod.rs::pull_store_tests`。
- Store-only pull 当前测试只证明接收策略拒绝发生在持久化前，没有成功拉取“不提前写系统剪贴板”的最终入口测试；该缺口应在 Phase 3 建立明确模式时补齐。
- 历史维护现有两项引擎内部测试准确固定现状：reconcile 失败跳过 cleanup/retention；cleanup 失败仍继续 retention，顺序固定为 reconcile → cleanup → retention。Phase 7 必须将这两项断言迁到 `HistoryMaintenanceRuntime` 最终 Interface 后删除旧测试。
- 移动上传进度持久化失败已通过稳定 Engine operation 验证：追加返回 code `1448`、`Internal`、可重试，暂存目录为空，同一句柄再次追加返回 code `1447` / `NotFound`。
- 暂存文件 append I/O 失败无法在普通真实文件系统上安全、稳定、跨平台地注入；不应通过进程级文件大小限制、信号处理或关闭任意文件描述符制造。Phase 6 的 `MobileFileUploadCoordinator` 必须提供可控 `MobileFileStagingPort` 测试边界并将该场景作为退出阻断项。
- Phase 1 当前完成了稳定入口可表达场景的保护和全量覆盖矩阵，但仍有三类最终负责人阻断项：移动上传 append I/O 失败、StoreOnlyPull 成功时不提前写系统剪贴板、历史维护关闭立即唤醒。未将 Phase 1 标为完成。

## 2026-08-03 前置条件复核

- `.planning/2026-08-02-space-setup-deps-design/` 的 Phase 0 至 Phase 8 已全部完成。
- 空间创建、解锁、恢复、锁定、加入、切换、成员传播、在线维护和关闭已完成职责收口。
- 稳定契约、应用层测试、真实 Engine 场景和全仓交付检查已有通过记录。
- 本总计划可以从 Phase 1 开始；本次不修改 `.planning/.active_plan`，也不提前实施 Phase 1。

## 审计结论

- 复杂度外溢的判断标准不是文件大小或依赖数量，而是调用方是否需要掌握内部顺序、失败清理、后台重试和关闭方式。
- 剪贴板入站由引擎层订阅通知、应用内容、确认结果和发送事件，是当前最明显的跨层流程外溢。
- 剪贴板入站应用通过大量可选能力拼装，生产路径可以形成行为不同的半完整对象。
- 移动文件上传由引擎层持有活动上传表、临时句柄、字节进度和所有失败清理，应用层只提供内部步骤。
- 文件传输状态把开始、进度、完成、失败和取消分别暴露给调用方，无法从 Interface 保证唯一终态。
- 搜索支持构造后补装后台协调者，空间解锁和恢复后还要求引擎层额外通知。
- 历史维护的顺序、时间间隔和失败策略位于引擎层，应用层只暴露三个独立步骤。
- 成员入口的说明仍称只负责查询，但实际同时承担成员移除、旧空间处理、失败恢复和重试。
- `AppFacade` 同时公开内部对象和转发动作，调用方可以绕过统一入口。
- 通用生命周期入口只修改状态，没有执行真实恢复，也没有生产调用者。

## 决策

- 当前空间计划保持活动状态，本总计划不修改 `.planning/.active_plan`。
- 搜索会话和成员恢复优先通过当前空间计划收口，不建立竞争负责人。
- 剪贴板入站使用少数命名完整模式，不继续增加独立可选能力。
- 移动流式上传保留四个稳定用户动作，但临时文件、进度和失败清理由应用层统一拥有。
- 文件传输先建立内部会话，再迁移移动上传和其他入站来源。
- 历史维护保持现有失败策略，本次只移动所有权，不顺便改变业务行为。
- `AppFacade` 最后收紧，避免在功能尚未收口时只增加转发层。
- `ActiveClipboardFacade`、`ConfigMigrationFacade` 和 `BlobTransferFacade` 不因文件大或依赖多而整体重构。

## 证据位置

- `crates/uc-application/src/facade/clipboard/facade.rs`
- `crates/uc-application/src/usecases/clipboard_sync/apply_inbound/usecase.rs`
- `crates/uc-engine/src/assembly/clipboard_runtime.rs`
- `crates/uc-application/src/facade/mobile_sync/facade.rs`
- `crates/uc-engine/src/runtime/mobile_upload.rs`
- `crates/uc-application/src/facade/file_transfer/facade.rs`
- `crates/uc-application/src/facade/search/mod.rs`
- `crates/uc-engine/src/operations/space/session_recovery.rs`
- `crates/uc-application/src/facade/clipboard_history/mod.rs`
- `crates/uc-engine/src/subsystems/history_maintenance.rs`
- `crates/uc-application/src/facade/roster/facade.rs`
- `crates/uc-application/src/facade/app_facade.rs`
- `crates/uc-application/src/facade/lifecycle/mod.rs`

## 非目标

- 不全面审计算法正确性、安全漏洞或性能。
- 不因为文件行数多而拆分 Module。
- 不改变公开 Engine 契约。
- 不在本计划中重写当前正在实施的空间计划。
