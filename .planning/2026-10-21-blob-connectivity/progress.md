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


### 2026-10-22 单项取消接入实际本地结算

实际ApplyInbound与Facade在保存屏障下的红测证明：尚未保存时，单项取消已返回NotInflight。现在同一DownloadSettlement提供不持有许可／内容的本地完成观察者；Facade在原获取完成及实际本地结算均结束后才回收inflight，原取消协调者等待两者。Core原子决定取消先封口或已开始提交继续等待，保存成功只报告真实迟到终态；保存失败完整保留原source。五秒只限制等待，调用者退出后重复取消仍加入原协调者。已终结失败的观察直接复用原事实，不新增失败停止任务污染健康关闭；该分支另有实际红测和修复。取消实现统一放在facade/cancellation.rs，不留转调副本，无第二池／许可、schema或Engine阶段查询。

真实许可和文件输出、明确保存端替身验证：保存期间重复获取Busy且不创建新输出、五秒未完成、等待方退出后重复取消、StorageFull两次查询保留、取消先于提交只清理本项且同批后项照常保存。Core／手动重试25项通过（5.093秒）；全部相关Application与真实Engine接收／重试214项通过（55.820秒）。最终同一源快照Core／Application／Engine库1887项通过／6项跳过（199.675秒）；metadata解析812包，workspace全目标check（1m36s）、repository、fmt、Rust style、1348站点隐私清单及diff通过。手工审查同一资源所有权、观察者无资源、锁顺序、终态不可逆、协调者复用及错误链；code-review skill缺失，不冒称技能审查。未执行设备仍跳过。

该片只解决运行中单项取消的实际等待，不等同于SQLite持久取消／保存失败分类完成。当前持久file_transfer可能已Completed而未entry_bound，Core在attempt失败时把该项投影Interrupted；须通过实际SQLite保存失败、混合取消与重开查询证明并修复，同时保留底层终态和原状态词表。普通已有partial重投映射、清理失败恢复、目录完整预算、表示blob、来源授权、全部生命周期、受管1GiB及最终head网络／CI／设备矩阵仍继续。


### 2026-10-22 网络完成后的本地失败持久结算

同一ReceiveAction在原许可内处理实际保存失败／提交前单项取消：先清理自身新输出，再经既有完整提交port的FileFailed原子保存固定业务失败与该项清理责任移交。Core核对原产物身份／归属、尝试可结算阶段，并决定已交付优先及网络完成后的业务投影；Infra在同一事务内比较原AEAD日志、验证原attempt、移除本项并写入既有failure_code。底层Completed不可回退；本地失败项不再计为已完成项／字节，迟到汇总不能把失败项重新绑定成功。记录失败本身出错则保留原保存source与独立结算source，Root保持非终态，不按NoEntry假报结束。复用原schema、状态字符串、固定失败代码和V1／V3，不新增格式、recipe、池、许可或Engine阶段查询。

实际SQLite保存触发器、产物AEAD与重开查询先暴露未交付仍计为2项完成，现已修复。最终原子提交契约20项通过（1.71秒）、加密转发2项通过；覆盖保存事务失败、混合失败／单项取消、晚段SQL回滚、已交付文件保护、终态拒绝及迟到成功绑定拒绝。生产V3重封后重新打开日志为空，原成功文件仍在。Core与完整Application手动重试24项通过（5.088秒），包括失败继续后项、原许可保护、双来源及非终态；保存端在Application接缝明确使用替身，不冒称实际Engine保存故障。

首轮五crate库2232通过／1失败／9跳过（200.781秒）：abandoned deletion在数据消失后立即重复删除，与原删除仍持有的封口竞争。原样单测4.510秒通过。测试改为等待该例唯一被丢弃等待的OperationFinished(Cancelled)完成事件后保留原NotFound断言；生产路径、超时与并发均未放宽。定向屏障4.622秒通过；最终同一源快照Core／Application／Engine／Storage／Content库2233通过／9跳过（197.177秒），metadata812包、workspace全目标check（1m43s）、repository、fmt、Rust style、1349站点观测隐私及diff通过。手工审查原子性、原许可和日志责任、已交付优先、底层终态不回退、source链及删除完成边界；code-review skill缺失，未冒称技能审查。设备未执行仍跳过。

后续优先验证Root在FileFailureSettlementError之后begin_receive_failure也失败的路径：目前先尝试状态写入再检查原错误，可能覆盖已有双来源，尚无故障注入证据；需先证明并修复，再在真实Engine保存边界注入SQLite故障。结算存储失败后的资源持有／恢复移交尚未完成，本轮finish仍会释放资源，不能把它当完整预算验收。计数本轮只排除已持久本地失败，未保存但无failure_code的网络完成项仍沿既有统计；整个保存期间的计数语义须继续核对。普通已有partial重投映射、目录／表示完整预算、清理恢复、来源授权、全部生命周期、受管1GiB与最终head网络／CI／设备矩阵仍继续，完整spec未完成。


### 2026-10-22 连续写入失败保留与真实保存故障验收

Application实际故障注入证实：文件保存StorageFull、失败事实结算PermissionDenied后，Root再次写begin_receive_failure的ReadOnlyFilesystem覆盖了原双来源。Root现先识别未完成的文件失败结算，直接返回完整来源并保留原尝试／日志，不再尝试第三次批次状态写入；实际回归确认两个原IO种类均保留、失败转换调用为0及Receiving不伪报终态。完整手动重试23项通过（5.096秒）。本轮没有改变原许可、格式、schema、状态字符串或Engine接口。

真实Engine经生产网络、实际V3／SQLite及稳定历史／重试操作，新增独立首／中／末保存故障：先在首个实际导出结束屏障安装按原item_id定向的SQL触发器，指定文件只有网络Completed后才拒绝保存；失败事实提交后不再拒绝最终部分汇总。三项网络／导出都成功，指定保存失败项被清理，其余两个均实际交付；重新打开数据库核对网络终态仍为Completed、失败项storage_unavailable及不同交付绑定。随后仅重试原失败摘要，三个文件最终成功，原两个成功文件路径与字节不变，protect／export累计仅四次。最终三个Native场景27.248秒通过，不使用存储替身、不新增测试schema。这里只重开数据库连接，不冒称该场景跨进程重启、来源C授权、Relay或1GiB资源验收。

首轮完整五crate库2235通过／1超时／9跳过（253.933秒），既有engine_start_builds_a_resumable_real_session超过原20秒；原样单测13.463秒通过。没有改超时、并发4、断言或生产启动逻辑，准确超时触发点未证实。最终同一源快照Core／Application／Engine／Storage／Content库2236通过／9跳过（215.351秒），该启动测试11.695秒通过；metadata812包、workspace全目标check（1m34s）、repository、fmt、Rust style、1349站点隐私及diff全部通过。手工审查负责人顺序、来源保留、定向故障的事务边界、实际保存结果和仅失败项重试；code-review skill缺失，未冒称技能审查。设备未执行仍跳过。

下一片优先未完成结算的资源责任：ReceiveAction.finish仍通过ReceiveBatch.finish清除local_work／commit_handoff，文件失败事实本身尚未写成时原许可和保护仍会释放。须用实际失败结算／关闭屏障先取得证据，明确原监督动作或既有reconciliation的完整移交，直到真实结算才能归还4／2许可，不能以错误对象持资源、第二预算或仅内存持有者替代活跃负责人。普通已有partial重投映射、准备拒绝交接、目录／表示完整预算、未保存阶段计数、来源授权、生命周期、受管1GiB及最终head网络／CI／设备矩阵仍继续，完整spec未完成。

### 2026-10-22 原失败结算责任继续执行（进行中）

- 本轮状态为进展：实际修改原处理／监督任务，而非只保留错误或孤立许可。`UnsettledFile` 只保存原尝试及清理资料；原 `ReceiveAction` 和原处理任务继续持有原下载资源，调用者经一次性回复先得到完整失败链。
- 已有屏障在修改前实际失败：`unsettled-resource-red.log` 显示保护计数预期 1、实际 0。新增用例还要求失败事实成功写入但父失败 CAS 继续失败时保留资源，全部修复后才释放；保存调用不得重复。
- 修复只补记 FileFailed、清理剩余受管产物和结算原尝试；没有重新下载／保存、第二额度池、持久重试队列或新增 Engine 步骤接口。局部失败以固定分类记录，按 1／2／4／8／16／30 秒退避，五秒仅约束外部等待。Core 校验原身份、取消胜出与已完成拒绝。
- 新增真实 Engine／网络／V3／SQLite 故障用例：首项保存，第二项保存和失败事实均由 SQLite 触发器拒绝；关闭应超时，解除失败事实故障后由原动作结算，保持首项文件并不启动后项。此项尚待执行，不以 fake 仓储通过代替。
- 编译验证两次被信号 15 终止（green.log、green-current.log），均无测试结果；首次期间实测可用内存约 278 MiB、无 swap、内存／IO 压力高，后续内存恢复。尚无证据断言信号来源，不能把中断记作断言失败或通过。`green-final.log` 正在重新验证。
- Rust 风格检查和 diff check 已通过；完整 Cargo／Native／仓库门禁仍未完成。普通既有 partial 再投递、清理失败责任移交、目录单元、来源 C 和完整矩阵等原范围保持未完成。

### 本机构建资源约束（用户最新反馈）

用户反馈每次验证导致本机卡死。已停止恢复／重启本机构建；最后进程检查没有 cargo、rustc、mbx 或 just 构建进程。不得沿前文继续自动重跑：先确定并落实可控的内存限制或其他验证环境，再恢复编译测试。现有代码保留且未验证，不宣称通过。实测机器 15 GiB、无 swap，先前 rustc 约 2.5 GiB，内存及 IO 压力高；不能推断所有临时内存均由本任务产生。

### 用户设置 swap 后恢复验证

用户明确 /goal resume；已确认 16 GiB swap 生效，可用约 13 GiB。沿原工作树恢复：构建通过临时 systemd scope 限制 MemoryHigh=3 GiB、MemoryMax=5 GiB、MemorySwapMax=4 GiB、CPUWeight／IOWeight=20，CARGO_BUILD_JOBS=1。保持共享缓存与仓库 target，验证串行执行，不因日志静默重复启动。首先只验证原失败结算资源屏障；完整 spec 范围不变。

资源受限验证首个结果：实际原许可／内容保护屏障通过（1项，15.01秒），同时验证 FileFailed 落库仍失败及 FileFailed 已成功而父失败 CAS 仍失败两个阶段；恢复后保存调用仍为1、原成功文件保留、资源归0。日志 `unsettled-bounded-target.log`，退出码0。默认特性下 Core／Application／Engine 定向28项通过（25.135秒），日志 `unsettled-bounded-related.log`；核对清单后确认它没有包含 dev-tools 下的真实 Engine 接收用例，不能冒称 Native 通过。已显式启用 dev-tools 单独执行 save_failure 模块4项（`unsettled-bounded-native.log`），结果待定。构建 scope 实测内存峰值约3 GiB、swap峰值约1.03 GiB；不更改缓存或 target，不调整测试超时。

真实 Native 首轮4项结果：3通过、1失败（37.054秒）；新关闭场景实际返回 Engine 1108／Internal／retryable，而非预期 DeadlineExceeded；独立重跑9.352秒复现同一结果，保留 `unsettled-bounded-native.log` 与 `unsettled-bounded-native-diagnosis.log`，不放宽关闭期限或断言。随后真实单测证明嵌套报告的纯期限被 is_deadline_elapsed 误判，`unsettled-nested-deadline-red.log` 为0通过／1失败。已实现只递归识别既有 LifecycleError／Arc 报告，必须全部原因都是期限才分类 deadline，混合 StorageFull 仍为实际失败且来源保留；它是否解释 Native 失败须经重跑确认，尚未宣称根因已关闭。

嵌套期限修复后的5项回归：纯分类与原3个Native通过，新关闭场景仍为1108／Internal（4通过／1失败，37.834秒），故该纯分类缺口不是此次Native失败的充分解释。已临时在 Engine lifecycle.rs 的 cfg(test) 中采集固定报告类别／任务计数／IO或SQL枚举种类，并在Native测试使用既有TaskJoinFailures收集器；不得输出原source正文或路径。当前 `unsettled-native-typed-diagnosis.log` 单独重跑；定位后必须移除临时report_shutdown_classes及其调用，不能携带eprintln诊断交付。

诊断证实实际差异：未安装订阅者的失败报告主原因仅期限到期，additional含TaskShutdownReport（timed_out_count=3，join_error_count=0），故先前只递归LifecycleError的修复不足。启用TaskJoinFailures订阅者改变调度，三次偶然通过（不能当根因修复）；去除订阅者后原样失败，`unsettled-native-no-subscriber-diagnosis.log`保存固定分类证据。现统一由Application LifecycleError识别全部原因中的真实纯超时TaskShutdownReport，join_error>0不能分类为期限；Engine删除仅扫描primary且只看timed_out>0的旧分支，避免同时有真实失败时误报期限。新增实际TaskRegistry超时／panic和混合StorageFull回归，完整source未丢失。已移除全部临时eprintln/report_shutdown_classes及Native订阅者；现无订阅者验证（unsettled-aggregate-and-native-final.log）进行中，断言及100ms调用期限未改变。

最终定向验证15项全部通过（38.170秒），包括默认无订阅者Native关闭场景10.069秒、原资源双阶段失败屏障15.011秒、真实TaskRegistry超时及混合panic／StorageFull分类；日志 unsettled-aggregate-and-native-final.log。当前已启动唯一scope uni-blob-settlement-final-gates：先重复无订阅者Native两轮，再显式dev-tools运行五crate库、metadata、workspace全目标check、fmt、style、repository及diff；阶段见 unsettled-final-stage.log，结果尚待检查。所有Cargo保持串行与原资源限制。

最终交付验证：无订阅者 Native 再次连续两轮10.331／10.691秒通过；同一源快照显式 uc-engine/dev-tools 的 Core／Application／Engine／Storage／Content 库2241通过／9跳过（263.366秒），既有startup测试12.017秒通过。metadata812包、workspace全目标check（3m45s）、fmt、Rust style、repository与观测隐私、diff全通过，unsettled-final-stage.log=complete、unsettled-final-gates.exit.log=0。资源scope已自动回收，最终峰值计数不可读，不能把执行期间约2–3 GiB样本冒称最终峰值；配置上限始终5 GiB内存／4 GiB swap，未清缓存或更换target。手工审查原许可／内容所有权、调用者退出与真实完成分离、逐阶段修复、已交付结果优先、终态不可回退、source链、超时混合真实失败以及临时诊断删除；code-review skill缺失，不冒称技能审查。完整spec及设备矩阵仍未完成，未执行设备仍跳过。

