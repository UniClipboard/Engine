# 发现

- 远程 main、当前 HEAD 和 spec 基线一致：2f9afc9956b0cda82e65c912f0c0f6dc6aa51256。
- git archive HEAD SHA256：44d65418195c835f5ab8126a053000d10509332f918aefd50083ac014d283912。
- 当前分支 feat/active-clipboard-connectivity；原 main 无生产改动。没有其他 Cargo/rustc/nextest 活动进程。
- golden_vectors 已固定状态与 pull 正常字节，非法样本已有部分单测。
- connectivity host 缺少恢复命令；HostClipboard::write 丢弃写入，必须先改测试宿主才能证明系统最终应用。
- 固定成员历史脚本 rev 保持不动，本片另建固定旧端入口。

- Uni ConnectionLease 已有 closed() 通用能力，可保持原关闭保护而不扩大原生类型边界。hold_for_work 保留流额度；默认 inbound_per_connection 为 4，本片需显式设置为 1。

- 单文件 materializer 保留原 MIME，只改文件格式 representation 为本机 file:// URI；测试宿主必须按既有 files 格式识别 URI，不能只凭 text/uri-list MIME。文件显示元数据同时写入系统 snapshot 是既有合同，完整 primary 内容仍须真实读取本体。

- 目录路径证据缺失已构造确定性真实 Iroh red（0.06s）：ObservedConnections.after_handshake 在观察任务登记锁 Busy 时直接返回，既丢初始真实选路，也丢该拒绝与连接的局部关联。修复把已确定的初始路径事实移到登记之前；观察无法登记仍诚实记录同连接 Busy，不扩大任务容量、不延长握手或改变业务关闭。

- 扩展目录二次激活在独立进程失败 2 次：两端记录表明状态已送达并重复 pull，随后 pulled-content store 拒绝，未产生第二次应用事件。`entry_availability_repo.rs::path_is_readable_regular_file` 只接受 metadata.is_file；冻结 main 与当前文件完全相同，本片未改此语义。目录首次全量内容/空目录系统读回及物理路径已通过。按 spec 保留业务可用性，不把本片扩大为目录可用性/重复存储重构；已有内容不重复 pull 用文本、图片和单文件真实双向验证。原失败记录在 target/active-clipboard-connectivity-evidence/directory-repeat-diagnosis。

- 工作区目录选路失败与观察锁 Busy 的早期问题不同：原 JSON 有成功建链终态/native 连接匿名编号及 Direct 快照，丢失的是 Started 与 AttemptStarted。有限非阻塞日志输出已有 submission.try_lock 丢弃计数策略。不能通过取消非阻塞策略或只看剩余节点伪称完整关联；本次让原连接终态保留自身 purpose，并让完整原生 receiver 通过已有不透明上下文报告原连接实际 channel 和匿名编号。选路检查仍只用实际本机连接事实，旧端源码不改、不能按跨端 hash/time 补父节点。
- 检查半帧测试时发现先前只写 b"U" 会立即触发坏 magic，不能当作合法半帧等待证据。最终全套结束后需改为 codec ACTIVE_PULL_MAGIC 单字节、不 FIN，补跑该用例覆盖真实缺少长度字段的 10 秒等待；不开流预算已有独立真实回归。

## 人工等价代码审查（验收门禁收尾前）

- 四向生产全部经过 Application 完整 sender/receiver/client；原 dispatcher 删除、原 client 仅 cfg(test)，原生 handler 只做既有不透明物理上下文、连接转换和一次完整调用。架构守卫覆盖目录并有回退负例。
- 入口绝对期限先于队列/身份/开流/帧/准备等待；FIN 保护 min(3 秒, 剩余预算)。状态无 ACK，日志与统计只称本地发送结算；系统应用由原 owner 判定。
- 两类 manager 各自使用原有限政策；本地实际工作捕获原 StreamUse 和会话许可。真实两文件、16 份取消额度、独立状态收取、阻塞池尚未开始时停止和等待者丢弃后的 IO 失败均有验收。
- 共享准备在每次后续发布/编码/加密前检查停止；已开始动作不拆分中断。原 reconstruct/plan/blob/manifest/cipher 仍只有一套完整 owner，无测试复刻业务流程。
- 授权沿用身份、登记和撤销快照；入站排队前捕获 fence，读取/交接/响应重检 grant；最终内容关系直接读取既有 Application scope，不用在线或发送开关代替授权。
- 帧/ALPN/状态码和大小限制未改；地址/准入/编解码/真实任务保持 source。Shared Offline/Timeout 仅按明确原分类投影，其他失败不伪装不可达。范围读取失败保持 NotAvailable 字节并返回本机原 source。
- 广播四目标有界 future 共用完整过滤/发送 helper；恢复槽单当前+单待发、顺序键不回退、类别/版本/到达时刻同锁更新，重复不刷新、已过静默窗口不再等待；上线集合保持独立 1500ms。
- 观测未增加 Engine 阶段/业务标识查询或跨层计时。初始物理事实与有界观察登记分开，终态自带既有目的；本机原生完整接收继承原连接匿名上下文，channel 只记录安全枚举。没有原生 key、地址或业务负载文本。
- 目录首次全量读回通过；目录二次激活既有可用性/重复存储债务明示，未扩大本片语义或预记通过。设备实测尚未做，须记跳过。
- code-review skill 未安装，本记录为人工等价审查；最终门禁或后续修复若改变代码需复核差异，不能据此提前归档计划。
