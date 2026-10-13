# 进度

2026-10-14：读取实现技能、Rust 技能、仓库约束与规格；核对远程 main；创建实现分支。阶段 0 尚未通过，未修改生产代码。

- 文本原实现基线通过（1/1，4.86s）；旧旧固定库基线通过（1/1，4.54s）。Engine 实现未修改；统一驱动包含恢复/当前状态/实际系统写入摘要。工件位于 target/active-clipboard-connectivity-evidence。
- 首次 host check 因测试 helper 名称 join_device 不存在失败，已修正为 join_host；后续测试构建通过。原始失败日志保留。
- 新增 codec 冻结非法/空值/尾随输入测试及已认证成员不开流的期限回归测试，待执行。

- codec 冻结边界 3/3 通过；无开流回归按预期失败（11.05s），旧 handler 没有入口总期限。慢目标广播测试已添加并编译中。
- 两文件真实取消验收尚缺，将在停止控制与完整本地工作桥接处继续补齐；未勾选阶段 0 完成。状态路径可独立推进，但不能因此宣称总体验收通过。

- 慢目标广播 red 按预期失败（1.00s）。状态完整收发已装配 Uni；新增 accept_exchange 保证单次协议入口的同一期限和额度，身份解析前冻结入站撤销 revision。state-check-3 通过（27.14s）；此前两次编译漏导入/枚举分支已修正，原日志保留。状态真实 iroh 回归执行中。

- 状态真实 iroh 回归 6/6 通过（11.36s），无开流 red 已转 green；四目标广播 red 已转 green。旧回调覆盖新激活的虚拟时钟 red 已加入并执行中。
- 编译期间曾修改 codec source 标注，MBX 拒绝缓存该输入；未绕过缓存。后续执行避免修改正在编译 crate 的输入。

- 恢复最新值槽：旧回调 red 按预期失败，迁移后 worker 9/9 通过（2.42s）。槽保存单候选与排序键、原到达时刻、版本；不保留无界状态队列。
- 拉取 requester/holder 与状态共四个方向已生产接线至 Uni；两个独立 manager 显式每连接额度 1。原 Core serve port 删除，完整准备 port 迁入 Application。
- 完整准备共享发布循环新增停止检查，阻塞池桥接捕获实际工作与原流额度/generation 许可；真实两文件/取消额度验收尚未完成。
- preparation-check-3 通过（13.71s）；pull-workspace-check-2 全部 workspace/all-targets 通过（42.36s）。批量迁移的 doc comment/import 顺序、测试重复 import 和遗漏参数编译失败已修复，保留失败日志。定向文本完整新流程执行中。

- 新 Uni 文本完整恢复 1/1 通过（4.21s）。真实两文件测试首次停在加入后的会话重建边界：QueryDeviceGroupChoices 返回 1103，尚未触发发布 A；夹具已按稳定查询的实际可用状态等待，并保留事件订阅。未放宽原预算，首失败保留于 real-cancellation.log。

- 独立进程文本完整内容 + 状态/pull 实际直连路径通过（4.84s）；图片与目录完整读回通过，单文件夹具误要求只有一份系统 representation（现有文件显示元数据使它为两份），已按真正可粘贴内容精确选择 primary，不放宽长度/哈希断言。unshare -Urnm 已确认可用；sudo 无免密权限，因此网络隔离采用 rootless user/network namespace，编译仍在普通用户下串行执行。

- 隔离直连：文本/图片/单文件通过；目录内容已完整读回，但拉取 selected-path 证据为空，门禁按失败结算（direct/junit.xml，directory.log）。不放宽路径要求；新增保存两端脱敏原始网络记录，使用独立诊断目录仅复现目录场景以定位证据缺口。

