# 文件／blob 受管传输实施

## 目标
按 spec 与阶段计划完成第七片；阶段 0 三项证明失败或无法证明时停止生产迁移，记录设计阻塞。
完整负责人为 Application transfer；调用方只提交完整接收、重试或取消。成功须实际停止／提交并结算，自动恢复归 transfer，重启归既有 reconciliation。

## 阶段
- [x] 阶段 0：冻结 main／历史端／SDK，盘点调用图，验证通用流与停止、重试材料和兼容授权。
- [ ] 阶段 1：规则、调度、通用流及专属能力。
- [ ] 阶段 2：真实下载／提供、取消和结算。
- [ ] 阶段 3：部分失败、手动重试与绑定。
- [ ] 阶段 4：唯一组装和删除旁路。
- [ ] 阶段 5：矩阵、回归、审查及提交。

## 验证接缝
沿 spec 已明确的 Uni 通用流／SDK 真实双向边界、Engine 接收／重试／取消边界和原始资料兼容边界验证。

## 当前状态
阶段 0 前置证明完成：SDK／Uni 完成等待、原始材料、当前授权、同内容隔离、协议向量、8 组旧资料矩阵与 1GiB 基线均有证据。阶段 1／2 推进：生产获取和提供端已接线，输出／解密及引用／tag 受监督；完整条目结算、部分失败、稳定重试与薄绑定继续。

## Next Step
NoEntry提交回执丢失的只读确认切片已完成：Core 2、真实SQLite／AEAD 29、App目录结算7、全量App＋file_transfer1285（124.681秒）及Native receive_retry23（228.790秒）通过；1项默认ignored未执行，Native其余300项不在过滤范围。locked metadata、workspace all-targets（3分30秒）、fmt、style、repository及diff通过，所有scope已结束，提交后核验专用磁盘TMPDIR已回收。下一项用真实Engine目录成员Completed写入拒绝验证父Receiving及实际终态交接；原fixture的SendFiles不支持目录句柄，应经可变HostClipboard inline真实目录URI＋ObserveClipboardChange，候选入口与角色证据见findings。随后继续成功staging清理、open/probe失败、其他撤回入口及生命周期、来源C、受管1GiB、最终兼容／设备矩阵。完整spec仍active。

## 先前切片记录
业务计数切片完成同源全部门禁：五crate库2244通过／9跳过（282.213秒），metadata812包、workspace全目标check3m41s、fmt／style／repository／1350隐私／diff通过；34 App、4 Native及33真实SQLite／AEAD契约通过。已提交b3cacddf。保存失败＋物理清理拒绝已复现原保护0／期望1及原source覆盖；同一ReceiveAction已接线清理恢复，25 App回归通过（25.148秒），包括原路径复用保护。保存后物理清理已提交01783244（App25／Native5及根门禁）。当前未提交获取失败／取消清理恢复：fetch-cleanup-red实际Facade保护0／期望1（0.007秒）；恢复意图区分FileCommit、FetchFailure、FetchCancellation，只有FileCommit写FileFailed，其余按原网络事实和实际journaled完成NoEntry；取消通过原CAS争取权威。原本只检查错误的mock测试由实际Facade／资源／文件系统／取消源检查替代。首轮25通过／2失败：一为旧错误类型断言，一为新测试错要求关闭吞掉Store；依据原supervision的错误保留合同，测试现明确断言Store和StorageFull仍存在，不改生产关闭。新增Native第二项实际导出失败＋目录清理拒绝，用原结果事件确认失败已返回，再验证100ms关闭、旧成功文件、原Failed网络事实及重开日志。

前一最终Native真实复现Unknown／StorageUnavailable（新增8.228秒失败，其他5项通过，53.704秒；原App26通过27.189秒）。现仅补充获取失败／取消的批次业务事实及来源，不结束LocalSettlement、不移动资源；保存失败仍等FileFailed持久事实。App增加实际清理等待中的查询分类，原失败断言不删。