### d83bc4cc之后：未保存成员的聚合计数

真实Native默认无订阅者场景已红测：仅首项保存、第二项仍transferring且失败事实未写入、第三项pending，items_completed却返回2（预期1）；business-progress-count-red.log保留0通过／1失败（9.319秒）。完成判定现复用Core对未分组成员的业务投影，再聚合成员数与字节；目录既有成员粒度不改，也不改变目录单元集合／失败原因、raw网络状态、schema或持久格式。实际SQLite／AEAD的Progress提交前后及重开计数、当前attempt隔离与目录各状态断言已补齐；显式profile test-util的两组合约及Engine dev-tools的4个Native场景正在uni-blob-count-green中串行验证。尚未宣称通过，普通partial来源映射及完整spec其他范围仍未完成。

计数切片首轮结果：真实存储／AEAD合约33项通过（1.288秒），Native原3项通过，但新增断言又证明未启动第三项没有计入total（实际2、预期3）；business-progress-count-native.log保留3通过／1失败（35.477秒）。已完成数及字节的新断言此前均通过，不以删除总量断言缩小目标。现原始计划保留成员权重／字节元数据，查询统一按既有业务项结果投影全部原始量，包含未启动项；目录仍是一项可选单元但按成员权重汇总。物化及查询共用原有规模计算，未新增持久字段或状态权威；整数范围失败保留TryFromIntError并向查询调用者返回，禁止clamp或吞错。实际旧计划恢复、目录阶段及查询前无下载记录的测试正在补齐，结果未确认。

原始计划投影定向34项已通过（25.177秒，含原手动重试和目录／无下载记录／范围错误）：计划仅保留成员与字节权重元数据，业务状态仍由原Core／实际保存事实决定，没有第二状态权威。物化与查询共用receive_member_totals；内部查询以Result传播真实整数转换source，所有调用者已迁移。当前uni-blob-plan-real先跑Engine dev-tools 4场景，再跑Profile test-util的实际存储／AEAD合约；结果待定。

完整计划投影后的实际验证：原始计划／手动重试34项通过（25.177秒），默认无订阅者Engine dev-tools 4项通过（38.690秒），Profile test-util两个真实合约33项通过（1.308秒）。Native新断言已覆盖完成数1、原始总量3、完成字节仅首项、总字节包含全部3项，同时保留原关闭与持久失败断言。当前唯一scope uni-blob-progress-final-gates运行同一源快照五crate库、metadata、workspace全目标check、fmt、style、repository／隐私及diff；结果尚待检查，不复用d83bc4cc的绿灯。

- 业务计数最终同一源快照五crate库2244通过／9跳过（282.213秒）；metadata812包、workspace全目标check（3m41s）、fmt、Rust style、repository／1350站点隐私及diff通过。手工审查原Core业务结果复用、目录仍成员计数和整个单元、原始计划补全未启动项、整数范围及source，无可用code-review skill。没有更改超时／测试并发、schema或原下载预算。下一步实际保存失败＋输出清理拒绝，确认原资源是否提前释放，完整spec仍继续。

- 保存失败＋物理清理拒绝已取得两个真实红测：原source被第三次写入覆盖，以及原保护0／期望1。原ReceiveAction现登记物理清理剩余责任，原任务先清理、再FileFailed、再父CAS；本地清理成功状态只由该任务持有，之后不再删除同一路径。App相关25项通过（25.148秒），新路径复用回归3.007秒通过；下一步真实Engine／SQLite输出目录故障及最终门禁。网络失败等其他清理边界尚未声称解决。

- 初轮真实Engine保存故障5项通过（47.601秒），新增物理输出清理故障9.248秒通过。审查发现只等网络Completed仍有关闭先取消保存的时序缺口；测试现等待原EventStream的InboundNotice完整入口结果通知，证明原保存失败已经返回后再执行100ms关闭，不新增订阅者或sleep，不放宽断言。需验证此最后测试快照及根门禁。

- 保存后物理清理切片最终相关App25项通过（25.148秒）；带完整入口事件屏障的真实Native5项通过（47.537秒），新增场景9.450秒。metadata812包、workspace全目标check（3m17s）、fmt、Rust style、repository／1350隐私站点、diff全部通过。手工审查资源连续性、单次清理、原错误与事务、无重传和关闭边界，无可用code-review skill。不把前一提交的2244全量测试当本次结果；未运行设备仍跳过。

- 获取失败恢复编辑时将use插入模块内文档之前，fmt首次E0753；已将导入移回模块入口文档之后，不更改文档类型或绕过fmt。

- 获取失败恢复首轮扩大25通过／2失败（26.169秒）：旧mock清理错误测试仍假定责任返回后结束、期望旧OutputCleanupFailure；已由实际Facade、原许可和实际IsADirectory的强用例替代，源和取消断言移入该完整流程。新增用例已证实资源保留，但错误地要求已结算后的transport关闭忽略原Store错误。依据原supervision.rs把完成错误保留至关闭及shutdown_failure固定分类，现明确断言原Store／StorageFull不丢失，取消才关闭成功；生产关闭分类不改。

- 获取清理最终验证已启动：scope uni-blob-fetch-cleanup-final，PID1803067；App相关、Native六项、metadata、workspace、fmt／style／repository／diff严格顺序。日志fetch-cleanup-final-*.log，先看stage和exit，禁止因静默重启。当前未声明新快照通过或提交；原5 GiB内存硬上限／4 GiB swap、单构建任务持续。

- 分类修复后的App26项通过（26.187秒），包含清理阻塞时StorageUnavailable／Cancelled查询、原保护仍持有，以及关闭保留原Store／StorageFull。Native同源重跑仍在编译，scope uni-blob-fetch-cleanup-reason-final、PID1818283，当前无最终通过结论。

- 获取失败清理与查询分类最终同源：App26通过（26.187秒），Native6通过（58.593秒；新增导出故障10.257秒）。metadata812包、workspace全目标check3m18s、fmt／style／repository／1350隐私／diff均通过。保留原失败日志，不更改超时、并发或生产关闭分类。当前手工审查后提交；后续先证明物理清理成功但原日志重写失败时的来源与资源，完整spec继续。

- FileCleaned原子交接实现与五项实际Profile契约已补齐，尚未声明通过；唯一scope uni-blob-fetch-ack-contracts、PID1868627串行Core／App和SQLite／AEAD／加密相关测试。先检查原stage／exit及实际进程，不重启活动任务。fmt、Rust style及diff当前静态通过。

- FileCleaned首轮Core／App29项通过（28.178秒）。真实Profile阶段编译E0599：TrackedFileTransferStatus只提供from_str_value而非FromStr；已使用原解析入口，未知值交Core明确拒绝，不补造状态。审查同时将唯一文件事实读取限制为两行（足以拒绝重复），新增真实SQLite未知／重复事实拒绝测试。原Profile尚未执行测试，不冒称通过；下一轮只重跑受影响的Profile契约，再扩大Native。

- 修正解析入口后的真实Profile41项通过（1.748秒）。随后补合法的“已准备产物但未登记网络”红测，0.072秒被原Core拒绝；现明确区分Missing／Tracked／Unusable，缺失只允许清理而不补造网络状态，本地保存失败仍要求Completed，未知／重复仍拒绝。新增真实Engine定向SQL拒绝第二项登记，验收首／三项交付与无永久Receiving；以及SQL回滚测试具体追溯Diesel来源。当前新源快照待重新执行Core／App／Profile／Native与门禁。

- FileCleaned最终同源验证启动：scope uni-blob-fetch-ack-final、PID1932962，Core／App、真实Profile、完整Native receive_retry及根门禁顺序运行。新记录类型消除“缺失等于损坏”的混淆，SQL拒绝保留具体Diesel来源。不得沿用旧29／41结果作为新接口快照绿灯，先看fetch-ack-final-stage／-gates.exit及真实scope。

- FileCleaned原子交接最终同源Core／App30通过27.159秒、Profile42通过1.721秒、完整Native receive_retry14通过99.672秒（新登记故障9.318秒）。metadata812包、workspace全目标check3m52s、fmt／style／repository／1350隐私／diff通过。手工审查原资源、当前attempt、AEAD／唯一记录、未登记与损坏区分、已交付保护及原网络事实；无可用code-review skill。现在整理提交，完整spec仍继续。

- 原子交接已提交da77bea0。普通partial再次SendFiles的真实Native红测已启动，scope uni-blob-redelivery-reuse-red、PID1998918；只新增测试，生产未开始映射修改。先读redelivery-reuse-red.log／exit及真实scope，未结束不得重启或改Rust。

- 普通partial重投Native红测已实际终结100（8.300秒，Summary8.322秒）：重新获取了原成功内容而非唯一失败内容，证据redelivery-reuse-red.log。当前无活动该scope。生产映射尚未修改，先审查共享原来源加载／CAS新项继承机制，保持原成功路径和原许可预算。

- 普通 partial 重投：已把手动重试的原资料读取、成功输出校验与选择准备提取到同一 ReceiveRetryPreparation；正常接收沿原 action 使用既有 accept_retry 和完整 ledger CAS。取消任务完成屏障由装配预先创建并与取消负责人共享，不引入 Apply 反向依赖。Application all-targets check 通过（36.77s），Rust style 通过；真实 Native 红转绿及相关取消／重试测试正在有界串行执行，尚未报告通过。普通重投使用原 tickets／原顺序，新信封仅作为同身份重投触发，原材料不可恢复则失败，不能拿新数组补旧事实。

- 普通重投首轮最终验证：Application相关42项全通过42.432s；完整Native receive_retry16项全通过129.249s，包含普通／顺序变化两种真实重投；metadata812包、workspace all-targets check3m17s、fmt／style／repository／隐私／diff全部通过。手工复查补回纯inline原有升级边界，随后扩大到完整apply_inbound相关测试。该inline测试首编译因直接访问private字段失败E0616，已改为现有StoreOnlyPullDeps/CommonDeps真实装配（没有新增setter或扩大接口）；首失败日志保留redelivery-inline-final-application.log，现运行redelivery-delivery-final-*。

- inline补充测试第二轮：恢复行为已Applied，但新增断言误把正常begin_redelivery登记也要求为0（实际1）；这是测试度量选错，不改生产行为。改为严格要求正常登记1次且BeginReceiveRetryInput集合为空。保留redelivery-delivery-final-application.log失败；当前redelivery-verified-application.log完整136项通过44.706s，Native两项与根门禁继续有界串行执行。

- 普通partial重投最终源快照验证：redelivery-verified-* 全阶段exit0；Application完整apply_inbound及取消相关136项通过44.706s；真实Native普通与顺序变化重投2项通过18.941s；metadata812包；workspace all-targets check3m16s；fmt／Rust style／repository含隐私与负例／diff全部通过。scope已inactive；活动期证据保存redelivery-verified-scope.log，观测MemoryPeak 3,223,322,624 bytes、MemoryHigh3GiB／Max5GiB／SwapMax4GiB，Cargo jobs1。手工代码复查完成，保持原材料／原身份、完整ledger、原action及实际取消屏障；纯inline正常修复覆盖。完整Native16项先于inline边界修正的源快照通过，不冒充最终头16项；本片没有完成整个spec。

- 普通partial复用已提交bbf86498。继续完整成果第三次投递，真实Native红测9.400s：total_accepted0／total_errored1，full-redelivery-red-exit100。现修复唯一Storage可用性能力，按原event／representation AAD用既有BlobCipherPort认证文件URI，再检查真实文件；Engine仅一次性私有工厂延迟构造完整port到cipher可用时，Application可用性错误保留source而不当FullyHeld。没有新schema、依赖、明文例外或Engine业务阶段接口。
- availability-final：Engine dev-tools lib编译检查通过1m01s；Application入站／Storage availability相关合同149项通过46.195s，包括实际AEAD已存在／消失、密文损坏、明文冒充与AAD变更，以及Application失败不重显／不覆盖且保留IO source。真实Native3项与根门禁尚在执行，不提前记通过。

- availability-final Native首项9.520s未通过新增过严断言：修复后已从total_errored1变为total_duplicate1，既有resurface_held_entry含快速重复窗口，即时第三次投递合法命中DuplicateSkipped。修正测试为完整成果须得到一个Accepted或Duplicate且errored／pending均0，并继续严格要求原attempt、路径／字节、导出数与保护数全部不变；不改变生产行为、窗口、等待期限或增加sleep。先前149项及Engine编译适用相同生产代码；仅重跑改正后的3项Native与根门禁，首失败保留availability-final-native.log。

- 完整成果可用性最终验证全部通过：同一生产源快照Engine dev-tools lib check1m01s、App／Storage合同149项46.195s；修正新增测试对快速重复的错误预期后Native3项27.185s全通过。availability-verified metadata812、workspace all-targets3m14s、fmt／style／repository含隐私与负例／diff各exit0。scope已inactive，无重叠Cargo；活动期资源记录availability-verified-scope.log。手工复查：原event／representation AAD、认证失败不冒充FullyHeld、正常物理存在性规则保留、私有一次性工厂只负责完整port装配，无新依赖／schema／明文例外。准备提交；网络终态store失败原任务修复、目录／表示完整预算与生命周期、来源C、1GiB及最终head完整矩阵等继续，整个spec未完成。

- 网络终态真实红测18.268s（terminal-repair-red-exit100）：解除SQL拒绝后10s仍无原Failed终态，初始已通过第一成功项保留／失败输出物理清理／关闭100ms未完成屏障。现原supervise worker可先交付失败但completion仍仅在实际结束后提交；session先封存终态意图，保留stored／published进度以重试同一事件且阻止关闭改写。原worker按1／2／4／8／16／30s修复，错误只含原链和纯完成信号，不持资源。父FileProgress以该信号延后NoEntry／FileFailed或FileCleaned；完成后的本地交接失败保留RawCompleted，不借用FetchFailure。首次编译发现函数指针改名／私有policy引用，第二轮发现supervise泛型测试遗漏，均迁移原调用方而未加兼容包装；Fmt曾把import放在inner doc前，已修正。最终Application all-targets check26.42s与原终态／manual_retry／file_transfer会话合同47项50.417s通过。真实Failed与Completed两种SQL故障和取消／准备、表示／目录路径仍须继续，不记全spec完成。

