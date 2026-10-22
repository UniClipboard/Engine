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

- 续接 a829f384 为 progress；工作区仅用户四个原有规划目录。当前完整切片将条目取消流程移至 transfer/receive/cancellation：入口按条目／attempt 合并请求，五秒只限制外部等待，实际 ledger 读取／取消声明、全部子获取停止、投影、staging 和最终 CAS 由同一个后台任务完成。任务归现有 transport 生命周期，调用方丢弃等待及重复取消不丢工作；原 readiness/reconciliation 继续负责重启，不新增持久状态或格式。
- 新协调者定向10项通过（0.034秒）：包括首次权威读取／最终提交两种慢阶段，五秒返回后不遗弃，并由真实生命周期收集后完成；panic 保留 JoinError 且不继续清理。首次组装编译发现 FileTransferAssembly 未保留 bus、迁移测试重复导入，已修正并保留 cancellation-owner-check.log。
- 取消业务逻辑与专属 port／模型／错误／测试已移出 facade，facade 仅接完整负责人；停止失败立即结束，不再继续清理正在使用的产物。原只保留首个子失败的逻辑改为结构化复合来源；取消终态通知移至实际子任务／会话结算后，TooLate 或清理失败不预发 Cancelled。

- 增加独立取消工作表过载测试：68个已接受慢读取占满工作位，第69个立即 Busy、不产生额外读取；已接受动作仍全部完成提交。该表不占用下载4／2池，避免停止与下载互锁；上限由可接受下载请求数4+64推导，重复key直接共享原任务。
- 本轮完整App回归1164 passed／1 skipped（3.828秒），Engine公开契约50 passed（0.022秒）。已迁移语义为明确的完整停止后才能继续投影／清理，去掉常量失败计数旧接口；业务失败由完整负责人记录一次，Engine共享结果只映射稳定错误。workspace检查与最终门禁仍在进行，未标记完整spec完成。

- 上一状态答复仅核实日志并说明缺口，未改变实现，归为 no progress；本轮继续推进完整目标。最终 workspace 全目标检查通过（1m15s），metadata、fmt、Rust style、repository preflight 通过，当前无活动 Cargo 进程。补齐当前取消负责人文档，并手工复核实际停止后清理顺序、关闭收集和多来源错误；code-review skill 不可用，不冒充技能审查。完整 ReceiveWorkPlan 的物化／条目结算仍是下一实作，未标完成。

- 物化负责人开始迁入 transfer/receive/materialization，模型、专属 port 和流程分文件；移除 materialize 旧参数入口，仅接收 ReceiveWorkPlan，并迁移全部真实调用／mock。首次 check 通过后清除旧模块二次导出，扩展编译暴露 facade 三处旧类型路径，已改直接导入完整负责人；receive-plan-owner-application.log 保留首轮失败，未视为测试通过。

- 全文相对链接扫描发现历史审查 inventory 的旧 uc-infra 路径已有失效，非本轮新增；辅助对照脚本首轮 Python 变量重绑定语法错误已修正。按 HEAD 对照确认新增失效链接为0（既有148个失效目标保留），本次迁移源码链接均有效。

- 迁移后的 Application 全库 1164 passed／1 skipped（3.833秒）。取消、目录发布／回滚、原始材料与旧测试均使用唯一计划入口；未增加机械映射测试。观测清单按官方脚本重生，Rust style 与 repository preflight 通过；workspace 全目标检查仍在运行，不提前写通过。

- 最终 workspace 全目标 check、metadata JSON、fmt、Rust style、repository preflight 和差异检查通过。额外直接对照 HEAD 的旧物化算法与新入口解构之后的完整文件后缀，确认算法正文逐字相同；没有借迁移改变部分失败／取消语义。已手工审查模型与 port 拆分、旧入口删除、全部调用路径、公开 facade 导出和历史文档链接；code-review skill 仍不可用。完整 spec 继续。

- 续接 024cda99 为 progress。本轮完整接收动作在 attempt 可见前登记，监督者在实际流程结束后发布完成事实；取消先封该动作，再停止已有子获取并等待整个动作，终态由权威账本读取。子命令继承计划 token，慢目标预留后不再启动新下载；提交 CAS 若取消先胜，则转为保留成功项的部分取消提交。首轮 uc-application all-targets check 通过（17.29秒）；新增慢预留保留成功文件与后续封口、独立尝试、失败／panic来源测试，仍待验证。

- 首轮 Application 回归发现共享完整 ApplyInboundError 会遮住调用方 Internal source 的原 Display（237通过后失败1，原日志保留）。改为仅共享下层 SourceError 的透明包装，保留调用方原变体、即时 Display 与完整来源，同时供计划取消等待读取同一失败；不通过改断言掩盖回归。

- 第二轮回归定位透明共享包装会跳过原始 IO root：240通过后原根来源断言失败。改用显式 Error::source 返回原 SourceError.as_ref，避免 thiserror transparent 跳过原根；无登记计划的普通处理保持原错误对象不变。保留 receive-work-fixed-application.log，未更改原断言。

- 原错误来源修复后 Application 全库1170通过／1跳过（3.846秒），含真实慢目标预留下保留首文件、后续不启动，以及取消赢提交CAS后调用方退出仍完成部分提交。复核补齐父计划取消导致的晚登记获取会话结算；取消等待主流程终态后仍结算活动投影，但不再清理／NoEntry 覆盖已保存的部分结果。新增相关完整取消负责人回归，正在最后验证。

- 最终候选 Application1172 passed／1 skipped（3.845秒），包含计划查表前结束的竞争回归。新的第二次权威读取让慢读测试显式释放两次读取，而不是减少必要检查；过载接受数仍68，未增加隐藏请求。Engine公开契约、workspace全目标与根检查正在执行，未宣称完整spec完成。

- 最终 Engine公开契约50 passed（0.017秒）、workspace全目标check（1m17s）、metadata JSON解析、fmt、Rust style、repository preflight、官方观测清单、变更Markdown新增失效链接0和diff检查通过。已手工复核完整动作持有、原任务在查表前结束、计划取消的晚子会话、失败双等待者来源和成功产物所有权；code-review skill不可用，不冒充技能审查。当前无编译／网络矩阵任务在运行，未重跑全workspace测试或设备／网络最终矩阵。仍须独立文件逐项失败继续／单项取消聚合、生产recipe与选项重试及绑定、所有退出入口／许可完整结算、旧入口删除和最终验收，目标保持完整未完成。

- 续接 c96202f9 为 progress。本轮独立文件逐项失败／单项取消继续，使用唯一项会话和原位置ID；计划取消仍封剩余项，目录仍整单元失败回滚。完整动作持久结果与所有子项事实汇总后才交接远端终态，保留200ms进度节流，不新增线上状态。必要会话complete失败不再只日志而返回成功。批次首轮check通过（16.62秒）；扩展逐项来源后漏初始化Item.source已定位修复；首轮完整App1172通过／1跳过（3.815秒），新增多项失败／取消／成功和真实通知聚合验证继续。

- 新增完整混合文件回归和真实有界通知Actor验证后，App1175 passed／1 skipped（3.816秒）：含同名三项失败／单项取消／最后成功不跳项，非连续成功的本机元数据与摘要准确；保存失败不发Completed、最后成功不覆盖失败、整批进度5Hz与终态一次。复核把失败项实际已验证字节保留在聚合高水位，不因输出失败倒退传输进度；继续根门禁与现有Engine契约。

- 最终候选App1175通过／1跳过（3.815秒），Engine公开契约50通过（0.025秒）。一条文档搜索命令遇到zsh空glob，无文件改动，已改为rg文件索引确认产品spec目录没有对应文件；行为结论维护在现有稳定设计文档和active plan，不新建重复入口。完整workspace全目标和架构门禁正在完成。

