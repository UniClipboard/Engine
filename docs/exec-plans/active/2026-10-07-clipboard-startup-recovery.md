# 剪贴板启动核对与宿主导入恢复

状态：实现及本地验收完成，等待 PR 与下游不可变来源接入。关联 UniClipboard/UniClipboard #1886；基线为已合并 PR #160。

## 完整责任与不变量

- Application 的 active-state reconcile 是启动核对的完整负责人；调用方只执行一次 reconcile，成功后才能启动读取/传播 register 的 worker。
- register 为空代表不对当前 OS 内容作声明；非空必须由重建快照与实时快照匹配来确认。核对不得写 OS 剪贴板、广播或伪造新激活时间。
- OS 读取/宿主导入失败不能确认旧 register：成功清除后允许启动，下次真实剪贴板变化由现有捕获流程处理。加载或清除 DB 失败仍阻断启动；身份、密钥、安全与其他启动阶段不降级。
- Engine 宿主适配器拥有单次导入目录：所有 representation 一起成功才交出快照；任何失败删除本次目录（包括之前已复制的 rep），不得删除其他成功调用目录或原文件。成功目录保持现有消费/缓存生命周期。
- HostCapabilityError 保存原始 source；启动错误在既有 startup_error 路由中记录安全因果链，EngineError/FFI 仍只返回稳定分类。Debug/Display/序列化仍脱敏。诊断沿既有 `error =` 安全渲染链输出固定 IO kind/os code，禁止格式化原始错误正文。

## 编码前失败模式

1. 空 register：不得触发启动 OS read。
2. 非空匹配/不匹配、重建失败、OS read 失败、源 metadata/open 权限拒绝或文件消失。
3. 创建导入根/单次目录失败、目标 create_new 失败、第一块或后续块读取失败、大小变化、短读/空块、写入/flush 失败。
4. 多 rep 后一项失败：清除本次所有文件；另一成功调用目录和原文件保持不变。
5. 读取失败后 DB reset 失败：保留 DB 错误因果链，启动失败；诊断同时保留读取失败与 reset 失败证据。
6. 失败后重启、随后正常文本/文件捕获仍可用；不得长期保持假 active state。
7. cleanup 本身失败：保留告警，不掩盖原错误；不声称故障文件已清理。
8. 标准采集经宏、模块日志过滤、队列刷新、实际 ZIP 导出后可能丢失 source/class 或泄漏路径；须解压核验而非只检查日志调用。

## 最小验收与扩展矩阵

先实现独立进程中真实 Engine、真实 SQLite/profile、HostClipboardAdapter 文件路径的启动恢复 E2E，保存原版失败与修复后 JSON/日志/退出码。使用隔离宿主剪贴板和文件 fault adapter，不操作全局剪贴板、真实 profile 或钥匙串。

扩展覆盖上述 register 与导入矩阵、重启与后续同步、reset 的真实 SQLite 故障、标准诊断 ZIP 中具体错误链及隐私哨兵。生产 Desktop daemon/平台宿主的最小跨仓改动见下节。合成 PermissionDenied 不能证明 macOS TCC；真实 Finder/TCC 现场未执行时记为跳过。

## Desktop 跨仓最小契约

Desktop 的 HostClipboard::read 与 HostFileAccess metadata/open/seek/read 当前转换丢弃 source，必须改为保留具体错误（平台 anyhow 用 into_boxed_dyn_error）；不改 detached stdout/stderr 策略、不提高正常 INFO 等级、不打开详细日志。

Engine 新增 HostCapabilityError 携带 source 的构造入口，保持稳定 category 与脱敏表示；启动错误映射前记录完整可用来源，稳定 EngineError 不新增原始来源字段。Desktop 修改仅在本 thread 独立 checkout 中进行。开发验证可使用此 Engine checkout，发布 pin 必须等待合并后改为不可变提交，不能提交绝对本地 override。Engine 未合并期间 Desktop 交付是明确依赖的待接入改动，不能伪称产品已完成来源 pin。

## 交付

运行相关 E2E 与根目录交付门禁；记录真实进程 PID、源码 revision、命令、rc、诊断 ZIP 与重跑索引。创建关联 #1886 的 PR，跟进 CI/审查，不 merge/release，不评论 issue。

## 已完成的实现与验收

Application 在 OS 读取失败时先清除 register，再允许启动；加载/清除持久化失败分别记录安全来源并继续阻断启动。Engine 将单次文件导入的所有提前返回统一纳入清理，目录只有创建成功后才归本次调用所有。HostCapabilityError 新增保留来源的构造入口，Debug/Display 与稳定 EngineError 的脱敏边界保持一致；startup_error 与导入/核对告警使用既有安全错误链渲染。

先用未修改生产代码的基线运行新进程验收，复现 code 1101；修复后九项故障矩阵通过，覆盖源打开/消失/中途读取、平台读取、metadata、空块、多 representation、目标创建和单次目录创建。实际 SQLite DELETE trigger 拒绝清除时仍返回 1101，移除故障后可重启恢复；空 register 跳过读取、匹配文本保留 register、正常文件/文本捕获与其他成功导入目录保留均已验收。标准 ZIP 含具体 PermissionDenied/NotFound/UnexpectedEof 来源，隐私哨兵未出现。

隔离 Desktop checkout 的实际适配器进程验收通过，来源为真实文件 metadata/open 错误和平台故障。真实 `uniclipd` 进一步通过私有命名 NSPasteboard、HTTP、SQLite/profile 与实际文件 I/O 验收：chmod 拒绝、只针对测试源文件的 ENOENT/EIO 注入、同故障再次启动、后续文本同步与正常文件捕获。标准导出实际包含 PermissionDenied/os code 13、NotFound/2 和 Uncategorized/5。Desktop 原有文件 DEBUG 记录的路径字段也纳入最小跨仓修复；完成后需重新检查标准 ZIP 的 Engine 和宿主两类日志。

根目录 metadata、workspace/all-targets check、fmt、Rust 风格、仓库所有权检查和 diff 检查已通过。实际证据索引、PID、命令、退出码、源码状态和 ZIP 留在本任务交付报告及独立工件目录；此文档只维护长期契约，不记录绝对开发路径。

## 仍未完成的边界

- Engine PR 合并与 Desktop 最终不可变来源接入由协调者推进；不提交绝对本地 override，不把本地联调当成已发布产品。
- Finder/TCC 现场复现、Windows/Linux 与移动设备矩阵未执行，记为跳过。
- 写入/flush/清理本身故障已列入失败模型，统一清理路径已覆盖；未逐项注入这些底层故障，不扩大已验证矩阵。