- terminal-repair-native：原Failed与RawCompleted本地失败两种真实SQL拒绝恢复均通过，合计19.085s。随后terminal-repair-final完整Application lib＋file_transfer集成1275项全通过113.597s，1项独占日志诊断按ignore跳过（blocked_updates_explain_queue_wait_and_coalesced_wakes_without_changing_order，下一轮精确独占执行）。Native20项跑到18项，前17项通过，新增Cancelled用例完成关闭和RawCancelled后仅因新增断言错误失败：既有projection在failure_code保存LocalUser.as_str()="local_user"，不是None。已改为严格核对原取消原因，不改生产逻辑／窗口／deadline；保留首失败terminal-repair-final-native.log。后两项当轮未运行，不记通过。

- 网络终态文件切片最终验证：同一生产源快照Application lib＋file_transfer全量1275项通过113.597s；唯一ignore的独占日志诊断已精确运行1项通过0.07s。改正新增取消原因空值断言后，完整Native receive_retry20项177.490s全通过（Failed／Completed／Cancelled SQL拒绝恢复、原成功保留、关闭等待均验证）。terminal-repair-verified metadata812包、workspace all-targets3m09s、fmt／style／repository含隐私与负例／diff全部exit0。原scope inactive，活动期资源观测保存terminal-repair-verified-scope.log。手工复查原worker／同一DownloadSettlement持资源、结果与完成分离、原意图封口及父文件完成信号、RawCompleted本地失败沿FileFailed、失败／取消沿FileCleaned，无新schema或监督池。首次真实红测与编译／新断言错误日志均保留。准备提交；初始登记后Started写入、表示／目录父NoEntry及全预算／生命周期、来源C、1GiB、最终head完整协议与CI／平台矩阵等未完成，完整goal active。

- 2026-10-23 Started 准备责任：真实 Native 首轮 19.855s 失败，数据库解除拒绝后第二项仍 Pending；修复后 10.713s 通过。相关 Application／文件会话 27 项通过39.985s。原 session 先登记、冻结准备失败意图，原下载任务补齐 Started 与 Failed；原接收动作待真实屏障再清理，不重新获取内容。完整 Native 接收重试21项及根门禁正在同一5GiB受限scope串行运行，见 started-repair-verified-*；未把在运行项记成通过。

- Started切片最终验证：Native receive_retry 21/21通过186.083s，相关App27/27通过39.985s；locked metadata、workspace alltargets3m07s、fmt、style、repository、diff均退出0。受限串行scope已退出，源码人工审阅完成；下一步表示／目录父结算，完整goal保持active。

- 表示终态真实红测：5.630s失败，原Completed仍被SQL拒绝且raw Transferring时父状态已Failed。待原完成信号后再结算的首轮编译暴露anyhow::Chain跨await不Send，改成先克隆元数据完成信号再await；首轮失败日志未覆盖。修复后Native及相关App正在串行验证，unit-settlement-corrected-*。

- 表示终态切片最终：Native1项11.502s、相关App124项45.721s通过；metadata、workspace alltargets3m07s、fmt、style、repository、diff均退出0，scope已结束。等待原信号前记录已知StorageUnavailable，父状态Receiving直至真实终态及业务结算完成。下一批集中验证父NoEntry持久失败／目录成员／物理回滚和原保护生命周期，再统一最终矩阵；完整goal active。

- 父NoEntry表示切片：Native真实双SQL红测15.493s遗留Failing；修复后2项Native通过19.414s。App四组合首次错认ReadOnlyFilesystem注入类别导致2.047s失败，修正后None真实红测0.032s失败；None接入同一原责任后4.061s通过。原分类Capture／Internal、双来源、原保护、一次保存及错误不持资源均由完整cap合同核对。
- 源码已冻结运行unit-no-entry-verified-*，PID／阶段／退出码各有日志，scope uni-blob-unit-no-entry-verified，磁盘TMPDIR为短路径uc-bc-tmp-n5。先focused合同已过，再App全量＋file_transfer、独占ignored诊断、Native receive_retry、metadata／workspace／fmt／style／repository／diff串行；尚未完成项不记通过。更早n1-n4目录scope终态后检查为空并rmdir，首次过长路径已回收。完成本轮后检查同scope实际inactive，再清理n5。

- 父NoEntry表示保存失败最终验证：App＋file_transfer1276项116.667s、ignored独占诊断1项0.05s、Native receive_retry23项227.407s通过；metadata、workspacealltargets3m10s、fmt、style、repository、diff均退出0。scope实际inactive，n5检查为空并rmdir，本轮所有新临时目录均已回收。仅此边界完成，目录及最终全矩阵保持open；完整goal active。


### 2026-10-23：目录撤回原责任与恢复检查点（局部验证）

目录发布后的物理撤回由原 ReceiveAction 持有原 DirectoryPublication 继续执行；调用方仍只发起完整接收，失败先保留原业务错误及撤回错误，不创建补偿任务池。未撤回根保留到原任务实际成功，已撤回根不再触碰其可被用户重用的目标名称；目录发布中途失败与内容保存失败均交接到同一原责任。原任务继续负责关闭屏障、失败声明、撤回和 NoEntry，重启恢复沿既有目录日志，不增加格式或协议版本。

真实文件系统故障测试先复现保存失败／中途发布失败后原任务不再撤回的问题，再验证原任务继续清理、关闭短等待未完成、用户新建同名根不受影响。相关 Application 测试先有 61 项通过，本地清理 5 项、真实 SQLite／AEAD 日志及原子提交 6 项通过。隐私回归曾因遗漏原固定警告和安全计数上下文失败；已恢复仅含计数及固定分类的诊断，保留实际底层 source，未加入文件名或路径。

随后新测试复现二次恢复误删：物理清理成功但 NoEntry 被拒绝后，用户重建的同名目录被下次恢复删除。恢复现先把旧可见输出撤回独占 staging，保留逐根证明，再通过既有 RecordDirectoryPublishPort 持久记录零可见根，然后清理 staging 和提交 NoEntry；检查点拒绝也保留证明。已记录零可见根或原 Staging 阶段的记录不再认领目标名称。完整 Application 恢复测试覆盖检查点拒绝和 NoEntry 拒绝两条路径，目录及 reconciliation 共 62 项通过（3.398 秒）。这不是完整进程崩溃矩阵验收。

本轮全量 Application、workspace 与根门禁正在资源受限单 scope 内串行执行，最终结果另记。一次直接启动 repository 门禁发现其内部会启动 Cargo，已终止该次运行（143），随后统一纳入同一受限串行 scope，未把该次记为通过。编译使用共享 target／共享缓存及短磁盘 TMPDIR，jobs=1；物理内存上限 5GiB、swap 上限 4GiB。目录早期准备失败、成功后的 staging 清理、其他撤回入口及稳定 Engine 全生命周期仍未完成；完整来源 C、资源、最终兼容／设备矩阵保持未完成。


目录切片的扩大验证：Application＋file_transfer 1281 项通过（120.502秒），1项原配置跳过；LAN file_upload 10项通过（0.069秒），其余228项不在该过滤范围。首次 workspace all-targets 检查因 LAN 测试装配缺少 record_directory_publish 失败，已迁移该调用方；随后全 workspace 检查通过（2分33秒），locked metadata、fmt、Rust style、完整 repository preflight（含隐私及负例）和 diff 均退出0。人工差异审阅核对原待结算位置、失败 source、逐根撤回、零可见根检查点及用户名称重用保护；没有增加持久化字段或 Engine 内部查询。编译发现本次移动代码留下一个未使用测试导入，移除后正在最后检查，不重跑无行为变化的全部故障测试。本轮原 active scope 的已观察峰值约3.0GiB；结束后not-set不记作峰值。Native完整目录接收及其他未执行矩阵仍未通过本节登记。


移除未使用测试导入后的最终 workspace all-targets 检查通过（2分38秒），fmt／style／diff全部退出0。最后仅剩既有警告；本切片 source 无新增未使用导入。全部验证 scope 已结束；提交前检查专用磁盘 TMPDIR，结束后回收。完整 spec 继续保持 active。


### 2026-10-23：目录准备失败的原暂存区责任与父NoEntry交接

完整负责人仍为 Application 接收流程，调用方只发起完整接收。DirectoryPublication 从 staging 打开成功后即建立，覆盖成员获取、目录组装、Staging／Publishing 记录及最终发布；所有物化失败汇入同一撤回路径。先持久记录 Staging 完整根映射，再创建成员目录及获取内容；只有该记录实际写入成功才给原持有者绑定可更新的日志，不能在首次写入失败后永久更新一条不存在的记录。

真实文件系统权限拒绝红测5.011秒失败：成员获取返回StorageFull，staging清理失败只记诊断，恢复权限后原动作不再清理。修复后原任务保留暂存区、原batch及关闭屏障，只修复本地结算，不重复下载或保存。首次Staging日志拒绝叠加父目录真实权限拒绝也沿同一责任恢复，测试确保不向不存在的日志写零可见根。

第二个红测复现物理清理成功后父NoEntry拒绝（0.006秒）：原关闭等待提前结束。现在物理清理成功的持有者也交回原接收动作，直到父NoEntry实际提交成功；后续持久重试不会重复删除已清理路径。错误保留原获取失败及结算失败两条来源。准备失败的既有StatusChanged终态通知由同一结算负责人在真实交接后发出一次，不在等待修复时提前发终态；公共通知不复制底层错误正文，取消仍用既有local_user。保存失败的原通知行为不另改。

新测试通过完整ApplyInboundClipboardUseCase驱动、实际文件系统权限拒绝及既有完整持久化port注入；覆盖关闭短等待、恢复后清理和父终态、两类原source、不重复获取、无提前通知及最终唯一通知。目录与reconciliation共65项通过（6.509秒）。NoEntry红测首次编译因测试装配的commit／clock位置和缺少事件导入失败，已修正并保留日志，之后才记录实际行为红测。Unix权限测试不冒充其他平台或真实SDK／SQLite目录全链路验证。全量Application与根门禁正在单scope内串行执行；成功后的staging清理、open/probe失败、其他撤回入口、真实Engine目录生命周期及后续矩阵继续保持未完成。


本切片最终同源检查：Application＋file_transfer 1284项通过（123.869秒），1项默认跳过；locked metadata、workspace all-targets（3分07秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff全部退出0。人工审阅核对从准备起持有同一publication、Staging先记日志再建成员、日志绑定只发生在实际写入后、已清理持有者继续承担父NoEntry、双来源以及终态通知时机。未增加Engine步骤接口、持久字段、明文例外、任务池或网络重试。全部构建scope已结束，提交后检查并回收本次专用磁盘TMPDIR；已观察峰值约3.0GiB。完整spec仍active，当前未完成项不缩减；另已记录同一Failed的NoEntry回执丢失重放边界待真实SQLite复现。


### 2026-10-23：NoEntry事务已提交而回执失败的只读确认

真实SQLite执行器包裹原事务，在实际提交后返回一次BrokenPipe；红测0.257秒失败，数据库已为Failed，而原NoEntry再次调用返回AttemptStateMismatch。首次测试编译的source链迭代生命周期问题已改为明确遍历，编译失败日志保留，之后才登记实际行为红测。

Core负责跨记录确认规则：同一条目、同一尝试及相同失败／取消终态才可确认；None只确认父终态，不移交其他产物；RolledBack还必须存在已settled／rolled_back的原产物记录。Infra复用原尝试行转换，在同一事务中读取并解析事实后调用Core；确认直接返回，不刷新时间戳、不覆写产物、事件或条目。不存在、仍待清理、已交付及未知产物不能被RolledBack确认改写；未知值保留实际解析来源。此路径不新增持久字段、状态字符串或Engine步骤接口。

Core 2项通过（0.017秒）；实际SQLite入站提交29项通过（9.794秒），包括Failed／Cancelled × None／RolledBack四组回执丢失，SQL触发器拒绝任何终态／产物UPDATE，重开后时间戳与AEAD密文完全不变；产物缺失、pending、landed与未知值矩阵保持拒绝，旧尝试不能改变重投的既有契约继续通过。

完整Application接收使用既有完整提交port模拟“先实际置Failed、再返回一次回执错误，并阻止后续确认”，验证原关闭短等待不完成，解除确认阻止后原任务结束，原获取StorageFull及回执BrokenPipe均保留，下载不重启，最终通知只发一次。目录结算7项通过（7.180秒）。该流程使用port模型及真实文件系统，实际SQLite验证在前述独立存储契约；不把两者合称真实Engine回执丢失注入。本轮全量Application、Native receive_retry及根门禁在同一受限scope内串行运行，结果待追加。完整spec其他目录清理、目录网络终态、来源C、生命周期、资源和最终兼容矩阵保持未完成。


本切片最终同源检查：Application＋file_transfer 1285项通过（124.681秒），1项默认跳过；Native receive_retry 23项通过（228.790秒），其余300项不在过滤范围。locked metadata、workspace all-targets（3分30秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff均退出0。Core 2项、真实SQLite／AEAD 29项的已通过代码未再修改；最终只有App测试名称引入整理后参与全量回归。人工审阅确认规则归Core、转换复用原mapper、确认在原事务中只读、相同终态／尝试检查和产物拒绝、原错误链、既有取消确认以及不重复下载／通知；未增加格式或Engine阶段入口。构建保持单scope串行、jobs=1、共享target／缓存和短磁盘TMPDIR，已观察峰值约3.0GiB；scope结束后核验并回收专用目录。完整spec仍active，Native回执丢失注入、目录成员终态与其他未执行矩阵不冒充完成。


### 2026-10-23：真实目录成员终态交接及非终态失败原因

以稳定Engine操作驱动真实目录发送：测试组合根的HostClipboard可在配对和故障安装后交出inline目录URI，走ObserveClipboardChange(dispatch=true)、原目录遍历／V3文件集合、SDK获取及SQLite。仅拒绝本次目录成员的Completed写入，原独立文件接收不受阻。测试初次误用公开枚举Directory而提前终止构建（143，未执行测试），随后改为既有DirectoryUnit；首个实际执行因错误假设“未启动成员不记失败行”失败，已改为核对一个Transferring，其余只可为未启动失败，不能靠底层总行数推导开始过下载。修正后的真正行为红测6.770秒失败：原成员仍Transferring时父目录已Failed。原网络意图恢复与关闭等待先完成后才断言该快照，故红测不是因尚未清理的测试资源失败。