获取失败物理清理与分类已提交f4cbc487（App26／Native6及根门禁）。当前FileCleaned原子交接切片：fetch-ack-red实际输出已删除但原保护／原获取source为(0,false)，应为(1,true)，0.008秒失败。新增原完整提交port的FileCleaned，Core统一判定当前开放attempt、唯一文件、未交付及匹配网络结果；Infra认证AEAD并在同一事务比较与移除本项，原Failed／Cancelled网络字段不写。普通获取清理走该能力，失败由原ReceiveAction保留原Source和资源继续修复；准备失败独立PreparationFailure，不对未登记文件做网络结算。没有新schema、池、预算或Engine接口。

Core／App29项通过28.178秒；Profile编译发现状态枚举无FromStr，已改用原from_str_value，未知状态不补造默认值。唯一事实读取增加limit(2)，新增实际SQLite重复／未知记录拒绝测试。

真实Profile41项通过1.748秒，Core／App29项通过28.178秒。审查发现网络登记失败后可能只有认证产物日志而无网络记录；不能把该合法准备状态与冲突／未知记录混为None并永久拒绝清理。新增实际SQLite缺失网络记录清理红测，以及真实Engine第二项登记被SQL拒绝、其余两项交付的最终验收（Native尚未运行）。

合法准备状态的实际SQLite红测0.072秒被原规则错误拒绝。Core现明确Missing／Tracked／Unusable：只有清理允许尚未登记而不补造网络记录，本地保存失败仍须Completed，未知／重复／活动／已交付记录继续拒绝。Native新增定向SQL登记拒绝并验证第三项正常交付、失败项可通过原稳定重试补取且保留原成功文件，尚待实际执行。

FileCleaned已提交da77bea0（Core／App30、Profile42、Native14及根门禁通过，完整日志fetch-ack-final-*）。当前新增普通partial重投Native红测：初次原第二项失败且首／三项成功，再发原SendFiles，第四次真实导出必须属于原失败摘要，最终只多一次导出且原成功路径／字节保持。使用既有IncomingPending和完整结果事件，避免消费旧结果；不以新数组位置冒充旧成果。

唯一活动scope uni-blob-redelivery-reuse-red、PID1998918；日志redelivery-reuse-red.log／.exit.log。先检查实际进程与终结，不重启活动构建、不修改Rust。之后读取现有来源加载、票据摘要能力、保存绑定和原始位置规则，再实现完整来源映射及普通逐项保存，不能简单删除progress禁用条件。红测已实际失败100（8.300秒，Summary8.322秒）：第四次导出是已成功内容。该scope已终结，生产映射尚未修改。RetrySnapshotReader和PreparedReceiveRetry／accept_retry／BeginReceiveRetryPort已有完整原正文读取、成员／身份／CAS及新项继承，可共享准备能力，普通投递仍用原出生ReceiveAction，不能递归另建owned任务或只开progress。准备／最终交接的非终态写入失败、表示／目录、全生命周期、受管1GiB和最终head矩阵仍继续。
后续保持完整spec：普通partial重投映射与重启材料完整性、保存失败后的物理清理失败责任、准备拒绝交接、目录／表示完整预算、来源C、完整生命周期、受管1GiB及最终head网络／CI／设备矩阵。未运行设备仍记跳过。

资源约束持续有效：用户设置16 GiB swap并明确resume；Cargo串行、CARGO_BUILD_JOBS=1，临时scope MemoryHigh=3 GiB／MemoryMax=5 GiB／MemorySwapMax=4 GiB及CPU／IO权重20，保持共享缓存和仓库target。先核对真实活动scope／PID与退出状态，不得重启仍在运行的验证。

## 错误
- 探针锁 guard 编译错误已修复，probe.log 保留。
- 重复关闭 provider store 已修复，probe-fixed.log 保留。
- 扩展 sibling SDK get 的 error 3 已定位为探针把 Raw 错用为 HashSeq；修正 GetRequest::blob 后双流通过，保留首次失败日志。