- 最终metadata JSON解析、workspace全目标check（1m18s）、fmt、Rust style、repository preflight、官方观测清单重生（1369处）、变更链接新增失效0和diff检查通过。手工审查独立会话身份、混合结果优先级、非连续成功元数据、来源保留、保存之后交接终态、聚合进度高水位与Actor关闭；code-review skill不可用。无活动Cargo／矩阵进程；未重跑完整workspace测试或最终网络／设备矩阵。目标继续，稳定逐项查询、原资料保留、选项重试CAS／绑定、生命周期与许可完整结算、旧入口删除和最终验收仍未完成。

- 续接08fe5805为progress。本轮先贯通既有持久进度的逐项查询：Core定义纯业务项／固定原因，Core规则按attempt权威汇总目录原单元；SQL只读已有身份／类型／状态／原因枚举，不解密或返回名称、路径、ticket。Engine扩展原查询结果items，原汇总字段保持原成员计数含义。首次workspace全目标check通过（1m43s）。
- 类型资料由实际获取上下文传入既有item_role字段，删除SQL解析Application命名约定的初稿；历史缺少类型返回unknown，不假称文件。同步迁移既有内部／兼容调用。第二轮workspace检查仍在执行；本轮还没有实现原始重试资料保留或RetryEntryReceive，不将持久查询子步骤冒充完整重试。

- 新增真实SQLite回归13项全部通过（0.179秒），含类型资料保留、损坏原因只回unknown、查询不含名称／路径／设备元数据，以及目录成员全完成但attempt未结算时仍不可报告目录完成。补充UniFFI／N-API查询入口和原结果映射，N-API字节数沿宿主整数串约定避免JS精度损失；固定词表通过Engine Display复用wire_enum同一来源。正在验证两绑定与Core／App回归。

- Core＋App全lib1508通过／1跳过（3.836秒）。查询的原Facade逐attempt循环已移入transfer/receive/query完整负责人，Facade仅交接完整get/list动作；尚待合并内存完整计划事实与原资料。曾重复轮询已结束的39045句柄返回Unknown process id；该句柄此前已确认exit0，没有重启任务，当前验证只等待实际新句柄。

- Engine公开契约首轮48/50执行（47通过／1失败）为测试新增非空items后，列表比较的显式期望仍为空集合；已将显式期望同步为真实非空项，保留原逐字段比较与旧JSON缺省兼容断言，没有弱化为只比数量。receive-items-contract.log保留首轮失败，继续绑定与最终检查。

- 最终Engine公开契约50通过（0.017秒），两绑定lib63通过（0.218秒），SQLite契约13通过（0.225秒）；workspace全目标check通过（1m21s）。当前持久项查询和绑定查询作为完整重试的一部分交付，不将其当作完整业务项结果；真实owner结算／原资料和未启动项合并、RetryEntryReceive、取消绑定与生命周期／许可／最终矩阵仍待继续。实际平台设备未执行，不记为通过。

- 上一目标轮仅核实并回答当前问题，归为 no progress；本轮重新核对实际差异与最终日志，确认查询切片 Engine 契约50、绑定63、SQLite13及workspace全目标检查通过。LAN facade 未按feature隐藏，默认workspace检查已覆盖本轮新增字段。手工审查类型来源、固定错误词表、旧JSON兼容、绑定字节精度、查询负责人及既有持久字段更新；code-review skill不可用，不冒充技能审查。完整逐项最终结果／重试仍未完成，保持原目标。

- 查询与角色／绑定切片已提交683ce622。后续目录表示错误来源回归首轮因测试导入私有未导出的helper编译失败，改为从实际错误链判断取消；随后原实现真实失败于原始类型不可达（directory-source-red-fixed.log）。修复目录表示失败／取消直接带原始来源返回，目录获取和发布均不启动，两个定向回归通过（0.030秒）。
- 取消绑定切片：UniFFI与N-API各增加整attempt和单项用户取消入口，分别只调用一个现有Engine操作，单项固定LocalUser；输出由Engine wire_enum的Display派生，保留cancellation_incomplete／not_inflight等区别。不增加业务步骤查询，实际清理和重试仍归原receive负责人。正在验证当前候选，不提前记为通过。

- 最终候选取消绑定lib64通过（0.217秒），Application全库1176通过／1跳过（3.891秒），Engine公开契约50通过（0.018秒），workspace全目标check通过（1m26s）。metadata JSON已解析，fmt、Rust style、repository preflight、官方观测1369站点清单、变更Markdown无新增失效链接及diff检查通过。手工审查绑定只交接完整操作、LocalUser固定输入、五秒未完成不归并及目录原始来源／无发布边界；code-review skill不可用。未执行完整workspace测试、网络或真机矩阵，不标通过；原profile诊断失败仍待最终结案。

- 上轮提交683ce622与15f72823并通过实际检查，归为progress。续接工作区仅用户四个原有规划目录。本轮先实现原始来源保留必须依赖的运行世代：readiness共享完整工作registry，关闭封旧世代，恢复只改变原世代，旧超时扫描识别完整接收动作；未把原资料保留或重试标为完成。
- 首轮Application all-targets check通过（26.41秒），世代／完整动作定向6通过（0.032秒）；加入真实接收身份读取竞争后Application1180通过／1跳过（3.859秒）。复核补充权威读取后停止检查，以及旧恢复失败不得影响新世代的回归；前一轮1180结果只属于此前候选，正在运行最终候选检查。

- 补充复查识别旧扫描初筛到清理之间的新接收登记竞态，增加按条目清理保护，与新登记在同一锁内互斥，跨异步清理持有；登记冲突保留ReceiveRegistrationError::CleanupInProgress类型和固定分类。内部register消费者统一迁移Result，转换脚本曾生成is_err().is_ok()，在编译前检查发现并修正，未视为通过。当前最终候选Application1182通过／1跳过（3.777秒），Engine公开契约50通过（0.018秒）；上一次1181和workspace1m19只对应增加清理保护之前的候选。

- 清理保护后的最终Application1182通过／1跳过（3.777秒），Engine公开契约50通过（0.018秒），workspace全目标check通过（1m18s）。metadata JSON、fmt、Rust style、repository preflight、官方1369站点清单、变更链接及diff检查通过。手工审查同锁世代封口、旧恢复不可控制新世代、生产共享registry、异步清理保护寿命、typed登记冲突及全部消费者迁移；code-review skill不可用。无新持久格式或Engine内部查询。未执行完整workspace测试／网络／设备矩阵，保留profile基线失败；原始资料、最终结果合并、选择重试与剩余sweep迁移继续。

- 上轮1541a25b实现运行世代／清理保护并验证，归为progress。当前工作区四个用户原有规划目录保留。本轮实际接线来源目录、完整项查询合并和删除封口：新源码最初导入插在模块内文档之前导致fmt／check失败，另有routes.drain借用和旧_source测试字段，均已修复；receive-catalog-check.log保留失败，fixed check通过（20.50秒）。新增集成测试首轮缺ApplyInboundInput两个必填字段，修复后8项执行6通过2通知超时；改为receipt弱持batch和最终事实，receive-catalog-tests-settled.log最终8通过（0.033秒）。中间脚本一次未找到函数文本，只改完删除方法，随后按实际文本完成Batch快照改造，未把部分执行当作完成。
- Rust style 首轮拒绝两种删除port纯转调，已让各完整入口直接持删除保护到实际结果，共享内部删除当前event的规则，未加豁免或绕过门禁；后续style通过。完整Application、契约和workspace检查正在运行，不提前记录通过。实际RetryEntryReceive与源字段消费仍未实现。

- 首轮完整App1188通过／1跳过（3.782秒）、Engine契约50通过（0.019秒）、workspace全目标check1m17s通过，均属于底层删除装饰器候选。手工复查发现原HistoryDeleteUC在进入装饰器之前读了旧event／文件引用，可能漏清理停止期间新保存的受管文件。因此删除该装饰器，把封口／等待迁入完整DeleteUC最前，并让clear、cleanup、reconcile和retention共用同一Arc；不保留两套删除管线。迁移首轮check发现cleanup另有两个readonly表示查询和测试anyhow source引用需保留／修正，失败日志保留。增加真实目录的最新event引用、受管文件删除和用户文件保留检查，当前新候选App1188通过／1跳过（3.885秒），最终契约／workspace门禁仍在执行。

