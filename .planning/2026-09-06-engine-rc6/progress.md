# 发布进度

## 2026-09-06

- 已检查公开发布、最近成功工作流、远端 main 和 rc.6 标签。
- 已更新 workspace、HarmonyOS 包和宿主版本断言；离线 metadata 仅更新 13 个 workspace 包的版本锁定。
- 核实 0ff09048 明确删除新旧联通门禁，修正发布说明的过期流程和 App 配置来源；没有修改工作流。
- 尚未推送或触发发布。
- 版本校验、发布流程测试 3 项、locked metadata、全目标编译、格式、仓库架构/隐私检查及 diff check 均通过；保留已有警告。
- 已准备升级提示，发布后附加到生成的 Release 说明，不覆盖任何资产。
- 文档路径首次查找使用了不存在的 design-docs/security；已按 docs/SECURITY.md 导航读取 docs/security/release-integrity.md。