## 本次交付
- [x] 基于最新 origin/main 创建分支，保留原有用户改动。
- [x] 形成 SDK 停止前置失败的最小复现与原因、替代方案。
- [x] 根门禁通过；完整 workspace 已执行并保留 4305 通过／1 失败／27 跳过，未假报通过。
- [x] 完成手工差异审查；code-review skill 缺失已记录。
- [x] 交付阶段 0 原型变更至 feat/blob-connectivity；SDK、材料、授权、协议、旧资料矩阵与资源前置均已证明，阶段 0 完成。

## 续接
用户目标仍为完成 spec。阶段 1 调度与预算已落地，专属交换 port、实际 owner 与生产迁移继续；协议、格式、原预算与完整验收不缩减。

- 当前错误来源切片：完整负责人仍为 receive/materialization，调用方仅提交计划；目录表示获取失败或取消须返回原始 source，且不启动目录成员、不发布目录。重试／重启责任不变，沿用 attempt 恢复；使用真实目录及注入获取失败验证原始类型、IO 原因和取消分类。

- 取消绑定完成标准：宿主只提交条目＋attempt或具体transfer；原Application负责人执行停止／结算，绑定仅映射同版本Engine操作及固定业务结果，不能把五秒未完成当作已取消。复用现有完整取消回归和绑定结果区分验证，无新持久状态。

- 来源保留的前置生命周期切片：readiness拥有同一接收运行世代与工作表，门禁关闭取消旧世代且旧action不能在重开后登记；恢复只可打开其原世代。旧超时扫描不得终结／删除仍由完整接收动作持有的条目（包括子下载已结束、慢预留／保存阶段）。调用方仍只调用既有关闭门禁／完整接收，持久attempt仍为权威，重启责任不变。验证封口前后登记竞争、关闭与恢复竞争以及完整动作结算前后的扫描资格，不添加持久格式。

- 当前来源／结果切片：receive/work拥有原始来源与批次结果，调用方仍提交完整接收、既有查询和删除。attempt接受后才保留原V3 Bytes及最小来源；保存后释放原包正文，全部成功不保留重试来源。查询按原始清单补齐未启动项，只有真实保存的完成项可报告交付完成。关闭清除来源；删除通过同一registry封住已开始的旧动作并等实际停止，不允许旧任务重建。重试仍由receive负责，重启无内存材料明确不可用，不新增持久格式。验证真实部分接收／保存失败查询、原包释放、删除／关闭与迟到登记竞争，以及全部删除端口生产接线。

- 当前选择物化切片：完整负责人仍为receive/materialization；计划保留完整原始blob数组，以原始位置选择重试并携带已保存项的输出／摘要证明。成功项重新核对内容和所有权后只复用，不重新获取，不把其输出登记为新attempt可清理产物。未选失败项仍缺失，目录必须完整单元；失败／取消只清理新owned输出。真实文件证明选择原位置、同名碰撞隔离、原成功不重传、篡改／所有权变化拒绝，原格式与重启责任不变。稳定RetryEntryReceive与CAS仍需完整流程接线，不以计划接口测试替代。

- 本轮来源提交切片：receive/work唯一拥有已保存来源与候选来源；调用方仍提交完整接收／重试。仅persisted确认capture成功后切换recipe；普通重复投递被保留旧partial、保存失败或未提交新记录时，保留原来源／输出证明并按原清单生成当前attempt选择身份，不把新数组当旧成果。旧动作未实际结算不得开始新投递。关闭／删除丢弃候选和旧来源，不允许迟到finish复活。无新schema，重启仍沿既有材料恢复；验证完整普通接收及随后手动选择、两次候选、删除／关闭和成功替换。此步不宣称已完成新旧成果的内容映射合并或历史持久恢复。

- 当前实际协议证明：完整收发仍归transfer，原Endpoint连接观察者只在既有握手完成入口输出固定协议类别。成功必须在真实文件流程看到blob协商记录及精确双向内容／旧资料保持；未知ALPN仍只归other，原文拒绝。本地诊断不新增业务root或Engine内部查询，无新持久格式；连接观察关闭与重试责任不变。先验证封闭编码、实际native握手和真实导出，再用旧新Direct／Relay完整资料矩阵验证。平台／资源／业务可视化及完整生命周期不由本局部记录替代。