### 最新补验
- observability-check-2：workspace all-targets check 通过，27.88 秒。
- transport-tests：7/7 通过，包括四向第二 backend、dial/open/write 绝对预算、两种入站读期限、16 份取消后本地额度保留及独立状态接收、错误来源与稳定边界分类。
- direct-final：隔离直连四类全通过（text 4.628s / image 5.039s / file 4.833s / directory 7.729s）；原 directory 观测锁 Busy 失败证据仍保留在 direct，未覆盖。
- CI 已增 fixed-main active 双向与 direct/relay 必须步骤及证据保留。仓库架构检查更新已删除 adapter 的读取，并新增原生流程回退负例；尚待运行。
- relay-run 正在执行（session 96669）；所有 Cargo 仍串行。真实 pull 不开流/半帧测试已添加，尚未运行。
- relay：隔离双栈 UDP 阻断真实 Relay，四类全通过（10.699 / 11.249 / 10.880 / 11.248 秒）；保留 UDP 计数、物理路径、系统内容及 JUnit。
- infra-active-tests 正在执行（session 26240）；新增真实半帧/不开流，以及阻塞池排队取消回归。
- 最新值槽新增消费后更新/类别一致/重复不刷新测试；worker 新增 A 阻塞时 B/C/D 合并且剩余防抖不重置测试，待运行。
- infra-active-tests 首轮编译失败：测试误引入未依赖的 futures，改为 std::future::poll_fn；第二轮删除 admit 影响原身份 gate 单测，将其限制 cfg(test)。不增加依赖。
- infra-active-tests-3：17/19 通过；真实 silent/partial 请求期限、阻塞池未开始工作停止均通过。两失败原因有证据：缺地址 fixture 原身份目录为空导致正确 Denied；阻塞线程未继承线程日志 dispatcher 导致固定 Internal 记录不可见。前者改为已授权但无地址，后者在原本地工作桥接捕获并恢复 tracing dispatcher；正在 infra-active-tests-4（session 11376）验证。
- infra-active-tests-4：Internal 单次固定脱敏记录现通过；缺地址合法身份 fixture 仍失败，定位为 manager 共享 flight 将 Offline 包在 Shared 中，而投影只匹配直接 Offline。已改为仅递归识别 Shared(Offline) 与原 Offline，并新增稳定边界回归；Denied/其他来源仍保留 Io source，不泛化为不可达。
- 真实两文件测试补 holder.suspend 的排空等待断言，要求实际 A 结算前不得退休世代；待补跑。
- Application clipboard 测试 484/484 通过（18.08 秒），覆盖上述新槽/当前轮、准备等待、Shared Offline 与原业务故障/权限/系统写入回归。
- nextest 配置给独立进程 active binary 同原 history 的独占资源预算；四类旧新矩阵允许 120 秒 harness 总时长，不改变每个网络入口 10 秒。不开流+半帧两次真实期限给 harness 30 秒。
- active-cancellation-regressions（session 47762）在串行构建 Engine dev-tools 测试；目标 19 个 Infra + 真实两文件 holder 世代排空。
- 宿主增加稳定 delete/lock 及 ActiveClipboardChanged 事件摘要。完整四类场景扩为再次激活已有内容、不重复 pull；兼容场景补真实删除持有条目后的拒绝，须观察固定客户端拒绝日志与实际 pull 路径，不能仅凭未应用断言。
- active-cancellation-regressions：20/20 nextest ci 通过，23.152 秒；真实两文件补 holder 世代排空通过 3.856 秒；JUnit 已立即保存 active-cancellation-junit.xml。编译 4m49s，维持同一 target/cache。
- process-regressions 正在构建（session 74148）；实际删除持有条目/新旧拒绝、再次激活已有内容、稳定锁定接收方不写入场景已添加。明确区分稳定锁定会退休世代，Locked 状态字节由原 codec + 实际 Iroh handler 合同验证，不能把稳定锁定场景称为已收到 0x02。
- process-regressions-3：7/7 通过，33.728 秒。JUnit 保存 process-regressions-junit.xml；固定旧 main 单独 ignored 矩阵尚待执行，未预记通过。
- 目录二次激活两次失败后保存两端原诊断，确认既有可用性仓库只认可普通文件、冻结 main 实现逐字相同；状态送达后重复 pull 并存储失败。技术债已单列，按照 spec 保留业务语义；目录仍验证首次完整读回，不伪记二次无 pull 通过。诊断相对路径初次生成在 host/target 下，已移动两 JSON 到根 target 证据目录；其余既有 host/target 空目录未删除非本任务数据。
- fixed-main-interop 首次构建失败：共享 target 在相同 debug profile 下误认为当前 Core fresh；未构建出旧端，cleanup 已回收临时源码/恢复宿主。采用原 fixed-main-history 已有的 touch 源码时间戳 + CARGO_PROFILE_DEV_DEBUG=0 指纹隔离，不改变源码字节、不新建 target、不绕过缓存。fixed-main-interop-2 正在运行（session 39663）。旧 build-profile/driver摘要写入证据。
- 隔离脚本追加 Ubuntu runner 禁止 user namespace 时的 sudo -n -E fallback；Cargo 仍始终由原用户在命名空间外构建，命名空间只跑预建二进制。语法检查通过。
- fixed-main-interop-2：固定 main 2f9afc9 的同一新驱动成功构建，12 个子场景（四类两方向 + 无内容拒绝两方向 + 稳定锁定两方向）矩阵 1/1 通过 60.606 秒。driver/Engine archive/profile/binary 摘要及 interop-junit 已保存；临时源码与 backup 由 EXIT 回收。
- repository-preflight 首轮正确阻止 Application 复用已退休 Core ContentExchangeGatePort；未降低守卫。改为接既有 CurrentSpaceMemberScopePort，删除 Engine CurrentMemberContentGate 转接器与已无消费者的 Core port；正常拒绝/暂停过滤相同，读取失败仍回 NotAvailable 并在完整本机返回中保留来源。test fixtures 也迁同一 scope；新增来源+原响应双断言。
- member-scope-check：workspace all-targets locked 通过 1m15s；去掉 Engine 新 unused import。
- repository-preflight-2：总检查通过，含隐私/真实 sink、OpenMLS 和全部原/新负例，不把旧 native / Core 入口重新放行。node 架构 unit 测试 37/37；Rust style 检查通过。
- workspace-suite 正在串行执行（session 52634），env UC_EXPECT_DIAGNOSTIC_STACK=available UC_TEST_BUILD_SCOPE=workspace。之后保存 JUnit，再跑 evidence/membership smoke/既有 relay/upgrade/fixed history/E02，以及本片新最终 direct/relay/fixed 兼容。
- workspace-suite 首轮完整运行 4233 项：4231 通过、2 失败、25 nextest 跳过；用时 543.731 秒（构建另计）。全部新 scope/source/额度/取消/既有业务回归通过。失败为目录路径关联缺开始记录、以及 channel Vocabulary 新字段的可见文本迁移集合未同步。
- 新路径失败原 JSON 位于 target/test-artifacts/20261013T185057Z-100952/current-directory-{holder,requester}-network.json。实际 Direct 快照与完成 Native connection 436bee0c 已在，connect_id cae9cb0a 的 Started/AttemptStarted 丢失；LocalFileWriter 已有非阻塞 submission.try_lock 丢弃计数策略，不取消该策略或扩大资源。
- terminal-purpose-red 使用真实原记录独立断言完成记录自带 clipboard_pull 分类，确定性失败。ConnectionObservation 原 owner 保留既有 purpose 到 Finished/AttemptFinished；原 native bridge 通过已有 NetworkRecorder.in_connection_scope + ObservationContext 关联完整 receiver 的真实 channel 和本机匿名连接编号，未新增接口或阶段查询。测试只接受同进程实际终态/接收事实，不用跨端 hash/time 拼接；旧端仍用原 Started 关联。
- channel 为已有 Core ConnectionChannel 安全枚举词表（Direct/Relay/Unknown），新增到显式可见字段迁移集合；不增加地址或身份文本。
- terminal-purpose-green：13/13 通过，5.008 秒，含原目录场景、真实落盘终态分类+物理匿名关联、字段迁移集及全部 transport；JUnit 保存 terminal-purpose-junit.xml。
- 补真正开始后的工作在等待者丢弃时返回真实 IO 失败仍记录一次且脱敏的 LocalWork 测试。workspace-suite-final 正在执行（session 81348），不会并行 Cargo；后续仍需其余门禁、最终 direct/relay/fixed 与文档/提交/线上最终 head。
- workspace-suite-final 正在测试阶段（编译已结束）：目录原场景通过 4.693 秒，开始后的工作在等待者丢弃时仍记录 IO 失败的新测试通过 0.007 秒，目前未发现失败。
- 校正半帧测试输入为 codec ACTIVE_PULL_MAGIC 单字节且不 FIN；生产未改，该用例将于全套结束后单独重编译补跑，不能将此前立即 bad-magic 拒绝记为合法半帧等待通过。全套的其他生产代码与接线保持同一版本。
- workspace-suite-final 全套：4234/4234 通过，7 slow、25 配置跳过，561.136 秒；JUnit 即时保存 workspace-final-junit.xml。最终目录、channel 迁移集、真实 scope source 与等待者取消后真实失败记录均通过。
- valid-partial-frame 正在重编译并补验（session 62285），校正测试输入为有效 magic 后须实际经历两次 10 秒截止；仅测试输入变化，生产源码与已通过全套一致。
- valid-partial-frame：修正后 1/1 通过 20.088 秒，真实 silent 与合法单 magic 半帧各用原 10 秒预算，Junit 保存 valid-partial-frame-junit.xml。
- evidence-suite：18/18 通过 1.357 秒；success/failure 示例与要求的结构化结果齐备，JUnit 保存 evidence-junit.xml。构建复用根 target/cache，未新增隔离构建目录。
- membership-smoke 正在执行（session 84384），使用 PR workflow 同一过滤器及 workspace 构建范围；后续其余门禁仍未预记通过。
- membership-smoke：同 PR 过滤器 16/16 通过，197.692 秒；长分区实际 131.909 秒，未改旧期限。JUnit 保存 membership-smoke-junit.xml。
- 已通过 nextest list --workspace --all-targets 获取当前实际 history test binary，列表保存 workspace-list.json。
- 原 run-membership-history-relay.sh --inside 使用当前预建 binary，通过 unshare -Urnm 执行（session 54831），普通用户编译、共享 target/cache 不变；原脚本/旧 rev 未改。
- 原 history relay：--inside 当前预建 test binary 1/1 通过 16.68 秒，真实 inet UDP drop 计数断言通过，证据 target/membership-history-relay。
- 原 E02 direct：rootless user/net/mount namespace，私有 /run tmpfs 允许原 ip netns harness；--prebuilt 未再编译，原文本与文件传输均通过（191 / 278 ms）。全部 mount/netns 仅在子命名空间，不更改主机全局网络；原脚本源码未改。证据 target/connection-recovery-evidence/direct.json。
- upgrade-smoke 正在执行（session 16259），workspace scope、latest a13 5e8d3d4 anchor，原脚本使用根共享 target/debug=0/cache，不另建 Cargo target。
- upgrade-smoke：a13 同一新宿主成功构建，5/5 nextest 预期契约通过 15.175 秒；矩阵语义为 pass 3 / rejected 1 / known-incompatible 1，其余 63 not-run，未称完整 68 项矩阵通过。JUnit 保存 upgrade-smoke-junit.xml，报告 target/test-artifacts/20261013T195541Z-296088/upgrade-matrix/matrix.md。
- 原固定历史 edf0a296 rev 未改：V1/V2 双方向 2/2 通过 26.041 秒；JUnit 保存 fixed-history-junit.xml，旧来源和 binary SHA 仍在 target/fixed-main-history。
- 本片固定 main 最终源码矩阵正在重跑（session 18914），含最新 scope/不透明连接上下文/自足终态目的；最终 direct/relay、root checks/audit/clippy 和线上 head 仍待完成。
- fixed-main-interop-final：最终矩阵 1/1（12 子场景）通过 61.829 秒，旧源码/同一 driver/profile/hash/interop-junit 保留；EXIT 清理源目录并恢复当前宿主。
- 最终隔离 direct-gate-final：text/image/file/directory 4/4 通过（5.171/6.632/4.986/4.767 秒）。最终 relay-gate-final：4/4 通过（11.375/11.716/11.284/11.120 秒），inet 双栈 UDP drop 实际计数通过。两个目录各有四场景 JUnit、源码树含 untracked 摘要、binaries、原生实际路径及内容证据。
- root 验收检查串行运行（session 86584）：metadata locked、workspace all-targets check、fmt check、Rust style、repository preflight、diff check；之后 audit + direct-log clippy。生产代码自最后全套未改，只有合法半帧测试输入补正并已单独验收。