- 最终owner候选App1188通过／1跳过（3.885秒）、Engine50通过（0.019秒）。workspace检查因mbx检测build-script声明输入在编译期间变化而丢弃observability_probe／local_capture_policy编译结果，exit101（receive-catalog-owner-workspace.log）；非Rust诊断错误，不记通过。当前target可用，磁盘889GiB可用，无已知本轮Cargo活动句柄，冻结文件后在相同target／wrapper上原命令重跑，不清缓存。此前删除的adapter路径预测未恢复仅为mbx warning，已记录实际源码删除，不恢复废弃入口。

- 冻结源码与文档后原target／mbx wrapper的workspace全目标check重跑通过（1m21s），metadata JSON已解析、fmt、Rust style、repository preflight、官方1369站点清单、变更Markdown无新增失效目标及diff检查通过。最终业务源码App1188通过／1跳过（3.885秒）、Engine50通过（0.019秒）。手工审查来源只内存保留／保存释放原包、receipt弱持通知寿命及实际保存证明、原项身份单一来源、删除封旧birth、停止后读取最终event／文件引用、全部维护入口共用一个删除UC、错误来源与用户输出所有权；code-review skill不可用。来源字段仍待真正重试消费，NoEntry来源寿命／容量、清空未枚举pending、删除调用等待退出后的完整责任及无runtime来源重建等缺口均保留。未执行完整workspace测试／网络／设备矩阵，不假报通过；完整目标继续。

- 上轮6f4e4bfd形成生产来源目录／查询与唯一删除负责人并完成实际验证，归为progress。当前保留四个用户原有规划目录。选择计划先加接缝做red：只允许获取原始第3项时旧代码实际请求member:0、准备写same.bin，真实失败证据在receive-selection-red.log（0通过1失败）。实现选择／复用及逐项完成证明后首轮5通过（0.033秒）；扩展混合表示、修改／缺失／越界所有权、目录子集拒绝后8通过（0.037秒）。
- 新固定分类ErrorClass接口必须适配按值生成词表，最初self.wire写法被纯转调检查拒绝；改为明确的Self::wire(*self)类型适配，未改检查器或加豁免。全部调用者迁移completed事实后，完整Application1195通过／1跳过（3.828秒），包含取消选中项保留旧成功、符号链接拒绝和逐项保存证明；稳定Retry API仍未实现。最终契约／workspace检查待运行。

- 最终候选Application1195通过／1跳过（3.828秒）、Engine公开契约50通过（0.015秒）、workspace全目标check通过（1m24s）。metadata JSON已解析，fmt、Rust style、repository preflight、官方1369站点清单、变更Markdown无新增失效目标及diff检查通过。手工审查原数组／项身份、复用内容与归属验证、旧产物不进入新清理、实际保存后逐项证明、全部内部消费者及不变的持久格式；code-review skill不可用。未执行完整workspace测试／网络／设备矩阵，原profile基线失败保留。实际RetryEntryReceive、CAS与绑定尚未接线，完整目标继续。

- 上一状态答复仅报告现状，归为no progress。续接已实际增加retry负责人、原位置快照重建、同条目防重入、旧清理检查、成员校验和共享ApplyInbound重试分支，生产组装与Application facade已接线。首轮check因私有membership路径失败，改用公开再导出；第二轮句柄5547因会话中断丢失，ps确认无对应活动进程，日志无最终结果，不记通过。当前实际句柄58372运行生产组装候选check；尚无新测试或稳定Engine／绑定，不宣称重试完成。

- 实际手动重试首轮共享接收回归5通过（含既有同名筛选1项），完整负责人扩展9通过（0.02秒）；生产API初候选workspace全目标check通过（1m25s）。随后Application1204通过／1跳过（25.66秒）、Engine公开契约51通过。补齐最终完整成功释放来源、继承项最终投影、固定未选取消结果、接受前归属／内容校验、原清理最后复核和薄绑定映射；复核把最终CAS校验归回transfer/retry，并以不透明关联交接后台、Application单点记录失败。一次按旧fmt文本定位scope的脚本在首次写入前失败，改精确patch完成；当前候选仍待最终完整验证，未把此前结果冒充当前通过。

- 最终核心行为候选App1206通过／1跳过（25.91秒），包括可控身份读取期间插入清理／关闭，不建新attempt且返回固定拒绝结果。之前绑定47＋19与Engine公开契约51通过；公开错误Debug已按Display脱敏，Lost原始类型仍可追溯。前一次workspace全目标通过（1m25s）、metadata JSON已解析。新增Markdown链接检查初次受Git i/w前缀影响计数为0，已显式固定a/b前缀重验4个新增目标、缺失0；未将零覆盖当作有效验证。当前冻结源码／文档准备最终根检查，完整spec仍未完成。

- 最终当前候选Engine公开契约51通过（0.00秒）、绑定47（0.64秒）＋19（0.00秒），App1206通过／1跳过（25.91秒）。最后workspace全目标check通过（1m30s），metadata JSON解析、fmt、Rust style、repository preflight、官方1371站点观测隐私清单与diff检查通过；新增Markdown目标4个无缺失。已手工审查源次序／证明、CAS与最终拒绝、单集合到实际保存结束、公共错误脱敏／原链及完整动作绑定；code-review skill不可用。几次只读路径盘点曾猜中不存在的glob／文件，后续用rg文件索引定位；生产组装首次还漏EntryId导入，已集中引入并重验，失败日志保留。无运行中的本轮Cargo／网络矩阵任务；没有清缓存／创建独立target。完整workspace、真实SQLite／Engine及最终网络／真机矩阵本轮未执行，profile基线未结案；目标完整未完成。准备提交本轮实际重试切片。

- 上轮d48e8497实际手动重试接线并完成验证，归为progress。当前开始补齐重启查询证明：新Application专属BeginReceiveRetryPort在一个事务完成旧attempt CAS与全部新项初态，复用／未选项不登记新清理路径或假下载事件。目录在同一事务记录空根映射的Staging头，既有真实目标准备随后覆写，不增schema／格式。将使用真实SQLite验证刚接受后的重开查询、失败回滚、旧CAS冲突、密文和旧成功文件不被清理。

- 当前真实SQLite完整库259通过／3跳过（20.16秒），包括V3材料与目录封装、错误输入不改权威和插入失败回滚。App1207通过／1跳过、Core335通过；新增保存阻塞期间查询断言1通过（0.00秒）。已保留两次账本红测及取消前项漏继承红测，不放宽原断言。准备冻结源码与文档运行跨层门禁；新下载真正保存的持久证明和完整Engine矩阵仍未完成。

- 本轮最终workspace全目标check通过（首轮1m33s；去掉新增测试的重复import后12.29s），metadata JSON解析812个包、fmt、Rust style、repository preflight、官方1371站点隐私清单及diff检查通过。真实Storage259通过／3跳过、App1207通过／1跳过、Core335通过，保存阻塞查询新增断言单独通过。手工复核CAS同事务、加密与原清理所有权、目录空根头覆写和父取消遍历；code-review skill不可用。完整workspace／真实Engine／最终网络设备矩阵未执行，完整目标继续。下一步同步main的暂停释放资源修复，再补齐实际保存的持久逐项证明。

- 8fd56c5b提交完成项初态持久化。重新fetch发现origin/main已推进至a8753ff5，合并提交5a3ae7b9同时纳入#224暂停文件释放与#229取消每传输zstd工作池；无冲突，四个用户规划目录保留。恢复本任务planning指针。当前补齐实际保存事实：仍由唯一接收流程交给原子capture提交候选项，提交与逐项交付标记同事务；保存失败／NoEntry不能把裸下载Completed在重启后冒充交付。先做真实SQLite红测，再扩大到实际capture提交契约。