- 真实Engine重试切片：完整接收及重试仍归Application transfer，调用方只使用SendFiles／查询／RetryEntryReceive。测试组合根对第二个真实文件导出注入一次StorageFull，其余所有能力原样使用实际实现；保存两项成功并报告一项存储失败，随后只重试失败项且保留成功文件的字节、路径和身份。SQLite与既有加密／CAS负责保存，不新增生产接口或持久格式；重试／关闭／重启责任保持。先验证完整基本流程，再扩大重复点击、取消及权限变化，不以fake原子提交代替实际Engine验收。

- 本轮未保存条目删除：完整负责人仍为Application history删除动作，调用方仅DeleteHistoryEntry。删除先由同一receive registry封口并等实际停止，再通过唯一原子删除能力清理保存条目或NoEntry的持久接收状态；真实缺失返回NotFound，旧非终态及未清理产物不允许跳过屏障。仓储在同一事务内解析实际event及是否存在条目／接收状态，调用方不拼接。复用既有schema、启动reconciliation和Engine operation监督；先用真实SQLite验证NoEntry成功、pending拒绝、原子回滚和重复删除，再验证Application完整删除及稳定Engine调用者退出。删除不会清理用户已拥有文件。

- 本轮ClearHistory完整负责人为Application history清空动作；宿主只发ClearHistory。先封住接收登记，同时通知所有出生动作停止，再收集实际结束，之后统一枚举持久历史／attempt／孤儿状态并调用唯一原子删除。清空期间及之前出生的旧动作永不复活；失败保留source及封口，调用者退出继续沿Engine operation监督。无新schema、无Engine步骤查询；清空成功只解开自己的屏障，不打开外部关闭的会话。先用真实Engine首项导出屏障证明无保存条目也必须等待，再接线完整能力，覆盖并发停止、未登记准备、错误与重开竞争及SQLite目标集。

- 当前认证读取切片：完整删除仍归Application history，宿主只DeleteHistoryEntry／ClearHistory；Infra列表能力必须返回全部认证表示或携带原始密码错误的失败，禁止把密文当业务正文。无新schema或协议；失败保留条目、接收状态及清理资料，用户修复资料后仍由原删除动作重试，重启责任不变。先验证批量拒绝及完整source，再用真实AEAD／错误AAD与实际删除证明不发生破坏；既有清理失败持久恢复仍需后续完整验证。

- 当前删除所有权切片：完整负责人仍为Application history；调用方仅执行原删除／清空。认证表示中词法落入缓存的路径必须先验证实际受管归属，父级逃逸或借用符号链接不能删除用户文件，也不能先丢数据库记录。原cache配置仍为唯一根，无新port／schema；失败保留原记录供修复后重试。先用实际文件系统复现，再验证内部文件／外部文件／缺失文件与真实Engine正常删除。磁盘清理失败持久恢复另由完整删除准入设计继续，不直接反转SQL顺序。

- 当前完整额度切片：唯一transfer负责人持有同一下载调度额度至本地实际结算，调用方仍仅提交完整接收／获取。首先通过Facade最终本地进度屏障验证每端2项是否提前归还；随后沿接收batch单项结果保留同一额度至本项输出／清理交接，禁止通过保留整批许可造成多文件死锁。不增加第二个调度器、预算、schema或Engine步骤接口。失败保留原source，调用等待退出不释放真实任务持有的额度；准备阶段整体60秒／完整admission仍须后续扩展，不把这一屏障当全预算通过。

- 当前准备准入切片：transfer下载负责人先原子入队并取得原许可，再启动Facade lifecycle／初始进度；计时沿同一permit.started_at覆盖准备、身份解析及首批数据。调用方仍只提交完整fetch／receive；准备future必须归原DownloadTask监督，取消／超时先封口并等已开始的准备实际结束，失败保留source。inflight登记与启动同一临界区，不能让重复请求先碰到旧session。无第二池／预算、无新schema或Engine步骤查询；materializer更早目标预留、目录发布及最终交接仍需继续完整验证。