原FileCacheBlobMaterializer仍负责完整流程，表示与目录共用一个私有等待实现：首次获取错误含FetchTerminalFailure时先记录已知业务失败，再等待原下载任务的真实终态完成信号，随后继续原目录失败／回滚／NoEntry。没有新资源池、重复获取、持久重试队列或Engine阶段入口；错误中的信号不持有许可和内容保护。目录不会因一个成员的本地终态仍在修复而提前完成父结算。

Core红测0.002秒复现目录非终态聚合丢失StorageUnavailable（None／Some）；聚合现在保留已有成员的具体失败原因，同时目录项及父尝试继续保持非终态。完成／取消的原终态规则保留，失败原因本身不代表父结算完成。Core与稳定Engine查询字段均已说明此含义，不加新字段或分类值。Core接收项3项通过（0.030秒）。提取共享等待时的一处Arc借用编译错误已修正，原日志保留。

实际Native目录与表示两项通过（25.138秒）：目录父Receiving、DirectoryUnit及StorageUnavailable、原总成员2／已保存0、100ms关闭未完成；解除SQL拒绝后原网络仍Completed，父目录最终Failed／Cancelled，暂存输出实际清理，不重启后续成员或旧成功项，旧成果字节不变。表示同一终态修复回归通过。测试使用实际文件系统、V3、SDK和SQLite，不以App假端口代替；未据此声明所有目录生命周期或资源矩阵完成。

当前扩大验证在单个受限scope内串行运行全量App、Native receive_retry、真实入站查询契约与根门禁。新真实SQLite查询断言还核对无活动接收快照时，持久成员的已知失败同样保留而目录仍Transferring。最终结果待追加；成功staging清理、open/probe失败、其他撤回入口、完整生命周期、来源C、受管1GiB和最终兼容／设备矩阵继续保持未完成。


本切片最终同源检查：Application＋file_transfer 1285项通过（124.457秒），1项默认跳过；Native receive_retry 24项通过（236.065秒），其余300项不在过滤范围；真实入站查询契约13项通过（2.799秒），新增持久目录失败原因断言通过。locked metadata、workspace all-targets（3分29秒）、fmt、Rust style、repository preflight（含隐私及负例）与diff均退出0。Core接收项3项通过后的行为实现未再改动，只补充查询字段说明；目录与表示原终态屏障已由实际Native再验。人工审阅核对原资源仍由DownloadSettlement及原batch持有、私有共享等待不交接资源、非终态原因不构成父终态、实际Raw Completed未被父失败覆盖、旧成果及原staging清理。单scope串行、jobs=1及既有5GiB内存／4GiB swap上限保持，已观察峰值约3.0GiB；结束后检查无活动进程并回收专用磁盘TMPDIR。完整spec继续active，其他目录清理入口、生命周期、来源C、资源和最终兼容／设备矩阵不据此勾选完成。

### 2026-10-23：已保存目录的成功清理责任

完整Application接收以既有端口及真实文件系统权限拒绝复现：两个根已经发布且捕获成功，父状态Completed，随后暂存区无法删除；旧实现只记警告并使关闭立即成功。红测0.007秒失败。DirectoryPublication::commit现在借用原持有者并返回实际IO错误，只有清理成功才标记完成；原接收任务通过私有一次回复通道报告错误，继续持有原batch、许可及输出责任，按既有1／2／4／8／16／30秒退避，仅修复本地清理。未新建任务或队列、未扩展公开Engine接口、未修改持久格式。

目录结算8项通过（1.049秒）：权限恢复前关闭短等待未完成，恢复后原任务清理暂存区并结束；原PermissionDenied来源可追溯，捕获一次、获取两项，两个已保存根仍为hello，父Completed未改写，也未进入失败结算。测试沿完整App能力与实际文件系统，捕获提交状态由既有端口模型表达，不冒充SQLite／SDK全链路。全量App及根交付检查待执行；其他open/probe清理和完整spec矩阵继续未完成。

本切片最终同源检查：Application全部1288项通过（17.844秒），1项跳过；locked metadata、workspace all-targets（3分17秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff均退出0。人工审阅核对一次回复只消费调用等待、原处理任务继续存活、batch的commit_handoff持续持有真实下载资源、成功事实不转入NoEntry／回滚、成功清理后才继续原收尾；所有commit调用已迁移为检查Result。此次没有执行Native或设备矩阵，不复用旧结果冒充当前验证。单scope串行且共享target／缓存，已观察峰值约3.0GiB；scope结束后确认无本轮活动编译并回收空的专用TMPDIR。完整spec保持active，后续open/probe失败清理、其他withdraw入口、生命周期、来源C、受管1GiB、最终兼容与设备矩阵仍未完成。

### 2026-10-23：目录候选打开与探测的清理责任

完整负责人仍为Application materializer及原接收任务，调用方只提交接收／关闭。真实权限拒绝红测0.007秒复现：目标卷探测返回不支持后，原实现忽略删除错误、转向受管缓存下载并提前完成关闭。现在为每个候选在打开前建立DirectoryPublication；打开失败或探测不支持后先实际rollback清理，只有清理成功才允许正常回退。清理失败把原候选交给既有batch及完整接收失败修复，保留真实来源；受管候选的打开失败即使已清理也保留至父NoEntry真实交接。不增加任务、格式、持久队列或公开Engine步骤接口；移除原先只记警告的discard_staging实现。

目录／rollback／probe共70项通过（1.066秒），包含正常不支持卷仍能保存至受管目录、按目标缓存一次探测，以及新增完整接收故障：清理未完成时关闭短等待不成功，未启动任何获取／捕获；权限恢复后原任务自动删除探测区，父尝试Failing至Failed，PermissionDenied来源保留，无关文件不变。本轮仅验证同进程原责任，首次Staging日志之前强杀的清理仍须另验，不能据此声明重启矩阵完成。全量Application、真实Engine目录回归及根检查待追加；其他withdraw入口、生命周期、来源C、受管1GiB和最终兼容／设备验收继续未完成。

本切片最终同源检查：全量Application1289项通过（17.870秒），1项跳过；真实Engine目录成员终态回归1项通过（11.609秒），其余323项未纳入本轮过滤；locked metadata、workspace all-targets（3分17秒）、fmt、Rust style、repository preflight（含隐私与负例）和diff均退出0。首次Native命令漏加dev-tools，实际匹配0项并以4退出，后续检查未运行；已在确认原scope结束后以`just cargo nextest run -p uc-engine --features dev-tools --lib -E 'test(real_engine_directory_member_terminal_repair_keeps_parent_receiving_until_settled)'`修正，未把零测试记为通过。两个原始日志均保留。

人工审阅核对候选在任何打开IO前即有原责任、失败候选实际清理后才回退、受管目标仍不要求NoReplace、探测缓存与正常降级行为保留、原错误及清理错误均由结构化双来源保存、没有开始下载也不补造文件失败行。构建保持单scope串行、jobs=1、共享target／缓存和既有5GiB内存／4GiB swap上限；已观察峰值约3.0GiB，最终scope inactive且无本轮活动编译，专用磁盘TMPDIR为空并回收。没有执行设备矩阵。完整spec保持active，其他withdraw入口、首次Staging日志前强杀清理、生命周期、来源C、受管1GiB、最终兼容和设备矩阵继续未完成。

### 2026-10-23：目录提交胜出后生命周期封口

检查提前退出路径时确认实际物化器已在发布前验证身份；转而以真实生命周期入口复现更直接的竞争。完整store-only接收、真实文件系统和既有发布port在最后一个目录根完成重命名后调用readiness.mark_not_ready，关闭原代次。红测0.006秒失败：物化器已CAS取得Committing，外层仍将停止信号改为PartialCancelled，丢失取消权后返回内部错误，未完成原目录保存。

完整负责人仍为原Application接收；有权威尝试且携带已发布DirectoryPublication时，外层保留已胜出的完整目录结果，随后仍以既有claim_receive_commit复核同一尝试的Committing，再调用原完整捕获能力。其他接收的取消路径不改，封口仍禁止新工作，原任务不重新打开代次、下载或创建第二次尝试。物化结果的权威提交前提已补充说明，不新增格式、接口或后台任务。

目录／generation／cancellation107项通过（11.322秒）。新增完整捕获端口模型检查Complete上下文、DirectoryPublished及同一尝试，拒绝部分或无责任保存；原捕获一次、父Completed、代次仍关闭、两根字节不变、暂存区实际清理。原取消先赢CAS、失败撤回和关闭屏障也在本轮过滤回归中通过。本测试使用真实文件系统与App端口模型，不冒充SQLite／SDK生命周期强杀矩阵。全量Application、真实Engine目录回归及根检查待追加；其余withdraw入口、首次Staging日志前强杀、完整生命周期、来源C、受管1GiB和最终兼容／设备矩阵继续未完成。

本切片最终同源检查：全量Application1290项通过（17.891秒），1项跳过；真实Engine目录成员终态回归1项通过（11.450秒），其余323项未纳入本轮过滤；locked metadata、workspace all-targets（3分51秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff均退出0。人工审阅核对发布持有者只能在权威目录取得Committing后返回、停止信号不再把该结果降为部分取消、后续仍校验原尝试而未绕过CAS、其他类型取消逻辑保留；捕获测试明确拒绝Partial和缺失目录责任的上下文。未据此声明真实Engine的完整生命周期或设备矩阵通过。

构建继续单scope串行、jobs=1及共享target／缓存，既有5GiB内存和4GiB swap上限未放宽；已观察峰值约3.0GiB。scope已inactive且无本轮活动编译，专用磁盘TMPDIR为空并回收。完整spec继续active；其余withdraw入口、首次Staging日志前强杀、完整生命周期／来源C／受管1GiB／最终兼容与设备矩阵保持未完成。

### 2026-10-23：已发布目录在保存前拒绝的完整责任

完整负责人仍为原Application接收，调用方只发起完整接收／关闭。真实目录已发布后，外层提交权复核的存储错误原先经问号直接退出，丢弃DirectoryPublication。完整App＋原ClaimReceiveCommitPort＋真实文件系统红测5.013秒失败：返回裸复核错误而没有应有的失败／撤回双来源。复核错误现在交给既有fail_directory_receive，按原权威转Failing，实际撤回、清理并交接NoEntry；撤回受阻时原任务保留原batch／许可继续本地修复，不再次获取或捕获。目录结算11项通过（1.071秒）。

同一区间的外层身份复核拒绝也曾消费发布责任、只记录撤回失败并继续结束；第二个红测5.011秒明确失败于关闭提前成功。正常物化器仍在发布前校验身份，测试只在既有物化能力返回后故意改变身份分量，以驱动防御拒绝分支，不把它称为正常SDK身份绕过。该分支同样交回既有完整失败结算；原身份拒绝和撤回IO均保留，公共通知不复制含内容哈希的底层正文。没有新增任务、格式、队列或公开Engine步骤接口。

目录／取消／代次109项通过（11.307秒）。新故障覆盖两个根真实发布、StorageFull或身份拒绝、PermissionDenied撤回受阻、关闭短等待未完成、解除拒绝后原任务自动清理及最终Failed，未调用捕获、只获取原两项、无关文件字节保持。既有复用目标名称、失败回执丢失、成功后清理和提交胜出后封口也在本轮回归通过。全量Application、当前源的真实Engine目录回归和根门禁待追加；其他重复投递／placeholder撤回入口、首次Staging日志前强杀清理、完整生命周期、来源C、受管1GiB、最终兼容及设备矩阵保持未完成。

本切片最终同源检查：全量Application1292项通过（18.067秒），1项跳过；真实Engine目录成员终态回归1项通过（12.185秒），其余323项未纳入过滤；locked metadata、workspace all-targets（3分24秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff均退出0。人工审阅核对有发布责任时复核失败不再经问号退出、身份拒绝不再消费撤回失败、原失败类别及完整来源保留、既有权威／回滚／NoEntry统一结算且不重复获取或保存；没有发布责任的原错误仍沿原路径返回。新故障为完整App能力及实际文件系统端口注入，不冒充真实Engine的同一存储／身份故障或强杀矩阵。

构建保持单scope串行、jobs=1、共享target／缓存及5GiB内存／4GiB swap上限，已观察峰值约3.0GiB。最终scope已inactive，确认无本轮活动编译后回收空的专用TMPDIR。完整spec保持active；重复投递／placeholder等剩余撤回入口、首次Staging日志前强杀清理、完整生命周期、来源C、受管1GiB、最终兼容及设备矩阵继续未完成。

### 2026-10-23：目录URI去重与最后的旧撤回入口

完整负责人仍为原Application接收，调用方只发起完整接收／关闭。先保存旧目录、删除旧根，再接收同名但内容身份和字节不同的目录，实际文件系统红测0.011秒失败：发布后的本机URI匹配误撤回新目录，随后因已取得Committing而丢失取消权。仅跳过发布后的去重后，重复投递和目录结算30项通过（6.047秒）。继续补充发送端URI恰好等于旧接收路径的情况，红测0.008秒返回DuplicateSkipped，证明下载前也不能按目录URI判断内容重复。目录现只沿数据库及完整内容哈希去重，既不查询也不登记目录URI可见内容缓存；发布后的结果不再进入晚期路径去重。原文本／其他表示的可见内容去重保留，未改协议、ticket或持久格式。

无权威attempt的内部组合在失败捕获时仍会消费DirectoryPublication并只记撤回警告；红测0.007秒失败于关闭提前成功。捕获失败和返回None现在不再以attempt存在为条件：只要有实际发布责任，就交给原fail_directory_receive及原任务修复。权威失败／取消／提交检查统一保留错误退出时的发布责任；若部分结果还带发布责任，先整体撤回而不交付残缺目录，placeholder分支不再消费目录。旧withdraw_publication以及仅供其使用的root_count getter已删除，未增加生产无跟踪旁路、格式、任务、队列或公开Engine接口。