- 根门禁全部通过：metadata/check/fmt/style/repository/diff、直接日志宏 default/LAN 0 违规；cargo-audit 0.22.2 deny unsound 通过，8 条策略允许警告。
- 实现提交 0b653c7a1c4b4be8a98b924a8704a5888401ed5d 已推送；草稿 PR https://github.com/UniClipboard/Engine/pull/181。线上 PR Check 38466296023 与 Windows Build 38466296011 运行中，未预记通过；待结果后归档。
- 首次线上结果：Windows x64/arm64 与仓库检查通过；升级 smoke 在新固定旧端脚本入口报 just: command not found（127），旧端构建已成功。原始日志 online-upgrade-first.log 保留。两个新脚本均已 source env，改为直接 cargo，保持根 target/cache；修正后的旧新 12 场景 61.655 秒通过，Direct/Relay 各 4/4 通过；根 metadata/check/fmt/style/repository/diff 再次通过。
- 首次 Engine 日志还含文本路径断言失败：内容完整，但三条状态原生记录 initial_path=unknown（4–7ms 关闭），接收 Unknown 且无连接别名，不是缺少开始记录。保留 online-tests-first.log 与 online-first-artifacts。普通未隔离宿主不推断路径，路径验收明确跳过；隔离脚本 UC_ACTIVE_CLIPBOARD_EXPECTED_PATH 必须有实际路径，空记录失败。固定旧新 12 场景改为复用隔离 Direct 门禁并通过 61.44 秒。
- 路径范围修正验收：普通宿主 7/7 通过（1 固定旧端配置忽略），隔离 path-scope-direct 与 path-scope-relay 各 4/4 通过；隔离旧新矩阵 12 子场景通过，根 metadata/check/fmt/style/repository/diff 通过。生产源码未改，仅宿主断言范围及脚本运行环境修正。