- 当前目标准备切片：完整负责人仍为Application transfer；materializer只提交带owned目标准备的完整获取，原DownloadTask取得同一许可后才预留目标、记录产物并准备lifecycle。成功使用本次实际预留路径；准备失败保留source与已创建产物事实，沿原接收流程清理，清理失败不得当结算成功。取消／deadline等已开始准备实际结束，不启动后续网络；排队拒绝不得创建文件。不加第二池、schema或Engine阶段查询。先用完整materializer＋真实Facade屏障复现同端第三项目标提前预留，再迁移单一文件获取契约及测试模拟边界；重试／重启沿既有attempt与产物日志，不扩大持久格式。

- 当前最终交接切片：transfer仍为唯一完整负责人，调用方只提交完整fetch及取消。最终进度、终态事件及错误结算须在原DownloadTask里完成，inflight直到完整交接结束才可复用；取消／等待方退出不能遗弃已开始的本地回调。保持同一许可、来源错误和五秒调用期限，不增加监督池或Engine阶段接口。先以实际Facade最终进度屏障验证重复Busy、取消未完成及调用者退出后重复取消，再统一内存／文件结果交接。父流程产物清理与最终保存仍沿完整接收owner继续验证。

- 当前父保存切片：ReceiveAction为整个接收负责人，调用方仍提交完整接收／重试；下载返回后单项交接须保留原持有者至父保存或清理真正结束，多文件下一项不能因积累许可死锁。先以实际ApplyInbound重试＋真实下载调度／文件写入替身＋保存屏障复现提前释放，再验证调用者退出、关闭五秒未完成和7／9项顺序处理。失败清理恢复移交及下一项排队取消时的资源连续性单独保留为未完成，不用普通保存屏障代替全边界证明；无新持久格式、池或Engine步骤查询。

- 当前未保存计数切片：完整查询负责人仍为Application ReceiveProgressQuery，调用方仅提交原接收查询。Core已有group_receive_items提供业务状态，Infra聚合应使用其结果，不能把raw网络Completed当已保存。无新schema／状态词表／Engine查询步骤；失败保留原来源，重启／重试继续读取现有事实。首先在真实Engine双SQL故障和原资源屏障中核对一项已保存、另一项raw Completed且failure_code为空时的项数／字节，再覆盖实际SQLite重开与目录完整单元，普通partial来源映射仍须独立完成。

- 普通已有partial同进程重投复用：本切片已通过真实Native正常／顺序变化重投、136项相关Application和根门禁，准备提交。继续核对完整接收后的普通重复投递；原非终态网络记录的终态交接修复、目录／表示完整预算、来源C／生命周期／1GiB与最终head矩阵等仍未完成，完整goal active。

- 当前完整成果重复投递核对：完整负责人仍Application接收流程，调用方仅SendFiles；完整可用条目应直接重显，无新attempt／下载／清理责任。先在真实Native补齐partial完成后第三次SendFiles，用接收结果、原attempt及文件身份／导出数证明，不以普通partial绿测替代。若availability误读密文，修复唯一实际能力并保留认证错误来源，不引入明文例外或新schema；重试／重启沿既有契约。

- 完整成果可用性切片：真实错误重复投递已修复并通过3项Native、149项相关合同和根门禁。后续先以真实SQLite trigger复现网络终态store写入失败后的资源／清理修复；实际publisher返回Ok，不把其测试替身行为误认为SQLite根因。完整goal继续active。

- 网络终态持久修复：以真实第二项StoreFull与failed终态SQL trigger取得端到端红测；先保持原网络事实／活动许可，再补记同一终态和父清理责任。须覆盖完成／取消／准备阶段，继续保留原5秒等待与源分类，不增加监督池或放宽FileCleaned。