最终目录／duplicate／取消／代次129项通过（11.341秒）；前一轮组合128项通过（11.360秒）。新目录两种URI碰撞都完成两次原捕获、各自两项获取，替代文件读回world，暂存区消失；无attempt用例分别覆盖捕获StorageFull和返回None，撤回PermissionDenied时关闭短等待未完成，解除拒绝后同一原任务清理全部未保存根，无关文件仍为keep。端口模型表达存储状态，实际使用文件系统，不冒充SDK／SQLite同一故障矩阵。全量Application、当前源的真实Engine目录回归与根门禁待追加；首次Staging日志前强杀清理、完整生命周期、来源C、受管1GiB和最终兼容／设备矩阵继续未完成。

本切片最终同源检查：全量Application1295项通过（18.123秒），1项跳过；真实Engine目录成员终态回归1项通过（12.549秒），其余323项未纳入过滤；locked metadata、workspace all-targets（3分24秒）、fmt、Rust style、repository preflight（含隐私与负例）及diff均退出0。人工审阅核对下载前目录不再按URI匹配可见缓存、目录成功只登记完整内容哈希、发布结果不进入晚期路径去重、权威错误统一保留发布责任、部分目录不进入placeholder交付、无attempt捕获失败和空结果都由原任务撤回。四处旧withdraw_publication调用及实现均已移除，未增加稳定接口、格式或后台池。

构建继续单scope串行、jobs=1、共享target／缓存，5GiB内存和4GiB swap上限未放宽，已观察峰值约3.0GiB；最终scope已inactive且无本轮活动编译，专用磁盘TMPDIR为空并回收。真实Engine回归只证明本次源的既有目录终态路径，未把App的URI／无attempt注入算作同一SQLite／SDK故障或设备验收。完整spec保持active；首次Staging日志前强杀清理、完整生命周期、来源C、受管1GiB、最终兼容、平台与设备矩阵继续未完成。

### 2026-10-23：受管Engine连续2×1GiB资源证据

通过稳定Engine操作、真实成员配对、受管Uni／SDK、实际SQLite和磁盘输出连续接收两份不同内容的1GiB文件。HostFileAccess按固定不同字节对生成流，每次最多返回128KiB，不在fixture中分配整份输入；实际命令请求的最大块为64KiB，共32768次读取。接收后只读取小型URI表示，文件本体用64KiB块独立比对固定期望字节，逐份确认长度1GiB和EOF，不调用ReadBlob读取大文件内容。Sampler只保留RSS／FD／线程／当前runtime任务峰值，内存为常数；每轮及关闭后另留检查点，不输出内容、身份或原始路径。生产传输、预算、协议和持久格式未改。

首轮因测试装配误用不存在的saved_count而编译101，不计业务红测或通过；修正为既有items_completed。三个实际运行分别77.078秒、77.736秒、81.319秒通过，首轮原始计数和补强后计数均保留。最终CI入口工件为`.planning/2026-10-21-blob-connectivity/managed-resource-evidence/blob-resources/managed-engine.json`；两轮内容均实际完成并读回，FD均59、当前runtime任务均115；采样RSS峰值436256KiB（约426MiB），关闭后FD15（初值16）、当前runtime任务3（其中1个Sampler）、线程59（初值5）。测试明确只断言关闭计数进入预先设定的FD／任务容差，以及第二轮FD／任务／线程／RSS不持续增长；不能据此声称所有线程或任务归零，也未确认余下两个任务的归属。

完整resource条件仍未达成：实际blob连接数、原许可／服务额度、SDK临时tag及staging恢复计数未取得，工件明确标false；线程回初值也为false。该证据不能替代全部SDK私有runtime任务、生命周期、来源C或平台／设备矩阵，也不把旧SDK约45MiB基线和双完整Engine约426MiB直接作同口径比较。首次Staging日志前强杀清理继续未完成。

用例加入既有host_adapter_contract，只有Linux＋dev-tools编译；既有workspace默认解析由connectivity host启用Engine dev-tools，locked metadata已实查features为default／dev-tools。默认workspace CI因此运行该非ignored用例；nextest明确独占、零重试及10分钟期限。沿既有UC_TEST_ARTIFACTS_DIR生成计数工件，保留显式绝对路径覆盖；本地最终运行只设置原CI工件变量，已验证自动创建目录和写入。没有新建替代测试框架、扩大Engine观测接口或增加测试专属生产阶段查询。

最终同源检查：受管2×1GiB用例1项通过（81.319秒），其余324项未纳入过滤；locked metadata、workspace all-targets（2分13秒）、fmt、Rust style、repository preflight（含隐私与负例）、diff均退出0。人工审阅核对生成式Host有限块、文件核对独立期望、无整份ReadBlob、关闭后计数范围、未知项false标记、工件绝对路径及workspace默认特性。生产代码未改，未重复无关App全量回归。构建始终单scope串行、jobs=1、共享target／缓存，5GiB内存／4GiB swap上限保持；scope已inactive，确认无本轮活动编译，磁盘TMPDIR为空并回收。完整spec继续active，资源缺口及其他未完成矩阵不勾选通过。

### 2026-10-23：关闭后线程诊断

严格大文件107.729秒、无文件最小配对36.097秒、临时无GC最小配对36.254秒均实际失败；30秒主runtime仅余采样器，原GC下iroh类30线程、临时无GC下3线程。生产GC已恢复且node.rs diff为空。排除“大文件才触发”和“只需等待Tokio10秒即可全部回收”；未将尚未取得堆栈的剩余SDK线程写成已证实死锁。完整设计取舍见sdk-store-lifecycle-decision.md。默认原大文件门禁保留，严格线程回归明确known-gap跳过，整体SDK资源退出不计通过；等待固定SDK修订取舍时其他未完成矩阵仍可继续。


### 2026-10-23：获准修订同一 SDK fork，推进完整 store 关闭

用户明确批准同一 fork 生命周期修复，保持协议、ticket 和磁盘格式不变，不再等待该范围确认。新 checkout 位于相邻 `iroh-blobs-lifecycle`，分支 `fix/store-shutdown-runtime`，未修改共享 Cargo checkout。根 Cargo.toml 的本地 patch 仅用于验证，交付前必须删除并固定同一 fork 的新 git 修订。

最小 public Store::shutdown 红测实际失败（2.024秒），GC=false 时返回成功后仍有10个SDK线程；真实2×1GiB与无文件原失败证据保留。SDK改为独立监督线程拥有私有runtime，GC睡眠只保留弱sender，关闭时abort并join GC，停止新命令、推进已接受导入与内部FinishImport、关闭并join metadata，再回收runtime及监督线程后回应原关闭请求。未改线上编码、ticket或存储表。

首个最小回归通过（0.012秒），扩展导入屏障和落盘重开后通过（0.189秒）：GC开／关、通用Store引用仍持有、关闭等待原导入、输出读回均验证。即时/proc线程条目断言曾两次失败（1／0），调试器关闭断点仅见调用方Tokio工作线程、无SDK worker；统一沿原两秒实际退出观察，未取消线程断言。真实双Engine严格回归已取消ignored，当前仍在串行编译运行，不能提前记通过。SDK自有回归尚待单独运行，本地patch尚未交付。


追加实证：新SDK真实双Engine严格退出仍失败（34.446秒），30秒后15线程／初值5，SDK命名前缀线程已为0。gdb先捕获全部runtime创建，再在关闭后的断点取得完整线程栈；多出的10个实际是rayon_core全局池空闲worker，继承了创建方Tokio线程的comm，不能按名称认定为Tokio runtime。证据sdk-runtime-python.log、sdk-idle-stacks.log及sdk-fixed-native-evidence。资源测试改为在基线前只初始化既有Rayon全局池，并同时保存before_shared_cpu_pool及池大小；不预建Engine／Store、不放宽严格退出断言，该校准后的Native尚待运行。

SDK自有公开API回归通过（0.220秒），新增覆盖全部引用自然释放及加载失败的回收。SDK actor任务panic不再被记作成功关闭。SDK文件／GC原有库回归当前串行运行；暂态SDK Cargo.lock使用Engine锁与相同iroh修订，原锁保存在专用TMPDIR/sdk-original-Cargo.lock，SDK检查结束后必须恢复，不能提交这次验证用锁变化。首次独立SDK构建因默认大调试信息而主动终止143，改用与本仓一致的line-tables-only/unpacked后通过；无共享缓存清理或绕过。


### 2026-10-23：正式SDK修订与跨runtime完整负责人

同一fork先提交560d65d并删除根path patch；正式git依赖的SDK停止／故障、协议向量及Store生命周期3项通过（0.815秒），严格双Engine通过33.844秒，连续2×1GiB通过100.929秒。最后工件线程15／正式初值15（共享Rayon10），FD15／16，任务1为采样器，真实字节全部通过；quota、tag、实际blob连接仍为false。新SDK捕获export worker panic后关闭失败，旧可行性探针错误要求成功已改为沿原生命周期port断言Store失败并保留source；首个失败0.478秒保留，不算通过。

最终审阅发现560d65d的确认任务仍依赖加载方runtime；跨runtime实际红测0.376秒SenderClosed。最新SDK修订152c42a9217f1377a998b271e0052311e8ada789由监督线程在私有runtime退出后独立送达原确认，不借用调用方runtime；失败回应也持有至私有runtime退出。跨runtime及其余公开API回归通过0.374秒，原文件／GC20项通过2.997秒。首次green的/proc读取遇到ESRCH，采样器统一按线程已退出处理ENOENT／ESRCH后重跑；没有放宽线程上限。SDK原Cargo.lock恢复且仓库clean，已推同一fork的fix/store-shutdown-runtime分支。Root候选已更新152c42a，正式最终验证仍需全部重跑，不能沿用560d65d的通过作为152c42a结果。


最终152c42a正式证据：SDK停止／故障、rc.15协议向量、Store生命周期3项通过0.909秒；真实双Engine33.624秒、连续2×1GiB91.727秒通过。大文件峰值RSS443520KiB（约433MiB），32768次最多64KiB读取；最终FD15／初值16、线程15／初值15（已记录Rayon10预热）、任务1仅采样器，真实字节与线程归初值标记true。数据在sdk-final-native-evidence及sdk-final-large-evidence，旧实验分目录保留。实际连接、quota、tags仍false，不算完整资源AC。实测空闲结算29.5／28.39秒接近30秒窗口，为CI调度预留至45秒，资源上限及SDK独立2秒期限不变；重跑无文件同helper与根门禁，不重复已经在更严格30秒条件通过的大文件读回。


交付前检查全部通过：45秒观察窗口的真实双Engine同helper回归1项33.796秒；locked metadata、workspace all-targets check（2分27秒）、fmt、Rust style、repository preflight（含1345站点隐私及负例、6项OpenMLS可执行验证）、diff均退出0。手工审阅核对SDK私有runtime独立所有权、成功及失败通道均在回收后结算、观察订阅与导入区分、临时覆盖已删除、同fork不可变修订及原SDK锁恢复、公共接口／协议／格式不变、Rayon预热与冷值、未知资源false和两份文件独立读回。完整spec、最终旧新／新新、生命周期、来源C及平台／设备矩阵保持未完成，不重复无关App全量回归。

所有本轮构建／诊断scope已结束，核对生产node.rs与实验前备份一致、SDK原锁及仓库clean；SDK源码checkout已脱离共享Cargo对象缓存并通过连通性检查。检查专用TMPDIR无活动文件描述符后清理全部本轮失败实验残留，未清理共享target／编译缓存或用户其它planning目录。


### 2026-10-23：最终SDK修订的旧新／新新完整文件互通

代码提交c7997a15660078d31b3f2fd2f19b91518f17d78c、SDK152c42a，current host重新构建4分22秒后验证：开工main a461b894与rc.15 f6f305d9 × Direct／强制Relay × 升级两侧8组，加当前版本双向Direct／Relay2组，共10／10通过。每组旧旧正常配对／重开、独立原资料复制、身份／传输身份／成员／历史保留、原接收文件读回和升级后双向文件精确字节、实际blob握手及预期通道均通过；新新两组使用同一当前宿主重开资料，不冒充旧版升级。全部原始工件cleaned与plaintext_clean为true、failed为false；固定旧库未打补丁，二进制摘要与先前冻结值一致。

私有完整工件保存在target/blob-connectivity-evidence/final-c7997a15-sdk-152c42a-2026-10-23；可提交汇总sdk-final-compat-summary.json只保留固定分类、计数、时间及代码／工件摘要，不含运行身份、地址或路径。逐项脚本断言十组使用固定二进制、相同被测head及源码树摘要、空工作差异。该矩阵只证明独立文件真实互通与旧资料重开；实际连接／quota／tag／staging完整资源、生命周期、来源C、首次Staging前强杀、完整回归及平台CI仍未完成。


### 2026-10-23：真实第三方 ticket 的缓存授权红测

生产装配三 runtime、真实签名 B 消息携带 C 原始文件 ticket，A 正常接收并独立读回。A 撤销 C 后仍将 B 视为 Active；不要求 B 先知晓撤销。原始文件 fetch-to-path 在 A 的本机缓存命中后仍成功写出27字节（真实红测9.86秒），固定分类摘要见source-authorization-13-red.log；此前内存 fetch 得到 Integrity 也说明越过授权进入了不适用于原始文件的解密，不能当作正确拒绝。根因是新请求冻结撤销后的 epoch，Attempt::check_current 仅验证 epoch，不读取当前成员；缓存路径无连接 Grant，因而缺少后者的权威校验。

最小修复仅对既有 Blob Attempt 调用原 owner.check，同时核对当前成员与前后epoch；其他协议保留原策略。无新接口、许可、连接、持久格式或独立任务。测试改为从 A 的真实 IncomingPending 确认作者 B 和原 attempt，再按 entry_id 查询完整终态；目标使用磁盘原始文件，失败断言不输出内容。修复后真实缓存拒绝、B 后续 C 引用 AccessDenied 与关闭尚待验证；额度归属仍未完成。

修复后真实三端回归1项通过（12.74秒）：B 的实际认证消息携带 C ticket 成功接收、字节精确匹配；A 撤销 C 且仍确认 B Active 后，缓存获取的错误链包含原 ConnectivityError::Denied、未留下目标文件，B 的后续 C 引用在原接收尝试中 Failed／AccessDenied；已有文件保留，三端原 shutdown 成功。不把 B 撤销通知未收敛当作已通过，也不以缺少通知延后 A 的授权拒绝。最终源只修改既有 Blob Attempt 的当前校验，未添加 Engine 能力、连接或持久格式。相关 P2P 全库／SDK合同与根门禁待完成；nextest 将该真实三端用例设为独占、60秒期限、零重试。

