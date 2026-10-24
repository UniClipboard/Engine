# 网络栈 submodule 交付

## 完成标准

Engine 构建集成独占网络依赖来源；宿主只依赖 uc-engine，无根补丁即可编译完整修复链。
缺少或不符的 submodule 明确失败，由维护者更新 checkout 后重试。运行期业务、关闭、恢复责任不变。

## 切片

1. 在单一网络栈 fork 内固定 iroh、blobs、tickets、util 的路径依赖，保留许可证与补丁来源。
2. Engine 使用单一 submodule，移除根补丁；CI、日志来源审查和文档同步。
3. 独立 Git 消费者仅依赖 uc-engine，验证 submodule 获取、唯一网络依赖来源及编译。
4. 执行 DNS 重放、日志规则、workspace 编译、格式与架构门禁。

## 验收状态

本地实现与验收完成：

- 独立 Git 消费者无根补丁编译通过；七个网络包各一份 Engine Git 来源，修复 DNS 链正确传递。
- Engine UDP EDNS 重放通过；网络日志来源规则 4 项通过（Unix、Windows 路径、Unicode 路径与拒绝未知布局）。
- metadata --locked、workspace all-targets check、fmt、Rust 风格、仓库门禁（含负向依赖来源测试、隐私与 OpenMLS 验证）通过。
- iroh Rust 源码未变化；blobs 仅初始化表达式格式化。
- Windows / 移动端构建与实机运行、远端 CI、Release bundle：跳过。此前根补丁方案同步成功不作为本次实机验收。

宿主迁移到包含本变更的 Engine 提交后删除历史四个 iroh 根补丁。