- 当前保存证明切片：真实NoEntry红测0通过1失败；修复后1通过。提交／加密／目录10项通过；完整App1207通过／1跳过（25.83秒）、Core335通过（0.41秒）、Storage260通过／3跳过（18.03秒）。正在串行运行既有attempt／artifact契约、Engine公开契约及合并main后的真实暂停文件释放验收；尚未将这些检查标为通过。

- 合并main后的既有attempt／artifact契约13＋2通过，Engine公开契约51通过；真实UniFFI暂停文件释放2通过（9.68秒），验证暂停SQLite／日志资源释放和恢复，不能替代活动接收的所有生命周期矩阵。内容处理库79通过（0.35秒），包括合入main的压缩回归。官方观测清单重生为1373站点，fmt/style/diff通过，开始冻结Rust／文档／inventory进行最终workspace、metadata与repository门禁。

- 本轮最终workspace全目标check通过（1m32s），metadata JSON解析812个包、fmt、Rust style、repository preflight、官方1373站点隐私清单及diff检查通过。最终相关回归App1207／Core335／Storage260／Content79／原子提交10／attempt和artifact15／Engine51／真实绑定暂停2项通过，4项跳过；合计1959通过。手工审查候选项原位置映射、密文装饰器、原子标记回滚和NoEntry隔离；code-review skill缺失，未冒充技能审查。源码／文档／inventory冻结期间未改动输入，未清缓存或新增target。完整workspace测试、旧profile基线结案、最终网络／1GiB／真机矩阵仍未完成。准备提交保存证明切片，完整目标保持未完成；继续历史成果／来源恢复、普通redelivery衔接、许可／关闭／删除／撤权与旧入口删除。

- 上轮47d6bf43完成真实原子保存证明并提交，归为progress，当前仍仅四个用户规划目录未跟踪。新盘点确认普通redelivery不能无条件按fresh数组索引继承原成果，须核验原表示身份；原Downloader／warmup／shutdown_endpoint仍留在生产adapter，调用只剩测试和测试装饰器。本轮先完成唯一下载路径收口：Application transfer继续拥有完整网络获取和停止；Core旧port只保留发布／引用／票据解释，生产adapter移除自拨／重试／对端关闭；真实本地读写测试迁至专属ReadStore，旧SDK比较仅保留在明确历史测试内。验证所有消费者迁移、真实Uni提供／获取与原协议兼容，随后回到原成果衔接。

- 生产旧下载实现和Core trait方法删除，App缓存fake与Engine发布装饰器全部迁移；真实Uni／本地导出19通过（0.15秒），同连接停止隔离与旧客户端兼容2通过（0.20秒）。完整App1207通过／1跳过（25.96秒）、Core335通过（0.38秒）。repository当前源码与五个新增反例通过。初次P2P check有4处旧Digest断言未迁移到export字节返回，修复并保留；共享fixture拆分漏ConnectivityError也已修复。当前真实P2P整库／协议向量／SDK结算探针／1GiB历史基线运行中，未提前记通过。

- 扩展P2P整库得到320通过／2失败／3跳过（162.35秒），因此后续向量／故障／资源测试未执行。失败不是文件名泄露：SDK metadata actor在另一个subscriber下创建事务span，父ID来自请求subscriber，导致clone不存在ID、RPC sender关闭；节点关闭也在异步actor析构时向错误registry释放parent。完整失败和backtrace已保留。
- diagnosing-bugs最小反馈环已缩至两测试组合：accepts_single_inbound_frame_from_known_peer（安装全局过滤subscriber）＋publish_path_logs_do_not_include_source_path；0.3秒稳定red。单跑后者与node诊断都通过，三测试组合0.36秒2失败。61测试prefix同样复现，两组更小的无全局subscriber前置组合全通过，排除单纯网络负载／顺序时间猜测。
- 单变量试验只把publish_path的SDK future创建与poll放到无业务parent的dispatch，原完成日志仍在外层记录；最小三测试变为2通过1失败，证实SDK跨subscriber的请求parent问题。新增正向日志断言，避免隐私测试日志为空也通过。mDNS actor显式parent None后测试状态3通过，但stderr仍有acto guardian析构panic，不能算真正green；正在对独立discoverer启动同样隔离短期parent。当前handle日志trace-dispatch-probe-discovery-root.log待启动验证，未提交／未宣称收口完成。

- 两个具体修复的最小三测试已3通过（0.27秒）且无panic输出：publish_path的SDK创建／poll隔离请求parent；mDNS actor显式parent None，discoverer的acto启动也不捕获短期parent。只修mDNS外层actor时仍存在被Tokio吞掉的guardian析构panic，即使test status为ok也未当作green；第二层隔离后实际stderr干净。
- SDK隔离已收拢为私有without_request_span闭包能力，并覆盖发布／引用、本地读写、实际交换及SDK初始化，不改错误链／业务日志／协议。全目标check首次暴露helper可见性和ObserveProgress是IntoFuture而非Future，已修为pub(in super::super)与IntoFuture；rustfmt macro空白问题已修。
- 新增真实Node启动span寿命测试node/startup_trace_tests.rs，断言发现仍在运行时启动span已关闭，并总是先停止节点再做结果断言。当前为证明该测试能抓住问题，临时还原third_party/iroh-mdns-address-lookup/src/lib.rs中discoverer.spawn(rt)的NoSubscriber包装（外层actor的parent None保留）。当前handle9643运行startup-span-lifetime-red-fixed.log；得到真实红测后必须恢复dispatcher::with_default(&Dispatch::none(), || discoverer.spawn(rt))，再跑green。未恢复前不能提交或记完成。

- 新真实启动计时测试已red：临时去掉discoverer隔离后0通过1失败（0.01秒），明确断言长期发现actor延长已完成启动请求计时；失败前已正常停止节点。已恢复最终dispatcher隔离并补vendor补丁说明，开始green及跨subscriber组合验证。

- 计时断言原版本把span.enter守卫跨await持有，会让同线程其它任务误继承startup；这是测试自身干扰，之前startup-span-lifetime-red*不能作为真实修复证据。已改为完整bind＋session激活async动作instrument(startup.clone())，只在本动作poll期间进入span；重新临时去掉discoverer隔离，证明正确接缝确实red，随后恢复。原两测试跨subscriber RPC／关闭panic复现独立成立，不受该测试错误影响。Endpoint构造与bind创建／poll现统一隔离，但不再声称旧创建span清单已经证明preset同步创建actor，实际scope以正确instrument回归为准。

- 正确的async instrument计时回归已red（startup-span-instrumented-red.log，0通过1失败，0.01秒），并在完整session激活后取样。open parent树明确为test_session_start→Node.bind→acto guardian→发现sender／receivers，证明不是半流程或enter守卫干扰。已恢复discoverer的NoSubscriber包装，开始最终4测试组合green。旧无效enter版本的red不作证据；后续以本次正确回归及组合无panic为准。

- 正确async instrument回归恢复隔离后4项全通过（startup-span-instrumented-green.log，0.26秒），显式扫描无panicked／FAILED。覆盖全局subscriber前置、真实发布日志捕获（新增完成记录正向断言）、原生连接诊断及完整会话启动span在发现停止前关闭。已开始remove-legacy-final-p2p-regression.log串行整库＋协议向量＋SDK停止结算探针＋1GiB历史基线，使用--no-fail-fast收集独立二进制结果；不能把微型回归或测试状态ok但stderr有panic当作整体green。

- 整库残留1失败已复现到SDK Endpoint关闭的跨subscriber父ID；新增显式双registry隐私回归先red（0.08秒）后green，测试夹具启动与生产Node统一隔离SDK长期驱动。保留领域日志正向捕获和路径隐私断言，不跳过测试、不修改SDK。最终P2P整库323通过／3跳过（167.73秒）、SDK结算1通过／协议向量1通过，日志无panic；首轮322通过／1失败及扩大77通过／1失败backtrace保留。历史1GiB基线1通过（23.74秒，峰值RSS45600KiB），只证明原SDK历史基线，FD／线程／任务未全部回到起点，未当作最终生产资源回收。开始Engine契约／真实取消与最终根检查，完整spec仍未完成。