扩大验证全部通过：P2P库326项／3跳过（183.68秒）；固定SDK停止／协议向量／Store生命周期3项（0.51／0.00／0.35秒）；真实Engine receive_retry24项（221.06秒）；三端授权再次通过10.15秒。locked metadata、workspace all-targets（2分37秒）、fmt、Rust style、repository preflight（1345隐私站点与负例）及diff均退出0。构建单scope串行，观察到峰值约3.0GiB，5GiB内存／4GiB swap上限未改变。最后人工审阅将测试身份比较改为固定失败消息，避免断言在失败时输出随机身份；nextest配置及该最终测试源码待直接复核。旧c7997a15的10组兼容证据保留，但本次生产授权修复后的最终head仍须重新验证，未冒充已通过。

最终源码的CI nextest用例13.965秒通过（其余326项未选中），独占与60秒期限配置有效；最终locked metadata、workspace all-targets（46.569秒）、fmt、style、repository与diff全退出0。测试身份／条目比较使用固定失败消息，不输出动态标识或内容。人工审阅确认真实B消息来源、C ticket及当前成员权威、缓存前后校验、无目标输出、AccessDenied终态、原文件保留及完整关闭；测试只复用现有完整Facade动作和生产Runtime装配，未增加生产Engine入口。所有验证scope已结束，提交该最小授权修复；来源C额度、staging与其余完整验收仍未完成。


### 2026-10-23：提供端 C 的原生额度证明

首个实际三端用例9.46秒通过：原B消息的C获取和直接C获取分别暂停在SDK本地导出完成权；第三C等待，独立B获取完成。该证据还可能同时受Uni每端流额度约束，不能单凭输出顺序证明下载调度许可。因此扩展为保留两项C和一项B的实际完成权，再要求另一项B占第四个活跃槽并完成，第三C依旧不导出。这样，C若错误取得下载活跃许可后只在Uni等待，会挤占第四槽并使该B获取失败。仅扩展既有测试导出索引集合，生产4／2预算与所有者不变；加强后的版本待验证，不把首轮当完整许可证明。

加强后的五份实际文件用例9.69秒通过：C原消息项与C直接获取、B直接获取的三次SDK导出均保留实际完成权，B另一项能使用第四个活跃槽完成精确读回；第三C持续等待且无输出。放开三项原屏障后，原B消息Completed、直接C两项与B项全部完成，实际导出顺序为C／C／B／B／C，直接输出字节独立匹配固定期望，三Runtime原关闭成功。证明来自B消息的C获取按C计入原下载许可；排队C未提前占据全局活跃槽，且不阻塞另一个提供端。既有生产调度无需修改，没有新增生产计数查询或池。仅将原测试单个pause索引改为明确索引集合，旧调用者仍只暂停其原索引；24项既有接收测试、新两项CI nextest和根门禁待复核，实际连接／tag／staging总资源仍未计通过。

扩大验证及最终门禁全部通过：沿原pause索引语义的24项真实Engine接收测试232.76秒通过；CI nextest中加强额度8.363秒、授权13.933秒，两项全部通过，其余326项未选中。locked metadata、workspace all-targets（134.346秒）、fmt、Rust style、repository preflight与diff均退出0。人工审阅核对原B消息来源、实际C／B发布、三项真实导出完成权、第四槽正向交付、等待C无输出、释放后的实际顺序与字节、原完整关闭、旧测试单索引语义及无锁跨await；没有新增生产池、预算、接口或schema。原4／2许可行为已获此原生接缝证明，不替代连接／tag／staging全计数与完整生命周期。所有本轮构建scope已结束；提交证据并核对无活动FD后回收h12，保留共享target／缓存。完整spec仍active。


### 2026-10-23：续接首次Staging前强杀

核对无活动Cargo／验证scope后续接已有未提交的真实kill／restart测试。首轮编译101：crash.rs由path属性加载，子模块须显式指定crash/directory_probe.rs，已修正并串行重跑；不计业务红测。构建沿共享target／缓存、jobs=1及3／5GiB内存、4GiB swap限制，使用磁盘target/blob-probe-tmp避免接近满额的/tmp。源码确认open_publication在Staging日志前创建／探测，现有reconciliation只凭原日志定位旧目录；等待真实结果后前移完整密文映射，保持Staging阶段不认领最终目标的语义。

第二轮真实运行7.37秒：已确认目录探测与SIGKILL，但发送端观察的1253（异步目录暂无同步报告）被测试错误unwrap，尚未到恢复断言，不计目标红测。测试只允许成功或该固定结果，其余失败仍拒绝；真实IncomingPending及探测屏障为已经分发证据。第三轮原源串行运行。

第三轮编译101为新增测试误用EngineError私有code字段，改用既有code()后第四轮8.87秒取得目标红测：真实SIGKILL、旧资料重开与readiness就绪后，旧保存目录staging为1／期望0。原尝试已Failed、当前保存目录已改变；复用用户目标及无关文件均在恢复后独立读回。现将原完整根校验与路径规划前移，先经原RecordDirectoryPublishPort保存密文Staging，之后才创建／探测；候选回退前保留原物理清理责任，不为未发布候选写部分发布事实。首次日志失败改为无文件系统产物、无回退下载并保留StorageFull来源；正在运行目录相关App回归。


最终源全量Application库1279项通过／1项跳过（124.31秒），含新增真实000／500权限下完整目录缓存回退；原63项目录过滤回归12.19秒通过。CI nextest原生三项及根门禁仍在同一scope串行继续。人工复核确认首次密文日志失败发生于任何staging创建／探测前，根映射不捏造发布事实；候选清理受阻仍归原batch，清理成功后才替换日志；仅原保存目录IO不可访问可回退，业务碰撞耗尽不回退；无新schema、队列、许可或Engine阶段接口。


CI nextest真实原生3项通过（40.744秒）：首次Staging前探测强杀／改保存目录／原资料重开11.070秒，既有独立文件进程中断恢复17.176秒，真实SDK目录成员终态修复12.470秒；其余327项未纳入过滤，不计本次通过。新用例确认旧staging被实际清理、原attempt Failed、当前新保存目录持久保持、用户复用目标和无关文件完整保留，发送端已关闭，不恢复旧网络下载。locked metadata已退出0，workspace与后续根门禁正在同scope继续。


最终同源验证全部通过：Application库1279项／1跳过（124.31秒），CI nextest真实进程／SDK三项40.744秒；locked metadata、workspace all-targets（3分23秒）、fmt、Rust style、repository preflight（1346隐私站点、隐私自测、Collector、6项OpenMLS可执行验证及架构负例）、diff均退出0。人工审阅核对完整密文映射先于任何创建／探测、首次日志失败无产物／无下载并保留StorageFull源、000／500保存目录回退、原候选实际清理后替换日志、Staging不认领用户复用的final、原attempt中断失败及不恢复旧网络下载；未新增持久版本、字段、队列、预算或公开接口。

最终scope已inactive，核对无活动Cargo／rustc／Engine进程及专用TMPDIR的活动FD，目录仅余本轮验证脚本并已移除；共享target／编译缓存保持。仅此恢复边界完成：完整资源总量、全生命周期、最终head兼容与既有六片／全工作区回归、平台CI／设备及观测验收继续未完成，不用本次三项替代它们。


### 2026-10-23：恢复完整目标并补资源证据

上一目标轮为实际进展：提交582fb710及真实强杀恢复、App和根门禁通过。当前权威工作区只有四处用户既有未跟踪planning目录，无活动Cargo／验证scope。规格完整验收仍未完成，本轮先补真实SDK临时tag与原生连接回收，再继续其余数量／生命周期及最终矩阵；不缩减完整目标。现有SDK152c42a提供list_temp_tags，原生Connection已有weak_handle；可扩展既有Infra双流隔离测试，无需新增Engine查询或Application阶段接口。


真实SDK资源三项8.47秒通过：4MiB不同内容16轮、32MiB不同内容3轮、4MiB同内容8轮，共27轮；活动下载实际temp-tag峰值分别2／2／1，各轮结算0，原生weak handle关闭后为0，既有单连接／不重拨／不重试均保留。工件在resource-isolation-evidence/blob-resources。正在扩展同一用例为2×1GiB，fixture改为64KiB流式源文件及独立逐块核对，移除与size成比例的Vec／全文件读回；保持同一SDK／Facade／Uni流程和原预算。并沿原ReceiveCase的第四次实际导出屏障加入锁定、暂停期限与重开测试，旧成功项须保留、旧未保存输出须实际清理，失败再沿原owner修复。


流式最终资源四项CI nextest90.320秒通过（大文件85.427秒），共29轮；连续2×1GiB各轮tag2→0，原连接1→关闭后0。实际大文件进程VmHWM52416KiB，独立/proc采样已正常退出，资源scope已inactive。当前uni-blob-lifecycle-native scope串行构建两项原生锁定／暂停测试；完整spec仍active。


两项原生生命周期首轮：暂停／重开10.466秒通过，锁定90.014秒TIMEOUT（整轮100.494秒，退出100）。这是实际Engine／SDK／SQLite在第四次导出仍持完成权时锁定的失败，不能算通过；现将锁定未结算的断言移到解锁前，并对解锁设置20秒阶段断言，定位提前锁定而非用最终90秒超时掩盖位置。源码SpaceSessionActivity.pause_for_lock当前只同步close_receive_gate，然后允许锁密钥，未等待原ReceiveAction真实结算；须先取得更精确红测再修改原负责人。


精确锁定红测5.476秒退出100，原真实导出未结算时LockEncryption已结束，排除单纯外层90秒定位不足。现将原EnsureReceiveReadyPort关闭能力改为完整异步关闭：FileTransferLifecycle先封旧世代，复用原born动作维护排空通知全部停止并等真实结果；Space activity在其完成后才允许后续锁密钥。清空历史及恢复测试统一使用原实现更名的drain_scope，旧clear_scope与无用同步facade转调删除；未加Engine步骤API、队列、持久状态或第二负责人。修复后App1279通过／1跳过（123.24秒），原生四项正在同scope继续。


首版锁定排空修复真实四项全部通过25.597秒：锁定5.957秒、暂停／重开5.541秒、调用者退出删除5.450秒、无保存条目清空8.625秒。原成功文件保持，原未交付输出实际清理，解锁／恢复没有旧任务迟到成功。

为覆盖新增关闭错误转换和世代竞争，现将同一完整排空实现收口到原ReceiveReadinessCoordinator.close，FileTransferLifecycle仅实现原完整关闭port的委托；新增实际registry双动作＋准备动作／关闭期间出生动作测试，以及关闭失败std::Error::source和原StorageFull链与失败门禁测试。不新增构造器、查询或第二实现。当前uni-blob-owner-contracts scope串行运行这两项，再运行最终锁定、暂停、撤销进行中来源、删除、清空5项原生测试；成员撤销新例要求真实当前成员权威先确认撤销，再释放原导出，不靠直接改库伪造资格。未取得结果前不计完成。


最终关闭合同两项0.04秒通过，新增Stop错误保留std source及原StorageFull，真实双接收／准备／关闭中动作均按原世代停止。最终原生5项34.846秒通过：锁定7.751秒、在途来源撤销6.237秒、暂停／恢复5.546秒、调用者退出删除5.008秒、无保存条目清空10.287秒；保留原成功文件，原未交付文件实际清理，撤销后原材料重试AccessDenied且不新增导出。owner-contracts scope已结束。

当前uni-blob-complete-validation scope串行执行全部根门禁、既有workspace组、成员全集和升级smoke；驱动target/blob-resource-tmp/full-checks.sh，状态completion-status.log，各项completion-*.log，工件completion-evidence。先核对scope与活动进程，不能因暂时无输出重启。最终旧新网络／线上CI／完整资源和剩余生命周期／观测核验仍未完成，不把本轮局部通过当作完整goal完成。


恢复核对：完整验证scope仍active，metadata/check/fmt/style/repository/diff全退出0；workspace正在串行编译，不另起Cargo或改变源码／HEAD。独立本地观测scope已启动，官方Jaeger2.20.0与Collector0.160.0二进制已按发布SHA256核验，复用仓库严格Collector过滤配置，仅改loopback端口；两份配置validate退出0，Jaeger/api/services返回200且为空。没有因Docker socket权限不足修改系统权限或请求sudo。真实文件完成／部分失败／重试／取消尚未进入该Collector，不计通过。工具与supervisor均在target/blob-resource-tmp/observability-tools，结束前须停止scope并回收。


本轮观测静态门禁已实际运行：Collector两份隐私配置合同5项全部通过（collector-privacy-current.log），架构隐私清单1346处通过（observability-privacy-current.log）。Jaeger实际浏览器列表已加载，截图observability-ui-initial-ready.png与页面文本可定位；实例无业务记录，仅证明UI可用，不冒充真实文件观测验收。浏览器与CDP捕获均为本机临时工具，无新增系统安装或配置。


在当前Engine测试二进制已完成编译后，只新增显式ignored观测用例receive_retry/observation.rs与声明／90秒独占nextest配置，没有改生产源码。当前完整回归不声称覆盖该新用例，completion-source-manifest.json保存之前tracked diff指纹并明确此边界。原串行驱动末尾已追加新用例的fmt、全部根门禁及精确运行（显式本机Collector端点），不会并发起Cargo；若前项失败则按既有fail-early停止。新例复用原ReceiveCase，两次真实初次部分失败、一次失败项重试完成及一次在途取消／5秒仍未完成／最后实际清理，最终flush；真实trace语义仍需从Collector／Jaeger再独立核对，不能以该行为测试成功替代。


严格Collector本地副本只增加过滤后的file exporter（accepted traces／logs JSONL），processors段与仓库原配置逐字相同并已assert；修改后validate退出0。停止并重启的是本轮创建的空内存Jaeger／Collector，之前启动日志另存initial-stack，未清理用户现有实例。当前观测scope会话51281正常ready；原完整验证scope仍单Cargo串行编译。准备了只读OTLP证据检查器，后续从实际两次file.send的同trace接收结果及两次独立手动重试业务结果核对，检查父节点、test环境及fixture隐私；未产生数据前不执行或计通过。


