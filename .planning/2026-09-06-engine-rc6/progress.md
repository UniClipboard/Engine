# 发布进度

## 2026-09-06

- 已检查公开发布、最近成功工作流、远端 main 和 rc.6 标签。
- 已更新 workspace、HarmonyOS 包和宿主版本断言；离线 metadata 仅更新 13 个 workspace 包的版本锁定。
- 核实 0ff09048 明确删除新旧联通门禁，修正发布说明的过期流程和 App 配置来源；没有修改工作流。
- 尚未推送或触发发布。
- 版本校验、发布流程测试 3 项、locked metadata、全目标编译、格式、仓库架构/隐私检查及 diff check 均通过；保留已有警告。
- 已准备升级提示，发布后附加到生成的 Release 说明，不覆盖任何资产。
- 发布来源 1631a370399ebcd0821b2163ca3594d63fad3451 已推送 main，远端 SHA 已核对一致。
- 正式发布已启动：https://github.com/UniClipboard/Engine/actions/runs/34015768332；headSha 正确，prerelease=true，dry_run=false。
- HarmonyOS 作业已成功完成并上传资产；iOS、Android 仍在构建。公开 Release 尚未生成。
- rc.6 版本下补跑 Engine 全部 146 项库测试通过，零失败、零跳过。
- Android 与 HarmonyOS 作业均成功，仍等待 iOS。尝试读取已完成 Android 作业日志时，GitHub 提示整轮运行结束后才可下载；不重复请求，最终统一检查。
- 三端构建全部成功，汇总作业已开始下载三端资产，准备统一校验和发布。
- 外置盘卷根目录不允许当前用户创建验证目录；已确认用户拥有 cargo-targets 目录，改在那里建立唯一临时资产核验目录，不修改 Cargo 构建目录配置，核验后回收。
- 运行 34015768332 全部成功，公开预发布 v1.1.0-rc.6 于 2026-09-06 06:42:37 UTC 创建；远端标签精确指向 1631a370399ebcd0821b2163ca3594d63fad3451。
- CI 已运行 verify-release-bundle.mjs，核验 22 份资产；加上清单共 23 个公开下载文件。桌面和移动通知作业成功，不代表产品采用完成。
- 已附加升级提示并保留自动生成说明，未覆盖资产。正在下载完整公开产物进行第二次核验。
- 23 个公开文件全部下载完成，本机 verify-release-bundle.mjs 再次通过 22 份资产的大小、SHA-256、完整清单与调试资料检查。
- 清单版本与远端标签源码均为 v1.1.0-rc.6 / 1631a370399ebcd0821b2163ca3594d63fad3451，下载锁文件校验值与发布提交中的 Cargo.lock 一致。
- 三端设备矩阵均保留 skipped 和原因。没有把 CI 构建、通知成功当作产品实机采用通过。
- 已确认无活动进程使用后删除本次唯一外置临时下载目录，公开资产不受影响，可从 Release 再下载。
- 最终地址：https://github.com/UniClipboard/Engine/releases/tag/v1.1.0-rc.6
- 文档路径首次查找使用了不存在的 design-docs/security；已按 docs/SECURITY.md 导航读取 docs/security/release-integrity.md。