- 最终Engine公开契约51通过；默认feature尝试真实取消用例只筛到0项，未记通过，补dev-tools后真实active_pull_cancel_settles_real_file_a_without_starting_file_b为1通过（5.94秒）。最终metadata解析812个包、workspace全目标check通过（1m29s）、fmt、Rust style、repository（含五个新反例）、1355站点隐私清单与diff检查全部通过。编译期间冻结源码／文档／inventory，所有Cargo串行复用共享target及缓存。手工审查原本地tag／TryReference、来源链、SDK无请求parent边界、启动计时与两registry回归；code-review skill仍缺失，未冒充技能审查。准备提交本切片；完整spec继续，普通redelivery原成果衔接、历史成果／来源恢复、完整许可／生命周期、最终Engine网络资源及平台矩阵仍未完成。


- 2576a4cf之后继续来源提交边界。真实普通重复投递的新包来自不同发送方、两项和新ticket／表示ID，被partial-kept分支丢弃后原三项recipe却被覆盖；redelivery-recipe-red-fixed.log为0通过1失败（0.00秒），明确来源由原source变为different-sender。首轮测试私有字段编译错误改为完整receiver测试构造，共享原readiness，不扩大生产接口。
- receive/work保留至多一份实际保存旧receipt，只有persisted成功确认才切换；未保存finish恢复原清单／输出所有权并按当前attempt重绑定项身份及原facts。连续两次失败、重复finish、实际保存切换／完整释放、关闭重开和删除后的迟到finish均有覆盖；普通begin也拒绝仍持有条目的旧完整动作，具体CleanupInProgress来源保留。
- 首轮最小2通过，manual_retry／work／batch扩展28通过（0.06秒）。补capture StorageFull分支后的整库1211通过／1失败／1跳过，失败在测试只downcast最外层而忽略既有SharedSource包装；改为沿完整chain核验原StorageFull，保留首次日志。最终App1212通过／1跳过（26.06秒），同一回归继续完整手动选择，只获取旧失败项并保留原成功。1355站点隐私清单与style／diff通过，开始Engine真实取消和最终根检查。
- 此步不宣称新旧两份partial成果已安全合并，也不宣称跨重启来源／历史交付证明恢复完成；完整许可、生命周期及最终生产网络资源／平台矩阵继续保留。

- 最终真实Engine dev-tools取消回归1通过（5.82秒），metadata解析812包、workspace全目标check通过（1m26s）、fmt、Rust style、repository preflight、1355站点隐私清单与diff全部通过。全过程Cargo串行复用共享target和原缓存，验证期间未修改源码／文档／inventory。手工审查旧proof仅来自实际保存receipt、同一原清单重绑定、锁顺序、finish幂等、删除／世代封口及具体错误来源；code-review skill不可用，不冒充技能审查。当前只修复未保存替换的同进程来源保留，未将其当作完整新旧partial合并或跨重启证明。准备提交，完整目标继续。


- 本轮按spec必需验收推进真实资料矩阵。当前宿主构建成功，原main／rc15冻结宿主SHA逐个与阶段0摘要一致；首轮10组真实文件通过，但协议记录Other，未计为完整实际ALPN证明。添加真实流程断言后以旧候选二进制重现red，原记录保留；封闭契约及native实际握手两红测也保留。
- 既有连接观察者增加固定SDK ALPN→封闭Blob分类；unknown仍Other，未扩大原始协议文本或远程字段，未增加Engine内部接口。封闭编码2通过、真实native握手1通过、实际本地导出1通过、Collector隐私5通过、原会话驱动3通过。修复后的十组真实资料全部通过，逐个核对当前及冻结旧二进制、真实blob协商、旧文件／新双向文件、身份成员历史、两栈UDP阻断与选路、资料副本和明文／清理。十格源码树／工作差异摘要均一致，matrix-summary记录候选5a9+diff与未证明范围，未冒充整体spec。
- 最终P2P整库323通过／3跳过（171.09秒），本轮未修改实际传输／SDK字节或持久格式；继续完整观测crate及根门禁。受管Engine1GiB资源、数量／时间全边界、真实部分重试／生命周期和当前交付head CI／平台矩阵仍未完成。

- 扩大观测crate的普通Cargo all-targets得到180通过／1失败／1跳过；失败是未修改的physical_connection_records同一二进制两测试重复安装全局subscriber，第二项AlreadyInstalled。改用仓库既有nextest逐测试进程隔离后，两项均实际通过；两crate完整nextest共181通过／1跳过（6.553秒），不改变断言或生产singleton语义，首轮失败日志保留。根门禁继续串行执行。

- 最终metadata解析812包、workspace全目标check通过（1m28s）、fmt、Rust style、repository preflight、1355站点隐私清单及diff全部通过。P2P323通过／3跳过、观测两个crate nextest181通过／1跳过；Collector隐私5和原驱动3通过。真实文件十格含实际blob握手全通过，原冻结库和当前二进制／代码快照指纹已逐格核验。Cargo串行共享target及原缓存，未手动清缓存或使用临时target。手工审查固定ALPN映射、封闭词表、未知拒绝、本地／远程隔离及旧角色复用；code-review skill不可用，不冒充技能审查。准备提交；完整spec未完成，后续优先真实Engine／SQLite部分重试和生命周期、完整准备／结算预算、受管1GiB及默认CI／平台验收。


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

- 最终三crate nextest启用Engine dev-tools及no-fail-fast：1773通过／9跳过（156.325秒），首次失败的真实Engine二次删除与所有原未运行项均实际执行通过；三项新增真实重试／取消／撤权同样通过。Application独立整库1214通过／1跳过（25.83秒）。
- 最终metadata解析812包、workspace全目标check（1m25s）、fmt、Rust style、repository preflight、1355站点隐私清单及diff均通过。Cargo串行共享target及原缓存。最终编译与测试期间固定代码／文档／inventory；JSON元数据在结束后改为忽略的log工件，未纳入提交。
- 手工审查替换事务FK与回滚顺序、表示身份及加密AAD、清理原始source与双失败保留、create_new所有权、取消屏障及锁顺序、二次删除typed结果、仅测试组合根故障注入。code-review skill不可用，不冒充技能审查。准备提交本轮真实Engine验收发现的修复；完整spec保持进行中。下一步NoEntry持久清理／删除调用者退出监督、完整预算与其余生命周期／历史来源／1GiB／最终CI矩阵。

- NoEntry删除红测no-entry-delete-red.log明确调用旧DeleteClipboardEntryPort，任务panic而未走原子清理。完整port改为仅接受EntryId并返回实际删除bool，旧event参数及DeleteUC的可选原子port／分步fallback全部删除。Facade与所有内部测试同步迁移，无新Engine接口或schema。持久事实决定Missing，不再保留ReceiveCleanup.had_receive。
- NoEntry真实SQLite六项初轮通过；扩展目录未发布日志场景得到no-entry-publish-delete-red.log红测，原实现错误返回true并丢弃发布恢复记录。Core新增完整删除准入规则，Infra在事务内读取事实；未发布目录逐attempt匹配Settled+RolledBack证明，拒绝错attempt、Pending及未完成phase。
- 读取表示失败原先unwrap_or_default，现保留具体source并不删除记录。Application四项定向测试通过（0.00秒）；真实Engine在实际导出完成权屏障内丢弃删除等待后仍停止重试，移除条目／所有接收状态／受管文件，二次NotFound通过（3.53秒）。既有Engine operation监督真实有效，不另建Application任务表。
- 编译首轮新SQLitecount缺QueryDsl（5处E0599），改用原测试已有count_star；引入的EventId未使用警告已清理。六项SQLite／Application3项初步绿灯与两个真实红测均保留，不把旧快照当最终全通过。准备冻结代码、扩大Core／App／Storage／Engine及实际profile commit集成验证和根门禁。
- 额外源码证据：DecryptingClipboardRepresentationRepository列表接口仍会在inline解密失败时返回原密文字节；该历史降级没有由本次get_representations错误回归证明安全，后续应沿已存在的认证文件引用能力核对删除／清理失败边界。本轮不冒称已修复该问题。完整预算、clear／更多生命周期、历史来源、1GiB与最终CI仍保留。