串行驱动末尾再排入真实Jaeger查询、业务列表截图及OTLP语义核对。JUnit watcher会话63316只读等待并保存每组XML，未运行Cargo；scope会话53955仍编译，当前已从Infra P2P库推进到其blob_store_lifecycle等目标，无动态测试结果。completion-source-files.json记录1892个源码／配置（含原未跟踪drain／lifecycle），显式排除新观测用例；生产源码未再变化。


确认活动bash脚本fd255当前偏移717，恰在尚未读取的membership命令前，原workspace Cargo不受影响。只原位重写未读取后缀：export UC_TEST_BUILD_SCOPE=workspace，workspace后先运行新观测用例／根门禁／Jaeger查询与业务列表，再成员全集及升级smoke，最后语义检查。后续分组复用相同workspace特性图，避免逐包重复编译；保持唯一Cargo负责人、共享target及既有资源限制。当前全量编译已推进Application库。


发现默认CI资源工件覆盖：两个既有Engine资源用例都写managed-engine.json，大文件与空配对关闭会相互替换。仅将默认测试工件分为managed-engine-large.json／managed-engine-shutdown.json，显式UC_MANAGED_BLOB_RESOURCE_EVIDENCE仍保留调用方路径；没有改测试行为或生产实现。当前已编译workspace二进制仍用旧名，资源保存watcher会话34518会按file_transfer_enabled保存两份，成功／失败及原始日志不删除。新命名随排队的根门禁与后续CI验证，不能冒充旧二进制已执行新代码。


新增离线永久通知阻塞原生用例已排入同一串行驱动：只有既有ReceiveExports测试fixture与cfg(test,dev-tools)组合根装饰完整reporter，原source真实shutdown、准确缓存重试、第四次真实导出屏障、原CancelEntryReceive与receiver完整shutdown；调用方不解除永久通知等待。预期只等本地导出并返回Cancelled、保留两份旧成功、清除新输出。原workspace二进制不含新hook／用例；新Native在排队的全根门禁后与观测用例共用workspace特性图运行，尚未计通过。当前完整回归已实际启动4586项，App及多数Engine单元通过，真实空配对关闭33.672秒通过；根门禁仍全0，尚无workspace最终状态。


完整workspace首个实际失败：Native ticket_provider_budget_is_shared_with_a_different_message_author 9.326秒，dispatch原接收者断言失败，随后send JoinError；现场completion-workspace.log约2085行保留，不能记作通过。读源码确认该断言本来就接受pending，排除单纯“发送端等候”解释。暂只加固定计数／匹配布尔诊断，未放宽断言、增加重试或改生产；等全回归结束后精确重现以区分拒绝、离线或目标遗漏。真实空配对关闭33.672秒、大文件2×1GiB94.374秒均通过，资源watcher已保存shutdown报告；workspacescope仍执行其它项且其fail-early会阻止后续串行命令。


当前workspace真实Engine资源：空配对关闭33.672秒、大文件2×1GiB94.374秒通过，逐块精确读回、最大Host返回65536字节、采样峰值RSS446720KiB，关闭空闲后FD15（初值16）／线程15（初值15）／当前runtime任务1（仅采样器，初值0）。两份JSON已由watcher34518保存且watcher正常结束；不再有该后台任务。当前Application1298 PASS／0 FAIL；总回归仍进行，已发现1项Native C额度FAIL，原失败二进制已硬链接保留至target/blob-resource-tmp/quota-original-workspace-test并记录SHA256，供同一被测二进制定位，未覆盖失败现场。


全库另发现两项实际失败：engine_start_builds_a_resumable_real_session在20.017秒被nextest终止（含完整LAN兼容分支，不能未经定位就加时）；dependency_firewall::application_public_whitelists_hide_domain_construction_bundles失败，精确指出facade/blob_transfer.rs测试支持再导出BlobTransferDeps违反门面白名单。后者已做最小边界修复：同一test-support条件下将被动装配类型移至既有deps入口，唯一Infra隔离测试消费者同步改引入，不放宽防火墙、不保留旧转调、不新增公开根或生产能力。须结束当前全库后重跑35项防火墙及受影响blob测试。新Native／观测仍排队，原scope会因workspace非0不继续，其后续需另行恢复，不能假定已执行。


相同预编译dependency_firewall二进制针对当前源码的原白名单测试已精确复验退出0（facade-whitelist-fixed.log），仅证明边界文本合同，编译与其余34项仍须后续检查。额度诊断继续保持原接受／pending断言，仅增加总计数及接收者匹配布尔；精确运行时可显式安装测试OTLP并在退出flush。另将原接收Completed断言移至send JoinError断言之前，两条要求均保留，以区分本地未完成与发送回执失败；未增加重试或变更产品。


完整workspace最终：编译30分30秒，执行1212.937秒；4586运行／4582通过／3失败／1超时／28跳过，退出100。四个失败为三端额度9.326秒、可恢复真实会话20.017秒TIMEOUT、facade装配白名单0.002秒、peer_address堆栈符号1.452秒（unresolved／期望captured）。workspace JUnit已保存completion-workspace-junit.xml；scope已inactive，master会话53955与JUnit watcher63316均结束，资源watcher34518也已结束，未有Cargo活动。后缀排队的新Native、OTLP、成员全集和升级均因原fail-early未执行，必须另行恢复。原严格Collector／Jaeger scope会话51281仍active，目标继续active；不能将根编译通过或4582通过代替完整交付。


首次冻结二进制会话定位未启动被测程序：宿主无/usr/bin/time，诊断scope退出127，错误保留resumable-original-reproduction.log（先复制后重试）。这是诊断驱动工具缺失，不是新增业务失败；改用Python标准库计时与90秒有界等待，不调整产品期限或nextest原门槛。


会话恢复已用冻结原workspace二进制、不改任何产品源码独占有界复验：13.30秒全部断言通过（驱动计时13.382秒，resumable-original-reproduction.log，退出0）；原20秒外层只容纳复杂启停／导入导出／多次解锁过窄，nextest为该具体用例增加独占槽和有限60秒，原所有操作期限及零重试不变，待新配置精确复验。堆栈失败确认是本轮驱动遗漏既有Linux CI环境：.github/workflows/pr-check.yml:222和9月22日原设计已明确available模式，原同二进制按UC_EXPECT_DIAGNOSTIC_STACK=available复验1.54秒通过（peer-address-linux-reproduction.log／evidence.json），源码和断言未改。后续所有Linux分组需带同一既有模式；macOS完整符号门禁仍不可替代。failure-diagnosis scope与会话92007、raw stack会话53651均已结束，无Cargo活动。


已启动唯一后续验证scope uni-blob-focused-validation，exec会话39035，仍jobs1／MemoryHigh3G／Max5G／Swap4G／权重20／原共享target与MBX。当前先workspace全目标check，后精确Engine库与防火墙；保留原功能组合（原Root Engine fingerprint仅default+dev-tools，workspace被动依赖含App test-support、P2P／Storage test-util）。驱动target/blob-resource-tmp/focused-checks.sh、输出focused-status.log／focused-*.log，每组直接保存JUnit；按现有Linux CI设置available模式，未修改堆栈源码。不同时起Cargo或把活动验证判断为挂起。


focused最终：全目标check／style／repository／metadata／fmt／diff全0，Native8项56.042秒全部通过（C额度10.210、恢复会话9.139、锁定8.721、离线永久通知阻塞5.081、撤销5.419、暂停3.721、删除4.031、清空9.705）；显式OTLP的C额度7.553秒通过，原全库C额度失败未重现，不能称根因已修。真实文件OTLP行为15.091秒通过、防火墙35项通过。真实Collector／Jaeger语义实际红测：2个file.send关联接收都被记成ok（实际2/3成功＋1StorageUnavailable），独立手动重试业务记录0（实际一次完成、一次取消）；159个accepted spans／177个logs无缺父节点、test环境及fixture隐私通过，Jaeger38条测试trace含2个file.send且0 retry。focused-status observation-semantics1、scope已inactive／exec39035退出1。基线OTLP、Jaeger JSON、语义摘要与实际列表／详情截图另存observability-red/，不能被后续绿测覆盖。


部分结果第一版真实验证SIGABRT：3.732秒、Rust明确报告worker4073927栈溢出，原core PID4073923；不是测试超时或OOM kill。应用了diagnose-crash技能只读核对，coredumpctl和全部线程gdb函数栈已保存（禁止打印frame参数）；抽取core使用临时私有目录，完成后已自动删除，无新增core副本残留。首次shell式rm清理命令被自动审查拒绝，未执行，改为Python TemporaryDirectory安全清理后完成诊断。新观测scope在worker内移动完整大型处理future是当前推断，现只在原spawn边界Box::pin该future，scope只搬运指针；保留默认线程栈、原任务及全部行为断言，再跑同一真实用例作因果验证。


接续确认：uni-blob-partial-observation.scope仍active，唯一Cargo在原共享target、jobs1和既有资源上限下编译Engine；观测service同样active。已修正task_plan过时Next Step。后续重试记录的完成权须落在execute_owned原监督任务的实际结算尾部，不能用execute_retry早返回的错误或Accepted提前结束；原ReceiveReply可能先回复错误再修复清理，必须保留此行为。Engine仅装饰完整retry能力，Application跨原task传不透明完成上下文，不新建任务或业务接口。

部分结果最小闭环通过：默认线程栈、原真实文件用例14.775秒通过，接收结果partial×2、无缺父节点且隐私通过。Box::pin原spawn边界后未再崩溃；partial-stage-result明确manual_retry_still_required=true、complete_spec_verified=false。独立保存observability-partial/，不覆盖observability-red/。原scope已inactive，开始唯一后续Rust修改。

重试观测首次驱动在fmt解析退出1：新use被插到模块内文档前，尚未执行Cargo测试；已移回模块入口。后续驱动统一retry-observation前缀避免与历史retry日志混名。

重试观测实现已落地：Engine装饰既有retry完整能力，独立在线root，Accepted不完成；原execute_owned监督任务在原处理／恢复结算、finish及drop work后提交固定结果。原ReceiveReply提前返回错误的行为保留，不依赖调用方继续等待。原任务scope只传不透明上下文并在原spawn边界装箱，无新增任务／业务接口。现有IO来源分类器移到完整接收模块共用，不复制分类规则。完整本地export装饰在测试故障装饰之外，原错误和值原样返回。两份Collector及设备白名单同步新固定词表，export不算业务root；5项Collector隐私和1346调用点检查通过。当前唯一scope uni-blob-retry-observation／exec58788，合同库→真实文件OTLP串行，完成后还须实际UI与根门禁。

合同137项0.377秒全通过；Native编译失败E0432，Engine无bytes直接依赖，改用既有tokio_util::bytes::Bytes，不加依赖。首轮错误保存在retry-observation-import-red.log，下一轮仅重跑受影响Native和fmt/style，不重复已通过合同。另注意repository preflight内含OpenMLS Cargo验证，不能在其他Cargo活动时启动；本次曾等待同一target锁，已确认其6项实际通过且无额外编译进程，后续归入同一串行驱动。

重试Native第二轮2.603秒SIGABRT，实际worker4110337栈溢出，core PID4110334。全部线程及85帧当前栈已保存，未打印参数／locals；临时私有core两次抽取均自动回收。此轮并未进入重试：实际栈是joiner持久化→JoinSpace→共同runtime dispatch→原Engine操作任务，资源可用且不是OOM。推断新增未装箱完整retry future扩大共同dispatch状态机，挤占配对栈；现只在Engine既有完整retry调用边界Box::pin该能力future，保持默认线程栈、原任务／回执／期限。下一轮同用例证明因果，不修改配对领域或提高栈。

重试第三轮真实Native15.991秒通过（默认栈、完整配对与实际重试／取消均完成）。初次语义脚本太早读取Collector过滤后文件，仅看到成功root，退出1；原失败摘要保留retry-observation-semantics.log。HTTP exporter flush不等于Collector下游文件已flush，随后不重跑业务测试，只在同一批记录实际到齐后复核：92 spans／103 logs，partial接收2、独立retry成功1／取消1，取消root5106.0532ms；各retry都有实际export＋persist子节点。输出失败2且error.type均storage，8个export全有一致完成日志，无缺父节点、无fixture隐私泄露。Jaeger查询2send／2retry／20testtraces；语义绿摘要retry-observation-semantics-green.json。下一步实际列表／详情核验，然后根门禁与完整回归。

已启动唯一最终候选串行scope uni-blob-candidate-observation，exec86121。候选包括未跟踪源码的逐文件SHA在candidate-observation/source-files.json，metadata／全目标check／根门禁后完整workspace、成员全集及upgrade smoke；不并行起Cargo，不在验证中改源码。实际Jaeger业务列表已查看，有两条独立retry和两条带失败子节点的send，未出现清理／正常shutdown业务root；详情Tags仍需实际截图核对。

实际Jaeger业务列表＋四类展开Tags截图已逐一查看，UI观测核验通过；原始OTLP、Jaeger查询、配置哈希和截图另存observability-green/。仅此观测闭环完成，全部spec／资源映射／最终交付head矩阵和CI仍active。

整分支门禁复核发现4项（branch-style.log）：两个真实薄包装（目录capture固定参数入口、ReceiveAction.finish→finish_shared）与两个检查器误报（is_some_and结果判定被贪婪正则吞掉；ErrorClass::class必须映射统一wire词表）。已停止仅刚开始编译的candidate scope，全部根检查均0／全目标3m39s，workspace未完成不标通过，进程已无Cargo。删除两个冗余入口并迁移全部调用；目录窄输入通过From转换到原完整Inbound入口，接收完成只保留共享source的唯一finish。检查器改为平衡参数边界、只豁免必需ErrorClass词表适配，不豁免其他trait别名；38项真实正负例全通过，含嵌套参数薄包装继续拒绝，整分支风格检查通过。

main预合并只读merge-tree发现3文件冲突（active_plan、UniFFI lib.rs／runtime.rs），预合并不改HEAD／工作区。需在当前改动形成可验证本地提交后合并main，保留第七片结果与主线宿主配置／日志合同；不得让最终GitHub合并门禁必然冲突。原候选哈希只代表中断前源码，后续须新建证据目录。

