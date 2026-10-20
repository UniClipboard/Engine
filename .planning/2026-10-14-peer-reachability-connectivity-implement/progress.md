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
