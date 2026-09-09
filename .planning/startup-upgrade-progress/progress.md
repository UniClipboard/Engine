# 实施记录

- 2026-09-09：读取交接、产品方案、仓库及文档规则；核实修复已在主线。恢复权限后 fetch 成功，启动基线 workspace check。
- 接口方向：单次可消费的进度输入和可克隆的只读观察者；有限快照保留步骤与终态，无宿主回调和无界事件队列。
- 本机首轮：Engine 通道 4 项通过，后续真实启动/失败/升级后服务失败等 8 项通过。Infra 升级库 21 项通过，集成 17 项、独立进程中断恢复 1 项通过；架构 preflight 通过，workspace check 通过。
- 新增 journal 兼容测试证明旧 postcard 前缀保持不变、旧读取器仍接受新增加密警告尾部，新读取器接受旧记录且不把未知警告当零。超出转换总数的警告测试先失败，补齐校验后通过。
- 使用 alpha.5 实际 CLI 在独立 synthetic profile 生成文本；Engine 公共入口升级、真实历史读取、关闭和重启测试通过（6.13 秒）。没有用户资料或原设备密钥。
- 大批量测试：4096 个额外内容表示，同一事件合计 4098 个表示，加两个 blob（其中一个不可读保留），转换、计数、恢复、篡改拒绝全部通过（34.28 秒）。
- Windows：干净旧验证 checkout fetch 最新 main 后新建 verify/startup-upgrade-progress；使用同一补丁和 synthetic fixture。验证脚本正在串行执行升级库、集成/进程恢复、Engine 启动及 alpha.5 端到端。
- 架构检查最初因固定字符串要求失败，保留原 Engine/EventStream 导出并单独增加新导出后通过。运行摘要转换已移入 assembly，生命周期通道不依赖 Infra 类型。
- 普通初始化重启误报先以真实启动测试复现，再通过实际旧资料/已转换数量判断修复；最新 Engine startup 9 项在 macOS 与 Windows 均通过。
- Windows 最终代码复跑全部成功：21 项升级库、17 项集成、1 项进程恢复、9 项 startup、1 项 alpha.5 公共入口。4098 表示/2 blob 补充压力与恢复检查通过（85.05 秒）。
- 两机 crates tree 一致：6ba8ac8f96a5eca4bdbe012df90549051257538a。最新 workspace check、metadata、fmt、架构/隐私、公共合同 45 项、依赖边界 34 项和 diff 检查通过。
- 完整范围与明确跳过项已写入 docs/exec-plans/completed/startup-upgrade-progress.md。分支检查通过；准备本地提交，不发布。