统一finish首次App编译定位1处类型不匹配：批量迁移误触LocalSettlement自身finish（已经接受Arc<SourceError>），此处不属于ReceiveAction迁移，已恢复原实现；原错误保存在branch-fix-application-type-red.log。其他仅ReceiveAction调用方改用Arc<ReceiveWorkError>，下一轮App与Native验证实际结果。

观测业务与UI证据已全部归档后停止本轮uni-blob-observation.service；其supervisor已回收Collector／Jaeger，保留已验证工具供整合main后必要复验。当前只有branch-fix Cargo scope运行，无额外观测后台服务。

统一完整入口后的App全库实际1281项通过／1跳过，69.067秒；错误链、慢本地清理、目录发布／捕获与取消等原断言全部保留。Native全集仍在唯一scope uni-blob-branch-fix（exec10625）编译，不提前计通过。整分支检查器38项及branch风格均已通过。

branch-fix最终：App1281通过／1跳过69.067秒，Native receive_retry28/28通过265.831秒（包含锁定、暂停、撤销、离线永久通知阻塞、目录和网络终态修复等），整分支style／diff通过，scope已inactive、exec10625退出0。当前源码允许形成可回退本地提交；完整spec仍缺最终全量／资源生命周期／十组矩阵和线上CI，不能归档。

暂存原始证据后git diff --cached --check发现原始patch上下文空行及Jaeger页面文本的表格尾随tab；未改写原始证据。仅在本任务证据目录用.gitattributes声明patch／txt空白为原始数据，源码仍正常检查；随后重新执行暂存检查。

已提交3de9f9c0：完整锁定排空、结果观测、资源与必需CI入口、整分支薄包装及检查器修复，含实际红绿工件。随后执行merge --no-commit origin/main，恰好3处冲突：planning指针保持本任务；UniFFI lib导出合并ConfigImport／RejectedConfigImport与ReceiveItem／Progress／RetryOutcome；runtime导入保留原取消／重试全部类型并新增main的ConfigImport错误别名，未选单边覆盖。已读取整合后的AGENTS；用户会话给定约束仍优先，本切片不新增配置明文例外。待实际绑定全目标编译通过再提交合并。

main整合验证全部退出0：UniFFI全部目标check1分32秒（含Engine及其依赖）、fmt、整分支style、工作区与暂存diff均通过；scope inactive、exec57856结束0。main宿主日志档位仅报告宿主声明，不改变Engine采集／远程合同，已核对更新文档；冲突仅导入联合与指针保留。接下来提交merge，再补活动接收切Space／本机离组等明确未完成验收。

开始剩余生命周期最小验收：完整负责人仍Application Space／原接收owner，调用方只发ResetSpace或远端RemoveMember后本机ChooseDeviceGroup。在真实第四次SDK导出已完成但尚未归还能力时发起操作；确认已有稳定readiness关闭或当前成员权威已撤销，再解除真实导出，要求原attempt中断、未保存输出清理、两份旧成功及路径保留、不重下旧项。ResetSpace还须实际Space身份改变／仅剩本机；本机离组须真实确认并达到Removed。无新生产port／阶段查询／持久字段；原owner负责重启／重试，当前只补测试，出现真实失败再修完整责任。

Space生命周期首轮：本机离组确认6.505秒真实通过；ResetSpace用例4.498秒失败于测试自身unwrap：切会话后QueryReceiveReadiness返回稳定1103／Unavailable。补读完整Engine入口发现SessionSupervisor.reset_space先operations.close_and_wait、quiesce、take旧session并shutdown，再调用App reset并安装新session；此前只读App内层Rebuild不足以判定未封口，未据该推断修改生产代码。新测试接受严格1103＋Unavailable这一既有整体封口合同，继续保留导出未归还前Reset不能完成、真实Space更换、旧尝试中断及成果／清理全部断言。首轮原日志space-lifecycle-query-contract-red.log保留；只重跑失败的Reset，离组不重复运行。

Reset第二轮已通过未归还前不完成、真实Space身份改变、原attempt中断、旧两份成果保留与未保存输出清理全部关键断言，6.323秒时仅末尾QueryDeviceGroupChoices在re_pairing_required新Space返回1213／InvalidState。原既有reset验收使用ListDevices验证仅本机，不要求此时成员选择能力可用；新测试改用同一既有稳定设备列表合同，仍要求新Space一台本机设备。未改生产实现或放宽生命周期／输出断言，第二轮日志和JUnit独立保存；再精确运行Reset。

真实切Space最终5.411秒通过，Summary5.426秒；本机离组此前6.505秒通过，两项完整场景仅新增测试，未改生产行为。最终仍严格验证旧接收不能迟到成功、旧成功文件／路径保留、新输出清理和实际新Space身份，query错误合同现场及JUnit均保留。原space-reset scope已inactive。

开始真实生命周期外部资源切片：复用blob_resources的同一/proc读取，仅在显式UC_RECEIVE_RESOURCE_EVIDENCE_DIR时由现有ReceiveCase记录冷／共享CPU池基线、SDK关闭前后对象释放及空闲收尾。原两Engine实际shutdown成功、原fixture对象Drop后才验FD／OS线程／当前runtime任务回初值，最多45秒沿原SDK资源验收空闲窗口，不加任务或业务接口；报告明确tag／quota／native连接未直接采到，不能冒充完成。六场景选择锁定、源撤销、实际切Space、本机离组、活动删除、真实失败结算后shutdown，默认PR复用已构建的workspace执行此有限组；默认行为回归不启用额外观测等待。工具入口为run-blob-lifecycle-resources.sh，每轮独立目录，保留前一JUnit并清空目标以防编译失败误归属旧报告，各操作内原期限与零重试保留，外层含资源等待的90秒专门窗口。待实测再判定，不提前记通过。

资源首轮六项225.791秒：锁定／源撤销／本机离组／切Space／活动删除五项通过，均FD15／线程17／任务0回原值；失败结算shutdown56.376秒只剩FD16（基线15），线程17和任务0已回值。发现该原测试长期持有read_dir.find得到的generation:DirEntry；Rust本机标准库fs文档明确Unix DirEntry保留原目录FD，路径已经独立后仍持有该测试句柄。现于取得database PathBuf后立即drop(generation)，不放宽资源门槛、不改生产；先保留原资源目录全部数字和首轮日志／JUnit，只精确重跑失败用例证明因果。

失败结算shutdown资源修复因果验证32.953秒通过：只提前释放测试DirEntry，严格FD由16回到基线15，线程17／任务0保持原基线。现六类生命周期均有真实内核资源绿证据；lifecycle-resource-summary.json明确5项首轮＋1项fixture修复后精确重跑，single_six_case_green_run=false，不伪称重跑了全组。首轮失败目录／日志／JUnit原样保留。原生连接／tag／quota直接计数仍单独列未完成，不用内核计数代替。

资源切片已提交214f9ef1，当前无Cargo活动。下一最小资源证明为Application原下载调度器的真实4／2占用及归还：只在既有test-support且显式资源证据env时，由私有scheduler测试模块在原锁内读取真实slots／running／pending计数，记录到该测试目录。调用方仍只执行完整Engine操作，不增加任何facade／port／结果／Engine阶段查询，也不把业务标识、peer key或内容写入工件；生产默认编译不含此模块。先用原三端C额度Native证明峰值4／每端2及最后slots0，再复用大文件与生命周期路径；正常功能与观测词表／协议／持久格式不变，重试／恢复仍归原调度owner。

实际下载额度最小切片通过：调度8项0.100秒全通过；原三端C提供端Native9.583秒通过，真实5个scheduler实例中2个实际工作。直接计数主实例峰值active4／peer_active_max2／queued1，最终closed=true、event=settled、active=queued=slots=0；另一实际工作实例峰值1并归零。summary在quota-counter-evidence-1/summary.json，不把该证据误称提供端服务池或SDK tag已验证。原完整workspace C额度失败仍未复现、未宣称根因已修。默认PR资源入口现包含6个生命周期＋原C额度场景，并独立校验实际private计数；零重试与业务期限保持。scope已inactive、exec63101退出0。

下载额度已提交a8684f3b。下一SDK资源最小证明：仅test-util＋显式资源目录时，Infra原生连接适配器保存SDK公开weak_handle，不持有native强引用、不创建采样任务；Engine的测试私有/proc采样同时读取Infra测试支持的技术句柄总数／存活数，不返回连接或业务对象，不新增稳定Engine／Application／Core查询。原SDK shutdown完整能力在所有工作结算后、store关闭前直接list_temp_tags计数并保存纯数字；只在测试功能存在，不改变协议、回执或生产默认路径。先复用原六生命周期＋C额度场景确认真实已注册句柄并归零、store仍打开时tag为0，再补2×1GiB同一路径最终证明。

- SDK直接资源最小切片通过：真实Engine锁定场景33.833s，原生端侧弱句柄登记2／空闲存活0，3次store仍打开时临时tag均0，实际使用的下载scheduler终态settled／slots=active=queued=0。scope uni-blob-sdk-resources已inactive；原始JUnit与计数保存在sdk-resource-evidence-1。现在把直接SDK断言接入原默认资源脚本，独占串行运行六生命周期＋三端配额七项，不修改源码直至结束。

- SDK资源默认脚本完整七项210.976s通过，六生命周期均FD15／线程17／runtime任务0，真实native弱句柄最终0；24次SDK actor仍打开时temp tags全0，所有实际使用下载调度器均settled且slots／active／queued=0，三端场景实际达到4／2。原始完整JUnit、txt和sdk-lifecycle-evidence-1聚合报告保留；scope已结束。下一步复用相同test-util计数补Engine连续2×1GiB，没有扩展生产API或流程步骤。

- 18569132提交SDK全生命周期直接证据。大文件最小变更复用原测试采样，额外记录每轮实际弱句柄、关闭前后计数，空闲FD要求回初始值；显式验收必须真实登记并最终无存活。默认原资源脚本加入原2×1GiB测试，仍为既有Engine测试／nextest，聚合真实SDK／quota工件；修正默认相对证据目录为绝对路径，防止nextest crate cwd导致CI工件写散。唯一Cargo将运行默认八项，保留相对输入以验证目录修正。

- 八项默认验收真实复现C配额Native失败28.994秒：receiver accepted1／errored1，原接收最终failed／0／StorageUnavailable。三端调度严格断言未放宽，原始输出立即归档quota-reproduced-red-1/native-at-failure.txt；原scope继续其已选择的大文件等测试，源码冻结。按diagnosing-bugs使用同一测试二进制和已有显式OTLP，区分SQLite持久提交／SDK导出或本地IO／终态记账竞争，不能把focused绿结果当根因修复。

- 冻结同一二进制的OTLP首轮再次复现相同failed0／StorageUnavailable，文件导出与两次persist子记录均ok；两轮gdb确认错误在原owner交接，但debuginfo=1只有行信息，无法读参数。另见failed1／无项错误的父失败变体，不能混称同一根因。当前临时test-support来源探针仅记固定布尔／IO kind和原source chain长度，不记录错误正文／内容／路径、不改变返回或接口，修复完成前删除。首轮fmt发现探针import插在workflow内doc之前，已移动到doc之后，原解析错误保留quota-source-probe-fmt.log（将归档）。

- 来源探针第一轮实际再现failed0／StorageUnavailable：file_save=false、file_progress_commit=false、各InboundReceiveCommitError固定分类均false、io_kind=null、source_chain_length=9；这不足以断言数据库或SDK根因。JUnit quota-source-probe-junit.xml，raw输出另存quota-source-probe-native-1.txt。继续同一私有probe补FetchTerminalFailure／FileTransferApplicationError／Projection类型和原anyhow创建堆栈；堆栈只保留函数符号，不输出路径或错误正文。当前唯一Cargo uni-blob-quota-source-probe-two.scope（exec57735），RUST_LIB_BACKTRACE=1，输出quota-source-probe-native-2.log、quota-source-probe-2/，源码冻结。所有probe最终删除；保留首轮red和原文档green。

- SQLite分类Native首轮9.965秒通过，冻结同一二进制压力第5轮红（前4绿）：真实sqlite_busy且原FetchTerminalFailure／Store链9，锁竞争不再只有推测。原临时探针源码与补丁已保留，最终实现删除全部探针并只把四个受管写事务改为immediate，保留receiver只读准备事务。下一唯一scope执行同一C Native、冻结12轮压力及默认八资源，保持原业务期限和断言。

- 原C Native修复后11.084秒通过，冻结修复后二进制12轮全通过，原断言／业务期限／零CI重试不变；当前默认八项资源已启动，C项9.755秒再次通过，尚待其余7项。原C问题已由失败来源与单变量四写事务修复的前后运行取得因果证据，不再把单次focused绿冒充根因。临时probe删除，原probe源码／红记录保存。诊断Collector／Jaeger service已停止，完整压力OTLP归档quota-sqlite-fixed-stress-1，根旧green JSONL已按HEAD原字节恢复。

- 修复后默认八项7绿1红：C9.755秒、大文件93.874秒及其余五生命周期通过；撤销项3.839秒在原member_id准备查询因1211／Unavailable／retryable=true立即panic，尚未开始接收。原稳定QueryDeviceGroupChoices代码及public_query_errors_distinguish_retry_from_user_recovery测试明确1211需重试、1212／1213不可重试。仅修测试准备helper：原30秒内继续查询可重试Unavailable，保留原1103等待与Active＋至少一个Active远端断言；永久／恢复错误仍立即失败，全部业务期限不变。原失败与JUnit／partial-summary保留sdk-large-evidence-2。下一默认八项重新验证修正后的真实fixture，不重复已独立12轮C压力。

- 最终默认八项全部通过：三端10.130s、连续2×1GiB95.126s、六生命周期34.209／33.829／34.000／36.224／34.054／33.896s。六类真实FD／线程／任务回基线，27次SDK仍打开时临时tag全0，native端侧句柄最终无存活，全部实际下载scheduler终态settled且slots／active／queued=0，主场景峰值4／2。大文件峰值RSS452576KiB，关闭空闲FD15／线程15／任务1仅采样器，native2→0；原始工件sdk-large-evidence-3/和final-resource-native-junit.xml。原两次八项red均保留。scope uni-blob-final-resources已inactive，临时probe源码全删除，人工审阅确认仅四写事务提前获取写权、只读准备及原CAS／source／加密规则保持，准备查询仅依稳定可重试合同在原30秒内等待。下一步提交具体候选、完整根门禁及workspace／成员／升级／交付head网络／平台CI。