- 原终态事件／projection恢复（独立文件）已完成该切片：1275项App全量＋独占诊断1项、Native接收重试20项与根门禁均通过，准备提交。下一步从真实Native初始Started提交拒绝检查已形成pending row却未交出session的责任；表示／目录早返回后的父结算、原网络key范围和全部生命周期等继续，不标全spec完成。

- Started准备已登记Pending责任：真实Native第二项Started更新拒绝、无第二项导出、关闭未完成和解除拒绝后补记Failed的端到端红测；验证经用户spec／plan批准的稳定操作与SQLite契约。原slot持续持有，不重发内容；不加持久recipe、池或业务阶段Engine入口。

- Started切片最终验证：Native receive_retry 21/21通过186.083s，相关App27/27通过39.985s；locked metadata、workspace alltargets3m07s、fmt、style、repository、diff均退出0。受限串行scope已退出，源码人工审阅完成；下一步表示／目录父结算，完整goal保持active。

- 表示完整结算切片：真实Engine SendImage触发96KiB表示blob，SQLite拒绝原Completed终态，核对父尝试在真实屏障前保持Receiving、关闭期限及解除拒绝后完成。原接收动作承担元数据修复，不重发内容，不新增格式／持久字段／池或Engine阶段接口；目录仍需独立全单元清理验收。

- 表示终态切片最终：Native1项11.502s、相关App124项45.721s通过；metadata、workspace alltargets3m07s、fmt、style、repository、diff均退出0，scope已结束。等待原信号前记录已知StorageUnavailable，父状态Receiving直至真实终态及业务结算完成。下一批集中验证父NoEntry持久失败／目录成员／物理回滚和原保护生命周期，再统一最终矩阵；完整goal active。

- 父NoEntry提交责任：预先确认的稳定Engine发送／查询／关闭和真实SQLite契约接缝；原接收动作负责保存失败后的NoEntry本地重试，只交接原失败，不重下或重存内容。先Native双SQL拒绝取得红测：解除NoEntry拒绝但继续拒绝capture，必须由原任务补齐Failed。后续相关App合同验证完整双来源及原保护／4/2许可，目录回滚另沿完整责任推进。

- 父NoEntry表示保存失败最终验证：App＋file_transfer1276项116.667s、ignored独占诊断1项0.05s、Native receive_retry23项227.407s通过；metadata、workspacealltargets3m10s、fmt、style、repository、diff均退出0。scope实际inactive，n5检查为空并rmdir，本轮所有新临时目录均已回收。仅此边界完成，目录及最终全矩阵保持open；完整goal active。

- 目录保存失败后撤回责任：从预先批准的完整Application接收和实际文件系统port接缝验证，先完整发布两个根再拒绝保存与撤回；解除撤回拒绝后同一原动作必须完成撤回／NoEntry并保持无关文件。错误只携带来源，输出责任不放入错误；原许可持续持有。此处先取得实际红测，之后再处理发布中途失败、staging清理、日志与重启／重用边界；不新增持久格式或Engine阶段接口。


- 当前目录准备失败切片：完整负责人仍为Application接收流程，调用方只提交完整接收；原DirectoryPublication从staging创建后即持有输出，覆盖成员获取、组装、阶段记录与发布失败。清理失败保留原来源和未完成责任，关闭须等待实际清理；原任务只重试本地结算，重启沿已持久的Staging日志。先以完整接收＋真实文件系统权限拒绝复现早期清理失败被丢弃，再统一原持有者并验证权限恢复、关闭屏障、NoEntry及不重复获取。成功后staging清理及open/probe失败仍须继续完成。


- NoEntry回执丢失切片：完整接收及本地重试仍归Application，宿主只提交原接收／关闭；Infra原子提交能力须只读确认同一条目、同一尝试和同一终态已经完成的NoEntry。RolledBack重放还须确认原产物日志确为settled／rolled_back，不重新写终态或覆盖已交付／活动产物。不加格式、Engine步骤查询或后台任务；原任务仍负责资源至实际确认。先用真实SQLite执行器在事务实际成功后返回一次IO错误，证明重放行为及拒绝边界，再验证完整Application原动作能结束。