- 最终扩大编译首次因新布尔查询缺BoolExpressionMethods得到E0599，no-entry-final-nextest.log保留且未计为测试通过；补齐trait后Core／Application／Storage／Engine完整nextest（dev-tools、no-fail-fast）2114通过／9跳过（149.896秒）。Core删除规则、NoEntry及目录回滚证明SQLite、真实Engine调用者放弃删除等待均在同一轮实际执行通过。
- Profile inbound_receive_commit_contract六项及encrypting_inbound_receive_commit一项，共七项实际通过；包含NoEntry取消结算、事务原子性、保存证明重开和删除清理。最后metadata解析812包、workspace全目标check（1m31s）、fmt、Rust style、repository preflight、1352站点隐私清单和diff均通过。Cargo串行共享target及原缓存，无临时target或清缓存旁路。
- 手工差异审查Core纯删除规则／Infra事务边界、同attempt回滚证明、所有状态表回滚及真实Missing、必需依赖／旧fallback删除、读取失败source、Engine原监督及受管／用户文件边界；code-review skill不可用，不冒称技能审查。完整spec仍未完成，准备提交本轮。后续clear的未保存接收、认证文件资料读取、更多失败／关闭责任、完整预算、历史来源、大文件资源与最终CI／平台矩阵继续。

- 首个真实clear红测因首项导出暂停导致原发送ACK尚未完成，report.accepted为0（clear-no-entry-red.log），修正fixture检查accepted+pending=1；实际收件IncomingPending／SDK导出及无历史条目仍由真实入口证明。随后clear-no-entry-red-fixed.log精确复现ClearHistory在无保存条目时提前返回。
- 生存登记改为出生动作独立子取消／完成，不改变外部会话代次。ClearScope先同时停止出生动作，等待未登记准备退出或已登记动作明确完成；注册完成丢失不是成功，多失败保存全部typed来源，失败后封口。专属目标集SQL UNION只读标识，合并历史／attempt／条目关联的file_transfer／产物／目录日志；不新增schema。原逐步caller编排及错误字符串化移除。
- 真实Engine基本clear8.38秒通过；扩大五项Engine32.69秒通过，含清空后接收新内容。Registry相关17通过（0.05秒）、实际初始readiness准备停止1通过（0.02秒）、真实SQLite目标集1通过（0.07秒）。
- 审查发现readiness.is_ready临时false会启动reconciliation与清空并行；clear-recovery-red.log确证恢复在清空内立即运行。改用共享维护许可，清空先封口／停止再等已有恢复；恢复重新检查ready，不与已接受clear并行。通知也由Registry唯一持有，clear结束能唤醒readiness等待。双向竞争及receive相关58通过（0.11秒），clear相关15通过（0.08秒），待最终整库／根门禁。
- 尚未绑定entry的provisional及更早输入／出站准备不由本轮出生表证明覆盖，留在全生命周期与准备预算验收中；既有认证资料读取／删除缓存失败边界仍未修复。完整目标不缩减，不把已关联条目清空当全spec完成。

- 清空切片最终Core／Application／Storage／Engine完整nextest 2124通过／9跳过（199.518秒）；metadata解析812包、workspace全目标check（1m24s）、fmt、Rust style、repository preflight、1351站点隐私清单及diff均通过。既有编译警告未记为新通过项，设备未执行。手工审查共享维护许可与锁顺序、取消／出生／完成权、失败封口、SQL目标集及真实Engine调用者退出；code-review skill缺失，不冒称技能审查。下一步继续认证资料读取及删除清理失败边界，完整spec仍在进行。

- 认证读取切片先由authenticated-list-red.log复现两个密码失败仍交付原密文，再由authenticated-delete-red.log复现真实V3引用密文破坏后DeleteHistoryEntry仍history_entry_deleted（4.251秒）。移除旧降级，整批只交付认证结果；聚合错误保留每项BlobCipherError，窄port的Storage转换仍可追溯原始具体类型与source；诊断只聚合数量。
- 定向内容整库及新增真实Engine共81通过（4.438秒）；Engine同时证明单删／清空拒绝、原接收attempt／受管文件／原密文记录保留，以及恢复原密文后通过原清空动作成功完成。最终Core／Application／Storage／Content／Engine完整nextest 2205通过／9跳过（168.796秒），metadata解析812包、workspace全目标check（57.31秒）、fmt、Rust style、repository preflight、1351站点隐私清单及diff全部通过。未执行设备仍跳过。
- 手工审查批量完整性、所有密码来源保留、窄port转换、固定日志、真实SQLite破坏／恢复及失败后重试；code-review skill缺失，不冒称技能审查。无新schema／格式或Engine步骤接口。删除缓存仍在SQL后best-effort，路径仅词法前缀，磁盘失败及安全所有权尚须继续；本轮不把认证读取当作完整删除／生命周期或完整spec通过。

- 缓存归属切片：实际原始路径cache/../user红测确认用户文件已被误删（delete-cache-ownership-red.log）。删除前新增完整引用验证，拒绝父级逃逸、缓存内任意符号链接及根自身；校验全部候选后才进入原子删除，使用解析后的根及目标进行既有清理。混合正常／危险引用不改变任何文件或记录，配置根合法别名不被误删。
- 首次风格标签因rustfmt隔开而未通过，已修正。审查发现非本机合法file URI被新拒绝误伤，首个扩大运行在编译阶段主动SIGINT终态130，确认编译子进程退出，未记通过；foreign-uri-red精确复现后，合法非本机引用继续保留外部所有权，UTF-8及URL解析失败保留原始具体source。最终七项定向删除回归通过（0.051秒），包含既有缺失缓存目标、外部文件／远端URI、原始解析source与根别名。
- 最终Application／Engine完整nextest 1527通过／6跳过（173.360秒）；真实Engine部分重试、取消、删除／清空、认证失败恢复与崩溃重启均实际执行。metadata解析812包、workspace全目标check（1m26s）、fmt、Rust style、repository preflight、1350站点隐私清单及diff全部通过。手工审查所有候选验证先于SQL、同源错误、用户路径保留、根别名、缺失及解析边界；code-review skill缺失，不冒称技能审查，设备未运行仍跳过。
- 本轮不证明路径操作已对抗本机并发文件系统替换，SQL后缓存remove_file仍为既有best-effort，历史清理恢复边界保留。下一步优先回到spec核心的完整准备／结算预算与生命周期验收；不以继续修旧缓存行为替代完整spec。静态证据：BlobDownloadResources::obtain返回的DownloadedBlob._permit在FetchBlobUseCase::execute*_inner局部持有，输出不携带同一guard，须用实际上层保存／清理屏障验证其余工作是否仍受同一额度约束。

- 完整Facade本地进度屏障先实证同端2项尚未结算就允许第三项开始（download-local-settlement-red.log，0.035秒）。同一DownloadPermit由receive/download_work的不透明持有者交给底层工作、Facade及本项本地交接共同持有，不再次排队或创建父批次额度；表示赋值先完成再记录item_result。测试覆盖全局4／每端2、仅Facade／带父交接、7／9项顺序处理及原StorageFull链。
- 只保留额度还不够：download-local-protection-red.log两项均失败，保护计数为0而预期2（0.033秒）。现在同时保留原BlobRetainedContent，原生实现内含TempTag与StreamWorkLease；最后持有者先释放内容／本地工作，再交回同一额度。无Any／downcast、新SDK类型、Engine查询、第二队列／池或schema。相关transfer／receive 192通过（2.169秒）。
- 初步完整Application／Engine1529通过／6跳过（193.718秒）之后，缓存关闭屏障又实证shutdown在本地工作／额度仍被持有时完整返回（download-local-shutdown-red.log，0.088秒）。关闭现在等待原slot真正全部交回后才关闭存储。五秒只结束调用等待、保护仍为1；重复Complete复用同一协调者、释放后才真正关闭。相关192项通过（5.509秒）。失败分支最初错误预期shutdown成功，已改为验证既有Store及原StorageFull，不改弱生产策略。
- 最终同一源快照Application／Engine完整nextest 1529通过／6跳过（178.054秒），含实际三文件重试、取消／删除／清空、认证失败恢复、进程崩溃恢复与新额度／保护／五秒关闭屏障。metadata解析812包、workspace全目标check（1m31s）、fmt、Rust style、repository preflight、1350站点隐私清单及diff全部通过。Cargo串行共享target及原缓存，未执行设备仍跳过。
- 手工审查许可唯一性及克隆所有权、内容与额度释放顺序、字段可见性、batch逐项释放避免死锁、错误来源不含持有者、通知／锁顺序、关闭quiescence及真实Engine回归；code-review skill缺失，不冒称技能审查。完整spec仍进行：准备阶段admission／60秒、目录发布／最终保存、inflight完整取消及清理失败移交仍需实际屏障证明，不把本轮当完整预算或生命周期完成。


- Facade初始本地进度红测实证准备在准入前启动；现由原DownloadTask持有准备future，取得原4／2许可后才开始lifecycle／初始进度，同一started_at覆盖准备及身份解析。原子检查登记后再创建任务，重复请求不先触碰旧session；不透明ObservationContext沿原准备future传递，无新Engine接口、池或预算。
- 实际Facade屏障验证全局4／每端2及等待64／16、满额拒绝、重复请求、排队取消、5秒CancellationIncomplete、调用者退出后原准备仍占用额度至真正结束。真实DownloadTask虚拟时钟验证准备59／60秒、排队不消耗下一活跃期限、准备30秒后身份解析仅余30秒；期限已过不启动identity／网络。原StorageFull及关闭source保留。前期相关194通过；最终修改后blob／receive定向82通过（10.166秒）。
- 新增正常准备拒绝回归先因两处私有路径导入编译失败，修正后取得实际红测：LifecycleClosed经统一Settlement污染shutdown（download-preparation-refusal-red.log）。现在按FileTransferApplicationError具体类型区分Closed／Busy／AlreadyCompleted等拒绝与Store／Settlement，保留整个原BlobTransferError；停止写入失败仍为Settlement。实际监督任务及shutdown验证三类正常拒绝不污染关闭，StorageFull仍作为原Store原因由关闭报告，未吞错。
- 第一次完整快照1530通过／1超时／6跳过（184.278秒），engine_start_builds_a_resumable_real_session超过20秒；单独原样复跑8.795秒通过。最终含分类修复的同一源快照Application／Engine完整1533通过／6跳过（189.050秒），该测试11.683秒通过。未改超时配置或测试并发4，首次失败没有记为通过。
- 最终metadata解析812包、workspace全目标check（1m28s）、fmt、Rust style、Engine repository preflight、1350站点隐私清单及diff全部通过。Cargo串行共享target与原缓存。手工审查许可／期限唯一性、登记与启动原子性、取消原因及实际准备结算、source链与关闭分类、观测上下文所有权；code-review skill缺失，不冒称技能审查。设备未执行仍跳过。
- 下一步从materialization/workflow.rs独立文件目标reserve_target／reserve_managed_output及artifact journal入手：它们仍先于fetch排队，须纳入同一下载任务及首批期限，不能创建父子双重许可。目录发布／最终保存、清理失败移交和inflight仍只跟底层worker的边界亦未完成；其余生命周期、历史来源、受管1GiB、业务记录及最终head网络／CI／设备验收保留，完整spec继续。


- 独立文件目标预留实际红测：完整materializer＋真实Facade中，前两同端目标仍阻塞，第三项已经进入ReserveInboundFileTargetPort（materializer-target-admission-red.log，0.017秒）。原目标预留和artifact journal确实发生在fetch排队之前，不以假调度模型替代。测试初次因缺snapshot字段编译失败，补齐后取得该实际红测。
- 文件获取统一改用owned BlobOutputTarget，既有目录固定路径也沿同一契约；独立文件把目标预留、受管create_new及artifact journal交给原DownloadTask，取得原许可后执行，同一started_at计时。输出路径用单次准备回执在成功后交给导出，不新增池、schema或Engine阶段接口。原使用名集合只在managed fallback时更新，保持原命名与碰撞行为。
- FileOutputPreparation先保存实际产物事实，再写原日志。准备失败由typed BlobOutputPreparationError保留原有整计划中止语义及source；已有目标清理失败仍携带独立cleanup与原始原因，未准备的队列拒绝不得pop前一成功产物。测试模拟适配层实际执行owned目标准备，原mock只处理已准备命令，不保留第二套生产实现。
- 首次扩大87通过／1失败、103未运行：旧取消回归发现模拟适配层在预留后漏查取消，错误调用第二次mock获取。补齐与生产负责人一致的前后取消检查，未放宽原断言。最终相关192项全部通过（10.301秒），覆盖实际materializer全局4／每端2、五秒取消未完成且仍占slot、未准备队列不创建文件及取消后的真实清理；直接Facade虚拟钟验证目标准备59／60秒、等待实际完成及StorageFull保留到shutdown。新增实际文件系统日志失败验证旧成功文件不变、目标清理或IsADirectory失败与StorageFull同时保留。
- 最终同一源快照Application／Engine完整nextest 1536通过／6跳过（193.552秒），实际部分重试、导出取消、删除／清空、撤权与进程重启恢复均通过。metadata解析812包、workspace全目标check（1m32s）、fmt、Rust style、repository preflight、1350站点隐私清单及diff全部通过。Cargo串行共享target与原缓存。手工审查future／许可／路径回执所有权、取消后真实准备完成、错误链、准备失败清理、队列无产物及命名行为；code-review skill仍缺失，不冒称技能审查。设备未执行仍跳过。
- 本片仅完成独立文件目标准备的准入／期限覆盖。Facade现仍在底层worker结果返回后移除inflight，再等待最终进度／事件；完整调用被丢弃、父流程清理失败或最终保存时的取消结算需要继续收口。下一步先用最终进度真实屏障证明取消／重复请求仍等待整个动作，再把完整Facade结算归同一原监督任务；目录准备／发布／最终保存、失败产物移交及其余spec硬验收继续，不能把本轮当完整spec或全生命周期通过。


- 最终进度实际红测：Facade完整动作仍阻塞最终本地进度时，重复同项已不再Busy，取消无法接入原动作（download-final-handoff-red.log，0.136秒）。原代码在底层worker返回后移除inflight，再在调用方future内交接进度／终态，等待方退出会失去后续结算。
- FetchBlobUseCase的原监督任务现在接受同一transfer负责人的最终交接，内存／文件统一由FetchFinalization完成；原许可与内容保护持有至交接实际结束，completion代表完整Facade，不新增池或重复许可。inflight保留实际完成状态，新请求统一回收结束项，pending取消协调者也保持排他；协调者只等同一完整结果，不再重复写session终态／通知。attempt取消先向全部子动作保存首因再发停止，既有不透明ObservationContext随原worker覆盖最终交接，无Engine内部查询。
- 成功、失败及取消的终态写入失败均返回真实Settlement来源，原获取失败与独立结算失败完整保留；不再只写warning后吞掉。额外分类红测证明纯最终落库失败此前被公开结果标成Unknown（download-final-classification-red.log，0.029秒）；现先识别既有BlobExchangeError并沿原类别及IO原因分类，StorageFull成为StorageUnavailable，不新增错误字符串解释。
- 首轮Facade10项中9通过／1失败：旧缓存shutdown屏障预期本地交接继续成功，现在完整worker也接受关闭取消，因此返回Cancelled；断言改为明确Cancelled，同时保留五秒ShutdownIncomplete、保护仍存在及实际关闭等待，不弱化停止证明。最终相关195项通过（10.297秒），含真实Facade最终进度下重复Busy／5秒未完成、原调用者退出后重复取消完成、真实ReceiverTransferHandle的Completed／Failed／Cancelled落库失败及原ConnectionReset／StorageFull链、已持久终态与迟到取消竞争、事件发布阻塞时调用方退出仍由原任务持有且shutdown等待实际结束。提交竞争只证明既有文件事件终态，不冒充完整条目保存CAS验收。
- 最终同一源快照Application／Engine1539通过／6跳过（185.854秒），实际三文件部分重试、取消保留旧成功、撤权、删除／清空及进程崩溃恢复均通过。metadata解析812包、workspace全目标check（1m27s）、fmt、Rust style、repository preflight、1348站点隐私清单及diff全部通过。Cargo串行共享target与原缓存；手工审查同一任务／许可所有权、完整完成通知、原子登记及取消首因、终态竞争、source链与类别、关闭等待及opaque观测关联；code-review skill缺失，不冒称技能审查。设备未执行仍跳过。
- 下一步从ReceiveWorkRegistry及ReceiveBatch.finish的实际父流程结算入手：DownloadTask完整Facade结束不等于materializer清理／日志交接或最终保存完成，batch.finish仍会清除剩余local_work。须用真实清理失败／最终保存屏障核对同一许可及内容保护的持有和恢复移交，不能让失败被当作已结算，也不能通过持有全批许可导致7／9文件死锁。目录准备／发布、其余生命周期、历史来源、受管1GiB资源、业务记录及最终head网络／CI／平台矩阵仍未完成。


- 父保存真实流程红测：实际ApplyInbound手动重试接入真实Facade／DownloadScheduler、计数本地内容保护及实际文件输出，在受控capture端口仍阻塞时保护计数已为0，transport也提前完整关闭（receive-parent-capture-red.log，0.016秒）。本测试不冒称真实网络或SQLite提交；其余重试／接收流程、监督与调度为实际实现。
- ReceiveBatch现在把最后一个实际下载的原DownloadSettlement交给commit_handoff，保留至ReceiveAction.finish。未下载的继承／未选结果不覆盖该持有者；下一项local_settlement开始时交还旧持有者，不积累整批许可；已有并发待处理项时照原单项交接释放，保持7／9项顺序推进。不新增许可、池、计时器、schema或Engine步骤查询，使用同一内容保护与permit对象。
- 新增完整重试保存屏障验证：capture阻塞期间同一保护保留1、Bounded shutdown五秒返回ShutdownIncomplete；调用者退出后原ReceiveAction继续实际保存，完成通知及关闭结束后保护归0、旧成功文件仍为success、只保存一次。定向7项通过（10.167秒）；扩大相关196项通过（10.329秒），含既有全局4／每端2、7／9项与取消／准备／最终Facade屏障，无许可积累死锁。
- 最终同一源快照Application／Engine1540通过／6跳过（187.790秒），实际重试、导出取消、删除／清空、撤权及进程恢复均通过。metadata解析812包、workspace全目标check（1m33s）、fmt、Rust style、repository preflight、1348站点隐私清单及diff全部通过。Cargo串行共享target与原缓存；手工审查guard对象及释放顺序、单项到父保存交接、继承项不覆盖、下一项释放与多文件推进、真实关闭等待及不把测试替身冒作网络／SQLite证据。code-review skill缺失，不冒称技能审查；设备未执行仍跳过。
- 本轮不是全预算完成。源码已明确两个剩余缺口：(1) local_settlement先清除旧commit_handoff，随后才解析票据／排队；下一项被拒绝或在取得许可前取消时，父部分保存可能没有原许可，须继续实际回归及设计连续交接；(2) ReceiveAction.finish的错误路径仍清除local_work／commit_handoff，即便OutputCleanupFailure保留未清理产物与非终态attempt，恢复持有移交仍需完成。不能以没有活跃IO或本轮普通保存通过直接宣称已结算。目录准备／发布的全部边界、其余生命周期、历史来源、受管1GiB、业务记录及最终head网络／CI／平台矩阵继续。

### 2026-10-22 完整双流隔离硬验收

新增实际Application完整Facade／自动重试＋生产Uni及SDK双流用例，27对／54流覆盖不同4MiB、32MiB及同内容。不同内容的A、B均有部分真实校验数据后取消A，B精确导出且无重试、重拨或连接关闭。测试引用仓储为内存替身，不宣称SQLite或Relay覆盖。定向7项通过；最终1866通过／9跳过（189.264秒）；metadata812包、workspace全目标check（1m34s）、repository、fmt、Rust style、1348站点观测隐私及diff通过。用户已明确批准逐项保存成功文件、保留4／2预算，规格与计划同步；下一切片从原子持久提交和产物日志移交开始。

### 2026-10-22 逐项进度提交基础切片

实现既有CommitInboundReceivePort的Progress结果、capture映射及加密decorator，保持Receiving。Core按实际产物身份／路径／归属判定唯一移交；Infra在原事务保存条目和交付事实，同时以原密文CAS重封剩余产物，避免恢复误删已交付文件。V1／V3沿原格式和schema。先后修复测试的profile私有类型导入、文件内层doc位置及manual_retry新context穷举分支；最终17项定向通过（0.163秒）、四层库1921通过／4跳过（23.719秒），metadata812包、workspace全目标check（1m33s）、repository、fmt、style、1348站点隐私、diff全部通过。

完整materializer逐项调用尚未接入，不宣称用户选择已端到端完成。后续接线已在findings记录：现有Arc<ReceiveAction>可直接复用；保存后立即更新receipt、只保留pending产物；最终replace本次已经保存的条目，并处理原本已有partial及rapid dedup。尚未实际清理失败的资源／目录完整单元及其余spec继续，设备未运行仍跳过。

### 2026-10-22 完整逐项保存首个接线切片

新接收／手动重试的文件计划在同一ReceiveAction内逐项保存，实际提交前保留原DownloadSettlement，成功后归还同一许可并从pending日志移除。中间快照合并全部继承成功（含尚未遍历位置）；失败只清本项并继续，最终replace本次保存记录。提前快速重复处理同时结算provisional；CAS封口立即取消原action，避免停止通知迟到时启动后项。目录及普通已有partial重投未纳入本次逐项路径，后者原成果映射仍未完成。

新增9文件实际Facade额度／保护与真实文件输出、保存失败后项继续、取消CAS通知竞态用例，保存端明确替身。真实Engine证明第三项仍Receiving时首项已从稳定历史入口可读，后续部分失败及仅失败项重试仍正确。定向18项、最终相关199项通过；Engine两个用例14.000秒通过。首次相关回归暴露测试替身的AlreadyCancelling及Progress状态／replace能力不符合既有port，已修正并保持全部业务断言。最终Application／Engine1544通过／6跳过（183.042秒），metadata812包、workspace全目标check（1m30s）、repository、fmt、style、1348站点隐私及diff全通过。

下一步优先实际单项取消＋阻塞保存及SQLite保存故障红测：目前fetch返回后才调用Progress，许可虽仍持有，Facade自己的inflight／网络会话已经完成；单项取消与保存失败的持久分类尚不能宣称完整。结合原DownloadTask／FetchFinalization和业务提交CAS设计完整责任，禁止新增池、Engine步骤查询或回退终态。普通已有partial来源映射、未清理产物恢复及其余完整spec继续。

交付审查移除保存失败后冗余的空日志写入，避免二次IO失败覆盖原source并提前终止后项；原日志保守覆盖已删除项，下一预留／最终结算更新。强化用例额外拒绝空日志，0.056秒通过；其后同一最终源快照1544通过／6跳过（183.042秒），根门禁重新全部通过，未复用前一次快照结论。
